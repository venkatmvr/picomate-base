// main.rs — PicoMate baseline firmware
//
// This file is the orchestrator only — no peripheral logic lives here.
// Each subsystem has its own module; see src/{oled,wifi,rgb,button,ota,...}.rs
//
// Execution order:
//   1. embassy_rp::init()  — claim all peripherals
//   2. mark_booted()       — confirm this firmware is good (OTA rollback protection)
//   3. OLED                — display first so all status is visible
//   4. wifi::init()        — CYW43 chip, embassy-net stack, AP join, DHCP
//   5. OTA listener        — TCP port 4242, background task
//   6. GPIO inputs         — button, encoder, PIR
//   7. Buzzer              — PWM on GP15
//   8. RGB LED             — WS2812 via PIO1 SM0
//   9. Main loop           — 100ms tick

#![no_std]
#![no_main]

mod button;
mod buzzer;
mod encoder;
mod oled;
mod ota;
mod pir;
mod rgb;
mod wifi;

use core::cell::RefCell;
use defmt::*;
use embassy_boot::BlockingFirmwareState;
use embassy_boot_rp::{AlignedBuffer, FirmwareUpdaterConfig, WatchdogFlash};
use embassy_executor::Spawner;
use embassy_rp::flash::ERASE_SIZE;
use embassy_rp::gpio::{Input, Pull};
use embassy_rp::i2c;
use embassy_rp::peripherals::PIO1;
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::bind_interrupts;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use heapless::String;
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

const FLASH_SIZE: usize = 2 * 1024 * 1024;

// ── Change this to prove an OTA update worked ─────────────────────────────────
// Flash v1.0 via `./flash.sh combined`, then change to "2.0" and run `./flash.sh ota`.
// If the OLED version changes — OTA works.
const APP_VERSION: &str = "1.0";

// Flash mutex shared between mark_booted (startup) and ota_task (runtime).
use ota::FlashMutex;
static FLASH_MUTEX: StaticCell<FlashMutex> = StaticCell::new();

// PIO1 is for WS2812 RGB — PIO0 is owned by wifi.rs
bind_interrupts!(struct Irqs {
    PIO1_IRQ_0 => InterruptHandler<PIO1>;
});

#[embassy_executor::task]
async fn ota_task(
    stack: embassy_net::Stack<'static>,
    flash: &'static FlashMutex,
) -> ! {
    ota::listen(stack, flash).await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // ── Flash — shared by mark_booted (now) and ota_task (later) ─────────────
    // WatchdogFlash resets the watchdog during long erase ops. 30s timeout gives
    // WiFi+DHCP time to complete before the watchdog would fire on a bad image.
    let raw_flash = WatchdogFlash::<FLASH_SIZE>::start(p.FLASH, p.WATCHDOG, Duration::from_secs(30));
    let flash: &'static FlashMutex =
        FLASH_MUTEX.init(Mutex::new(RefCell::new(raw_flash)));

    // ── Mark firmware good (OTA rollback protection) ──────────────────────────
    // If new firmware crashes before mark_booted(), watchdog fires and bootloader
    // rolls back to the previous ACTIVE image on next boot.
    {
        let config = FirmwareUpdaterConfig::from_linkerfile_blocking(flash, flash);
        let mut aligned = AlignedBuffer([0u8; ERASE_SIZE]);
        let mut state = BlockingFirmwareState::from_config(config, &mut aligned.0);
        state.mark_booted().ok(); // ok() — first-ever boot has no STATE, that's fine
    }

    // ── OLED (I2C0: GP16=SDA, GP17=SCL) ──────────────────────────────────────
    let i2c = i2c::I2c::new_blocking(p.I2C0, p.PIN_17, p.PIN_16, i2c::Config::default());
    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().unwrap();
    let style = oled::make_style();
    let mut hdr: String<16> = String::new();
    core::fmt::write(&mut hdr, format_args!("PicoMate v{}", APP_VERSION)).ok();
    oled::print(&mut display, style, &[hdr.as_str(), "WiFi..."]);

    // ── WiFi (PIO0 SM0, GP23/24/25/29 + DMA0) ────────────────────────────────
    let mut wifi = wifi::init(
        &spawner,
        p.PIN_23, p.PIN_25, p.PIO0, p.PIN_24, p.PIN_29, p.DMA_CH0,
    ).await;
    oled::print(&mut display, style, &[hdr.as_str(), "WiFi OK", wifi.ip.as_str(), "Init..."]);

    // ── OTA listener (TCP :4242, background) ─────────────────────────────────
    spawner.spawn(ota_task(wifi.stack, flash)).unwrap();

    // ── Button (GP26, active-low, pull-up) ────────────────────────────────────
    let btn = button::Button::new(Input::new(p.PIN_26, Pull::Up));

    // ── Encoder (GP6=CLK, GP7=DT, pull-up, polled) ───────────────────────────
    let mut enc = encoder::Encoder::new(
        Input::new(p.PIN_6, Pull::Up),
        Input::new(p.PIN_7, Pull::Up),
    );

    // ── PIR motion sensor (GP18, AS312, active-high) ──────────────────────────
    // NOTE: stuck LOW — hardware issue logged in tasks/todo.md
    let pir = pir::Pir::new(Input::new(p.PIN_18, Pull::Down));

    // ── Buzzer (GP15, PWM slice 7 channel B, 2 kHz) ───────────────────────────
    let mut buzz = buzzer::Buzzer::new(p.PWM_SLICE7, p.PIN_15);

    // ── WS2812 RGB LED (PIO1 SM0, GP22) ──────────────────────────────────────
    let mut pio1 = Pio::new(p.PIO1, Irqs);
    let mut rgb = rgb::Ws2812::new(&mut pio1.common, pio1.sm0, p.PIN_22);

    info!("all init done — OTA listening on :4242");
    oled::print(&mut display, style, &[hdr.as_str(), "Ready", wifi.ip.as_str(), "OTA:4242"]);

    // ── Main loop: 100 ms tick ────────────────────────────────────────────────
    let mut tick: u32 = 0;
    let mut led_on = false;

    loop {
        Timer::after(Duration::from_millis(100)).await;
        tick += 1;

        let enc_count = enc.update();
        let pressed   = btn.is_pressed();

        let hue = (enc_count as u8).wrapping_mul(5);
        let (r, g, b) = rgb::wheel(hue);
        rgb.write_color(r, g, b).await;

        if pressed { buzz.on(); } else { buzz.off(); }

        if tick % 5 == 0 {
            led_on = !led_on;
            wifi.control.gpio_set(0, led_on).await;
        }

        if tick % 5 == 0 {
            let mut line2: String<24> = String::new();
            core::fmt::write(
                &mut line2,
                format_args!("Btn:{} PIR:{}", if pressed { "On " } else { "Off" }, pir.state_str()),
            ).ok();
            let mut line3: String<24> = String::new();
            core::fmt::write(
                &mut line3,
                format_args!("Enc:{:+} RGB:{}", enc_count, hue),
            ).ok();
            oled::print(&mut display, style, &[
                hdr.as_str(),
                line2.as_str(),
                line3.as_str(),
                wifi.ip.as_str(),
            ]);
        }
    }
}
