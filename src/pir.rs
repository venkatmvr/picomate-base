// pir.rs — PIR motion sensor AS312 on GP18
//
// Output is active-high:
//   HIGH → motion detected (stays high for ~2s after last motion)
//   LOW  → no motion / idle
// Pull::Down used in case output is open-drain (floats low when idle).

use embassy_rp::gpio::Input;

pub struct Pir<'d> {
    pin: Input<'d>,
}

impl<'d> Pir<'d> {
    pub fn new(pin: Input<'d>) -> Self {
        Self { pin }
    }

    pub fn motion_detected(&self) -> bool {
        self.pin.is_high()
    }

    /// Raw pin level — use this on OLED to debug which way the logic goes.
    /// "H" = pin is HIGH, "L" = pin is LOW. Compare against motion/no-motion.
    pub fn raw_str(&self) -> &'static str {
        if self.pin.is_high() { "PIR:H" } else { "PIR:L" }
    }

    pub fn state_str(&self) -> &'static str {
        if self.motion_detected() { "Motion!" } else { "Clear" }
    }
}
