// rgb.rs — WS2812 single-pixel driver via PIO1
//
// Uses PIO1 SM0 with a side-set program.
// Timing: 10 PIO cycles/bit, clock divider = 125MHz/8MHz = 15.625
//   T3=3 low → T1=2 high → T2=5 high(1-bit) or low(0-bit)
//   0-bit: 250ns high, 1000ns low
//   1-bit: 875ns high, 375ns low

use embassy_rp::peripherals::PIO1;
use embassy_rp::pio::{Common, Config, Direction, FifoJoin, PioPin, ShiftConfig, ShiftDirection, StateMachine};
use embassy_rp::pio::program::pio_asm;
use embassy_rp::Peri;
use fixed::types::U24F8;

/// Global brightness 0–255. WS2812 at 255 is very bright; tune this to taste.
const BRIGHTNESS: u8 = 32; // ~12%

pub struct Ws2812<'d> {
    sm: StateMachine<'d, PIO1, 0>,
    _pin: embassy_rp::pio::Pin<'d, PIO1>,
}

impl<'d> Ws2812<'d> {
    pub fn new(
        pio: &mut Common<'d, PIO1>,
        mut sm0: StateMachine<'d, PIO1, 0>,
        pin: Peri<'d, impl PioPin + 'd>,
    ) -> Self {
        let prg = pio_asm!(
            ".side_set 1",
            ".wrap_target",
            "bitloop:",
            "  out x, 1        side 0 [2]",  // T3=3: drive low, load bit into X
            "  jmp !x, do_zero side 1 [1]",  // T1=2: drive high, branch if 0-bit
            "do_one:",
            "  jmp bitloop     side 1 [4]",  // T2=5: keep high for 1-bit
            "do_zero:",
            "  nop             side 0 [4]",  // T2=5: drive low for 0-bit
            ".wrap",
        );

        let mut cfg = Config::default();
        let out_pin = pio.make_pio_pin(pin);
        let loaded = pio.load_program(&prg.program);
        cfg.use_program(&loaded, &[&out_pin]);

        // 800 kHz WS2812 signal: 10 cycles/bit → PIO needs 8 MHz clock
        // divider = 125 MHz / 8 MHz = 15.625
        // U24F8 bit layout: integer_part << 8 | (frac * 256)
        // 15 << 8 | (0.625 * 256 = 160) = 0x0FA0
        cfg.clock_divider = U24F8::from_bits((15u32 << 8) | 160u32);

        cfg.fifo_join = FifoJoin::TxOnly;
        cfg.shift_out = ShiftConfig {
            auto_fill: true,
            threshold: 24,
            direction: ShiftDirection::Left,
        };

        sm0.set_config(&cfg);
        sm0.set_pin_dirs(Direction::Out, &[&out_pin]); // side-set pin must be output
        sm0.set_enable(true);

        Self { sm: sm0, _pin: out_pin }
    }

    /// Send one GRB pixel (WS2812 expects GRB byte order).
    /// Values are scaled by BRIGHTNESS before transmission.
    pub async fn write_color(&mut self, r: u8, g: u8, b: u8) {
        let dim = |v: u8| (v as u16 * BRIGHTNESS as u16 / 255) as u8;
        let word = ((dim(g) as u32) << 24) | ((dim(r) as u32) << 16) | ((dim(b) as u32) << 8);
        self.sm.tx().wait_push(word).await;
    }
}

/// 256-step hue wheel → (r, g, b). Use wrapping_add to cycle.
pub fn wheel(pos: u8) -> (u8, u8, u8) {
    let p = pos as u16;
    match p {
        0..=84   => (255 - p as u8 * 3,             p as u8 * 3, 0),
        85..=169 => { let p = p - 85;  (0, 255 - p as u8 * 3,             p as u8 * 3) }
        _        => { let p = p - 170; (            p as u8 * 3, 0, 255 - p as u8 * 3) }
    }
}
