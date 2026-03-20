// button.rs — Momentary button, active-low with internal pull-up.
// Caller constructs Input<'d> and passes it in.

use embassy_rp::gpio::Input;

pub struct Button<'d> {
    pin: Input<'d>,
}

impl<'d> Button<'d> {
    pub fn new(pin: Input<'d>) -> Self {
        Self { pin }
    }

    pub fn is_pressed(&self) -> bool {
        self.pin.is_low()
    }

    pub fn state_str(&self) -> &'static str {
        if self.is_pressed() { "Btn: Pressed" } else { "Btn: Released" }
    }
}
