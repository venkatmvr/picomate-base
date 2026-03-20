// main.rs — PicoMate baseline firmware
//
// Phase 1: OLED + LED blink (done)
// Phase 2: Button (GP26) + WS2812 RGB (GP22, PIO1) + Encoder (GP6/GP7)
//          + Buzzer (GP15, PWM) + PIR AS312 (GP18)
//
// Execution order:
//   1. embassy_rp::init()    — claim all peripherals
//   2. I2C + OLED            — display first so errors are visible
//   3. CYW43 (PIO0 SM0)      — onboard LED lives on the WiFi chip
//   4. GPIO inputs           — button, encoder, PIR
//   5. Buzzer (PWM slice 7)  — GP15
//   6. WS2812 (PIO1 SM0)     — RGB LED via PIO
//   7. Main loop             — 100ms tick, updates all outputs + OLED

#![no_std]
#![no_main]

mod button;
mod buzzer;
mod encoder;
mod oled;
mod pir;
mod rgb;

use cyw43_pio::{DEFAULT_CLOCK_DIVIDER, PioSpi};
use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::i2c;
use embassy_rp::peripherals::{DMA_CH0, PIO0, PIO1};
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::bind_interrupts;
use embassy_time::{Duration, Timer};
use heapless::String;
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    PIO1_IRQ_0 => InterruptHandler<PIO1>;
});

#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // ── OLED (I2C0: GP16=SDA, GP17=SCL) ─────────────────────────────────────
    let i2c = i2c::I2c::new_blocking(
        p.I2C0,
        p.PIN_17,
        p.PIN_16,
        i2c::Config::default(),
    );
    let interface = I2CDisplayInterface::new(i2c);
    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();
    display.init().unwrap();

    let style = oled::make_style();
    oled::print(&mut display, style, &["PicoMate v1", "CYW43..."]);

    // ── CYW43 (PIO0 SM0: onboard LED + future WiFi) ──────────────────────────
    let fw  = include_bytes!("../cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs  = Output::new(p.PIN_25, Level::High);
    let mut pio0 = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio0.common, pio0.sm0, DEFAULT_CLOCK_DIVIDER, pio0.irq0,
        cs, p.PIN_24, p.PIN_29, p.DMA_CH0,
    );

    static CYW43_STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = CYW43_STATE.init(cyw43::State::new());
    let (_net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    spawner.spawn(cyw43_task(runner)).unwrap();
    control.init(clm).await;
    oled::print(&mut display, style, &["PicoMate v1", "CYW43 OK", "GPIO..."]);

    // ── Button (GP26, active-low, pull-up) ───────────────────────────────────
    let btn = button::Button::new(Input::new(p.PIN_26, Pull::Up));

    // ── Encoder (GP6=CLK, GP7=DT, pull-up) ───────────────────────────────────
    let mut enc = encoder::Encoder::new(
        Input::new(p.PIN_6, Pull::Up),
        Input::new(p.PIN_7, Pull::Up),
    );

    // ── PIR motion sensor (GP18, AS312, active-high)
    // Pull::Down ensures line reads LOW when idle (needed if output is open-drain)
    let pir = pir::Pir::new(Input::new(p.PIN_18, Pull::Down));

    // ── Buzzer (GP15, PWM slice 7 channel B) ─────────────────────────────────
    let mut buzz = buzzer::Buzzer::new(p.PWM_SLICE7, p.PIN_15);
    oled::print(&mut display, style, &["PicoMate v1", "CYW43 OK", "GPIO OK", "RGB..."]);

    // ── WS2812 RGB LED (PIO1 SM0, GP22) ──────────────────────────────────────
    let mut pio1 = Pio::new(p.PIO1, Irqs);
    let mut rgb = rgb::Ws2812::new(&mut pio1.common, pio1.sm0, p.PIN_22);

    info!("init done");
    oled::print(&mut display, style, &["PicoMate v1", "Ready"]);

    // ── Main loop: 100ms tick ────────────────────────────────────────────────
    // Encoder controls RGB hue — each step shifts 3 hue positions (85 clicks = full wheel).
    // Buzzer on while button held.
    // OLED: line1=header, line2=btn+pir, line3=enc+rgb, line4=sensors (populated later).
    let mut tick: u32 = 0;
    let mut led_on = false;

    loop {
        Timer::after(Duration::from_millis(100)).await;
        tick += 1;

        // Poll inputs
        let enc_count = enc.update();
        let pressed   = btn.is_pressed();

        // Encoder → RGB hue  (count * 5 gives full wheel in ~51 steps)
        let hue = (enc_count as u8).wrapping_mul(5);
        let (r, g, b) = rgb::wheel(hue);
        rgb.write_color(r, g, b).await;

        // Buzzer follows button
        if pressed { buzz.on(); } else { buzz.off(); }

        // Blink onboard LED every 5 ticks (500ms)
        if tick % 5 == 0 {
            led_on = !led_on;
            control.gpio_set(0, led_on).await;
        }

        // Update OLED every 5 ticks (500ms)
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

            // Line 4: sensor readings — filled in as I2C sensors are added
            oled::print(&mut display, style, &[
                "PicoMate v1",
                line2.as_str(),
                line3.as_str(),
                "",
            ]);
        }
    }
}
