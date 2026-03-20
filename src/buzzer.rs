// buzzer.rs — Passive buzzer on GP15 via PWM (slice 7, channel B)
//
// Passive buzzer needs a square wave at the resonant frequency to make sound.
// We use PWM at 2 kHz, 50% duty cycle → audible beep.
// Silence = duty 0 (PWM output stays low, no oscillation).
//
// GP15 = PWM slice 7, channel B  (RP2040: GPIO N → slice N/2, ch A if even, B if odd)

use embassy_rp::peripherals::PWM_SLICE7;
use embassy_rp::pwm::{ChannelBPin, Config as PwmConfig, Pwm};
use embassy_rp::Peri;

// 125 MHz sys_clk / 2 kHz target / no divider = 62_500 ticks per period
const TOP: u16 = 62_500;
const HALF: u16 = TOP / 2;

pub struct Buzzer<'d> {
    pwm: Pwm<'d>,
}

impl<'d> Buzzer<'d> {
    pub fn new(
        slice: Peri<'d, PWM_SLICE7>,
        pin: Peri<'d, impl ChannelBPin<PWM_SLICE7> + 'd>,
    ) -> Self {
        let mut cfg = PwmConfig::default();
        cfg.top = TOP;
        cfg.compare_b = 0; // start silent
        Self {
            pwm: Pwm::new_output_b(slice, pin, cfg),
        }
    }

    pub fn on(&mut self) {
        let mut cfg = PwmConfig::default();
        cfg.top = TOP;
        cfg.compare_b = HALF; // 50% duty → 2 kHz square wave
        self.pwm.set_config(&cfg);
    }

    pub fn off(&mut self) {
        let mut cfg = PwmConfig::default();
        cfg.top = TOP;
        cfg.compare_b = 0;
        self.pwm.set_config(&cfg);
    }
}
