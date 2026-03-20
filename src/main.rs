// main.rs — PicoMate baseline firmware
//
// Phase 1: OLED + LED blink (foundation, no peripherals yet)
// Each later phase adds a peripheral and displays its reading on the OLED.
//
// Execution order:
//   1. embassy_rp::init()     — set up all RP2040 peripherals
//   2. I2C + OLED init        — display is first so we can show errors
//   3. CYW43 init             — needed for onboard LED (it's on the WiFi chip)
//   4. Spawn wifi_task        — runs the CYW43 event loop forever in background
//   5. Main loop              — blinks LED, updates OLED

#![no_std]
#![no_main]

mod oled;

use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::i2c;
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::bind_interrupts;
use embassy_time::{Duration, Timer};
use heapless::String;
use core::fmt::Write as FmtWrite;
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

// Wire up PIO0's interrupt to the Embassy handler.
// Embassy needs this to wake async tasks when the CYW43 SPI transfer completes.
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

// ── Background task: runs the CYW43 WiFi chip event loop ─────────────────────
// This task never returns. It processes CYW43 internal events (scan results,
// connection state changes, received packets) and must always be running.
// It yields cooperatively whenever there's nothing to process.
#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

// ── Entry point ───────────────────────────────────────────────────────────────
#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // embassy_rp::init() claims ownership of every peripheral register.
    // Returns a Peripherals struct — each field is a zero-sized token proving
    // you have exclusive access to that peripheral.
    let p = embassy_rp::init(Default::default());

    // ── OLED init (I2C0, GP16=SDA, GP17=SCL) ─────────────────────────────────
    // We init the display first so we can show status/errors from the start.
    // I2c::new_blocking() configures the I2C hardware registers synchronously.
    // The ssd1306 crate wraps it into a display object that knows about
    // the SSD1306/SSD1315 command set (contrast, addressing mode, etc.).
    let i2c = i2c::I2c::new_blocking(
        p.I2C0,
        p.PIN_17, // SCL
        p.PIN_16, // SDA
        i2c::Config::default(),
    );
    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().unwrap();

    let style = oled::make_style();
    oled::print(&mut display, style, &["PicoMate v1", "Starting..."]);

    // ── CYW43 init ───────────────────────────────────────────────────────────
    // The WiFi chip firmware is embedded in the binary at compile time.
    // include_bytes! reads the file from disk into a &[u8] in flash (.rodata).
    let fw  = include_bytes!("../cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

    // Standard Pico W CYW43 wiring (fixed on PCB, not configurable):
    //   PIN_23 = WL_ON (power enable)
    //   PIN_25 = WL_CS (SPI chip select)
    //   PIN_24 = WL_MOSI / WL_CLK data
    //   PIN_29 = WL_CLK
    //   DMA_CH0 = DMA channel for SPI transfers
    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs  = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    // PioSpi programs a PIO state machine to bitbang SPI at ~31 MHz.
    // Regular SPI peripheral can't reach CYW43's required timing.
    let spi = PioSpi::new(
        &mut pio.common, pio.sm0, DEFAULT_CLOCK_DIVIDER, pio.irq0,
        cs, p.PIN_24, p.PIN_29, p.DMA_CH0,
    );

    // cyw43::State holds the chip's internal state (buffers, connection info).
    // Must be 'static because cyw43_task borrows it forever.
    static CYW43_STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = CYW43_STATE.init(cyw43::State::new());

    // cyw43::new() returns:
    //   net_device — implement NetworkDevice trait (used later for WiFi)
    //   control    — send commands to the chip (join, LED, power management)
    //   runner     — the event loop that must run in a background task
    let (_net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    spawner.spawn(cyw43_task(runner)).unwrap();

    // Upload the CLM (regulatory/calibration) blob to the chip.
    // Must happen after spawn so the runner task can process the upload.
    control.init(clm).await;

    info!("init done");
    oled::print(&mut display, style, &["PicoMate v1", "Ready"]);

    // ── Main loop: blink LED + show uptime on OLED ────────────────────────────
    // The LED is GPIO 0 on the CYW43 chip (not an RP2040 pin).
    // control.gpio_set() sends a command to the CYW43 over PIO SPI.
    let mut tick: u32 = 0;
    loop {
        control.gpio_set(0, true).await;
        Timer::after(Duration::from_millis(500)).await;
        control.gpio_set(0, false).await;
        Timer::after(Duration::from_millis(500)).await;

        tick += 1;
        let mut uptime: String<32> = String::new();
        write!(uptime, "up {}s", tick).ok();
        oled::print(&mut display, style, &["PicoMate v1", "Ready", uptime.as_str()]);
    }
}
