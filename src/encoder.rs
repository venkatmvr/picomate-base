// encoder.rs — Quadrature rotary encoder, polled.
// GP6 = CLK (A), GP7 = DT (B).
// Decodes on falling edge of A:
//   B high → CW (+1),  B low → CCW (-1)

use embassy_rp::gpio::Input;

pub struct Encoder<'d> {
    a: Input<'d>,
    b: Input<'d>,
    count: i32,
    last_a: bool,
}

impl<'d> Encoder<'d> {
    pub fn new(pin_a: Input<'d>, pin_b: Input<'d>) -> Self {
        let last_a = pin_a.is_high();
        Self { a: pin_a, b: pin_b, count: 0, last_a }
    }

    /// Call every ~10ms. Returns updated count.
    pub fn update(&mut self) -> i32 {
        let a = self.a.is_high();
        if a != self.last_a {
            if !a {
                // Falling edge on A — sample B for direction
                if self.b.is_high() { self.count += 1; } else { self.count -= 1; }
            }
            self.last_a = a;
        }
        self.count
    }

    #[allow(dead_code)]
    pub fn count(&self) -> i32 {
        self.count
    }
}
