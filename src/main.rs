// main.rs — PicoMate baseline firmware
//
// This file is the orchestrator only — no peripheral logic lives here.
// Each subsystem has its own module; see src/{oled,wifi,rgb,button,...}.rs
//
// Execution order:
//   1. embassy_rp::init()  — claim all peripherals
//   2. OLED                — display first so all status is visible
//   3. wifi::init()        — CYW43 chip, embassy-net stack, AP join, DHCP
//   4. GPIO inputs         — button, encoder, PIR
//   5. Buzzer              — PWM on GP15
//   6. RGB LED             — WS2812 via PIO1 SM0
//   7. Main loop           — 100ms tick

#![no_std]
#![no_main]

mod button;
mod buzzer;
mod encoder;
mod oled;
mod pir;
mod rgb;
mod wifi;

use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Input, Pull};
use embassy_rp::i2c;
use embassy_rp::peripherals::PIO1;
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::bind_interrupts;
use embassy_time::{Duration, Timer};
use heapless::String;
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};
use {defmt_rtt as _, panic_probe as _};

// PIO1 is for the WS2812 RGB LED — PIO0 is owned by wifi.rs
bind_interrupts!(struct Irqs {
    PIO1_IRQ_0 => InterruptHandler<PIO1>;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // ── OLED (I2C0: GP16=SDA, GP17=SCL) ──────────────────────────────────────
    let i2c = i2c::I2c::new_blocking(p.I2C0, p.PIN_17, p.PIN_16, i2c::Config::default());
    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().unwrap();
    let style = oled::make_style();
    oled::print(&mut display, style, &["PicoMate v1", "WiFi..."]);

    // ── WiFi (PIO0 SM0, GP23/24/25/29 + DMA0) ────────────────────────────────
    let mut wifi = wifi::init(
        &spawner,
        p.PIN_23, p.PIN_25, p.PIO0, p.PIN_24, p.PIN_29, p.DMA_CH0,
    ).await;
    oled::print(&mut display, style, &["PicoMate v1", "WiFi OK", wifi.ip.as_str(), "Init..."]);

    // ── Button (GP26, active-low, pull-up) ────────────────────────────────────
    let btn = button::Button::new(Input::new(p.PIN_26, Pull::Up));

    // ── Encoder (GP6=CLK, GP7=DT, pull-up, polled) ───────────────────────────
    let mut enc = encoder::Encoder::new(
        Input::new(p.PIN_6, Pull::Up),
        Input::new(p.PIN_7, Pull::Up),
    );

    // ── PIR motion sensor (GP18, AS312, active-high) ──────────────────────────
    // NOTE: PIR stuck LOW — hardware issue logged in tasks/todo.md
    let pir = pir::Pir::new(Input::new(p.PIN_18, Pull::Down));

    // ── Buzzer (GP15, PWM slice 7 channel B, 2 kHz) ───────────────────────────
    let mut buzz = buzzer::Buzzer::new(p.PWM_SLICE7, p.PIN_15);

    // ── WS2812 RGB LED (PIO1 SM0, GP22) ──────────────────────────────────────
    let mut pio1 = Pio::new(p.PIO1, Irqs);
    let mut rgb = rgb::Ws2812::new(&mut pio1.common, pio1.sm0, p.PIN_22);

    info!("all init done");
    oled::print(&mut display, style, &["PicoMate v1", "Ready", wifi.ip.as_str()]);

    // ── Main loop: 100 ms tick ────────────────────────────────────────────────
    let mut tick: u32 = 0;
    let mut led_on = false;

    loop {
        Timer::after(Duration::from_millis(100)).await;
        tick += 1;

        let enc_count = enc.update();
        let pressed   = btn.is_pressed();

        // Encoder → RGB hue (~51 clicks per full wheel)
        let hue = (enc_count as u8).wrapping_mul(5);
        let (r, g, b) = rgb::wheel(hue);
        rgb.write_color(r, g, b).await;

        // Buzzer follows button
        if pressed { buzz.on(); } else { buzz.off(); }

        // Blink CYW43 onboard LED every 500 ms
        if tick % 5 == 0 {
            led_on = !led_on;
            wifi.control.gpio_set(0, led_on).await;
        }

        // OLED update every 500 ms
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
                "PicoMate v1",
                line2.as_str(),
                line3.as_str(),
                wifi.ip.as_str(),
            ]);
        }
    }
}
