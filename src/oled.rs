// oled.rs — OLED display helper for SSD1315 (SSD1306-compatible)
//
// Wraps the ssd1306 + embedded-graphics crates into a simple API:
//   oled::print(&mut display, style, &["line1", "line2"])
//
// The display is 128x64 pixels. FONT_6X10 gives us 4 lines at y=0,16,32,48.

use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyle, MonoTextStyleBuilder},
    pixelcolor::BinaryColor,
    prelude::*,
    text::{Baseline, Text},
};
use ssd1306::prelude::*;

/// The text style used everywhere — 6x10 pixel monospace font, white on black.
pub type Style = MonoTextStyle<'static, BinaryColor>;

pub fn make_style() -> Style {
    MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(BinaryColor::On)
        .build()
}

/// Clear the display and print up to 4 lines of text.
/// Lines are spaced 16px apart (y = 0, 16, 32, 48).
/// Empty strings are skipped (no blank rectangle drawn).
pub fn print<DI, SIZE>(
    display: &mut ssd1306::Ssd1306<DI, SIZE, ssd1306::mode::BufferedGraphicsMode<SIZE>>,
    style: Style,
    lines: &[&str],
) where
    DI: WriteOnlyDataCommand,
    SIZE: DisplaySize,
{
    display.clear(BinaryColor::Off).unwrap();
    for (i, line) in lines.iter().enumerate().take(4) {
        if !line.is_empty() {
            Text::with_baseline(
                line,
                Point::new(0, (i * 16) as i32),
                style,
                Baseline::Top,
            )
            .draw(display)
            .unwrap();
        }
    }
    display.flush().unwrap();
}
