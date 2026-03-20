# Architecture: Embassy Rust on Pico W

## 1. The Hardware Context

The Pico W is an **RP2040** microcontroller (dual-core ARM Cortex-M0+) with a **CYW43439** WiFi chip attached via SPI. It has:
- 2 MB flash (external, XIP — execute-in-place)
- 264 KB SRAM
- No OS, no heap by default, no `std`

---

## 2. `memory.x` — Flash/RAM Map

```
MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100      ← 256 bytes
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}
```

The linker uses this to assign your code/data to physical addresses:

| Region | What lives here |
|--------|----------------|
| `BOOT2` | 256-byte second-stage bootloader (provided by `embassy-rp`). Configures flash XIP mode so the chip can read code from flash after reset. |
| `FLASH` | Your compiled binary — `.text` (code), `.rodata` (constants like the CYW43 firmware blobs). |
| `RAM` | Stack, `.bss` (zero-initialized statics), `.data` (initialized statics), and the async executor's task memory. |

Without `memory.x`, the linker doesn't know where to put anything and fails.

---

## 3. `build.rs` — Linker Scripts Injected at Build Time

```rust
println!("cargo:rustc-link-arg-bins=-Tlink.x");    // cortex-m-rt
println!("cargo:rustc-link-arg-bins=-Tlink-rp.x"); // embassy-rp (adds BOOT2)
println!("cargo:rustc-link-arg-bins=-Tdefmt.x");   // defmt logging
```

These aren't your files — they ship inside the crates. `build.rs` tells `rustc` to pass these as linker flags. They wire your `memory.x` regions into a full linker script that:
- Places the BOOT2 binary at `0x10000000`
- Sets up the interrupt vector table
- Runs `cortex_m_rt`'s reset handler which zeroes `.bss`, copies `.data`, then calls `main`

---

## 4. `.cargo/config.toml` — Cross-Compilation Setup

```toml
[build]
target = "thumbv6m-none-eabi"   ← ARM Cortex-M0+, no FPU, bare metal (no OS)

[target.thumbv6m-none-eabi]
runner = "elf2uf2-rs -d"        ← after build, convert ELF → UF2 and flash to Pico
```

`none-eabi` = no operating system, no C stdlib. Your binary IS the OS.
`elf2uf2-rs -d` converts the compiled ELF to a UF2 file and copies it to the Pico's USB mass-storage drive (when held in BOOTSEL mode).

---

## 5. `#![no_std]` and `#![no_main]`

```rust
#![no_std]   // don't link against std (which needs an OS)
#![no_main]  // don't use the C runtime's main() entry point
```

Without `std`, you get no `Vec`, no `String`, no `println!`. The Embassy executor provides the async runtime instead. `cortex-m-rt` provides the reset vector and interrupt table.

---

## 6. The Crate Stack

```
Your Code (src/main.rs)
    │
    ├── embassy-executor    ← async task scheduler (cooperative, no preemption)
    │       Runs tasks in a loop; when a task .awaits, another runs.
    │       On Cortex-M0+, tasks live in static memory (no heap).
    │
    ├── embassy-rp          ← RP2040 HAL (Hardware Abstraction Layer)
    │       Drivers for GPIO, I2C, SPI, PIO, DMA, Flash, UART, etc.
    │       async-aware: embassy_rp::i2c::I2c::write(...).await
    │
    ├── embassy-time        ← Timer abstraction
    │       Timer::after(Duration::from_millis(500)).await
    │       Backed by RP2040's hardware timer.
    │
    ├── cyw43               ← Driver for the CYW43439 WiFi chip
    │       Manages the WiFi state machine. Requires its own async task
    │       (wifi_task) that runs the chip's event loop continuously.
    │
    ├── cyw43-pio           ← Glue: drives the CYW43 SPI bus via RP2040 PIO
    │       PIO = Programmable I/O — a small state machine in hardware
    │       that handles the bit-banging of SPI at high speed.
    │
    ├── ssd1306             ← Display driver for SSD1306/SSD1315 OLED
    │       Speaks I2C or SPI; knows how to push pixels to the display.
    │
    ├── embedded-graphics   ← 2D drawing library (text, shapes, images)
    │       Hardware-agnostic. Draws into a framebuffer; ssd1306 flushes it.
    │
    ├── defmt + defmt-rtt   ← Logging framework
    │       defmt: efficient binary log format (info!, warn!, error!)
    │       defmt-rtt: sends logs over RTT (debug probe) or USB.
    │
    ├── panic-probe         ← On panic, prints the location and halts
    │
    └── static_cell         ← Safe static initialization
            StaticCell<T>: lets you safely init a static mut at runtime once.
            Needed because async tasks need 'static references.
```

---

## 7. How the Code Flows at Runtime

```
Reset
  └─ BOOT2 (256 bytes) configures flash XIP
       └─ cortex-m-rt reset handler
            ├─ zeroes .bss, copies .data to RAM
            └─ calls #[embassy_executor::main] fn main(spawner)
                  │
                  ├─ Init peripherals (embassy_rp::init)
                  ├─ Init I2C → OLED → display "Starting..."
                  ├─ Init PIO → CYW43 SPI → load firmware
                  ├─ spawner.spawn(wifi_task(runner))  ← background task
                  │
                  └─ loop {
                        control.gpio_set(0, true).await  ← yields to wifi_task
                        Timer::after(...).await           ← yields to scheduler
                     }
```

The `spawner` lets you launch concurrent async tasks. The Embassy executor is **cooperative**: a task runs until it hits `.await`, then the scheduler picks the next ready task. No threads, no preemption.

---

## 8. Why `StaticCell`

```rust
static STATE: StaticCell<cyw43::State> = StaticCell::new();
let state = STATE.init(cyw43::State::new());
```

`wifi_task` needs a `'static` reference to the CYW43 state because it lives forever. You can't use a local variable (it'd be dropped). `StaticCell` gives you a safe one-time-init pattern for statics — it panics if you call `.init()` twice.

---

## 9. The PIO / CYW43 Connection

The CYW43439 WiFi chip connects to RP2040 via SPI, but at speeds too high for software bit-banging. The RP2040 has 8 PIO state machines — tiny programmable hardware processors. `cyw43-pio` programs one of them to run the SPI protocol in hardware at the right speed, feeding data to the CYW43 chip. That's why you see `Pio::new(p.PIO0, Irqs)` before WiFi init.

---

## 10. Project File Structure

```
picomate-base/
├── .cargo/config.toml    ← cross-compile target + flash runner
├── build.rs              ← inject linker scripts
├── memory.x              ← flash/RAM layout
├── Cargo.toml            ← crate deps
├── cyw43-firmware/       ← binary blobs for the WiFi chip
│   ├── 43439A0.bin
│   └── 43439A0_clm.bin
├── src/
│   ├── main.rs           ← entry point, task orchestration
│   └── oled.rs           ← display helper (print lines to OLED)
└── tasks/
    └── todo.md           ← phase checklist
```

---

## 11. Peripheral Phase Plan

### Phase 1: Foundation
- OLED shows "PicoMate v1 / Ready"
- Onboard LED blinks (via CYW43 GPIO 0)

### Phase 2: Peripherals (each reading shown on OLED)

> **Note — hardware variant:** The DeskPi PicoMate website lists different GPIO assignments
> than what works on the physical board in testing. The table below shows **verified working pins**.
> Website-listed pins are noted where they differ. If your board matches the website, adjust the
> `p.PIN_N` values in `main.rs` accordingly.

| Peripheral | Chip | Bus | Verified Pin | Website Pin |
|------------|------|-----|-------------|-------------|
| OLED Display | SSD1315 | I2C0 | SDA=GP16, SCL=GP17 | SPI GP7,10,11 |
| Push Button | — | GPIO | GP26 | GP14 |
| RGB LED | WS2812 | PIO1 SM0 | GP22 | GP27 |
| Rotary Encoder | — | GPIO | GP6 (CLK), GP7 (DT) | GP16, GP17 |
| Buzzer | — | PWM7B | GP15 | GP15 ✓ |
| PIR Motion | AS312 | GPIO | GP18 | GP18 ✓ |
| IMU | LSM6DS3TR-C | I2C1 | SDA=GP4, SCL=GP5 | GP4, GP5 ✓ |
| Light Sensor | LTR-381RGB-01 | I2C | GP8, GP9 | GP8, GP9 ✓ |
| Magnetometer | MMC5603NJ | I2C | GP12, GP13 | GP12, GP13 ✓ |
| Temp/Humidity | SHT30-DIS | I2C | GP24, GP25 | GP24, GP25 ✓ |
| Microphone | ZTS6531S | I2C | GP0, GP1 | GP0, GP1 ✓ |

### Phase 3: WiFi
- Connect with cyw43 + embassy-net
- Show IP on OLED

### Phase 4: OTA (picowota)
- picowota submodule as bootloader
- Flash partitioned: bootloader | active | DFU
- App calls `picowota_reboot(true)` → enters OTA mode
- Server pushes new `.bin` over HTTP

---
 one important question i2c is slower and easier but on pico w we have the wy chip for wifi and all spi communications and if
that does all spi communications do we route all pio through spi as its faster, or does embassy toolchain capture all
periperipherals and run all commmunications through spi to the peripherals due to pio and state machines speed

⏺ Great architecture question. Here's the clear answer:

  The CYW43 is NOT a peripheral hub

  The CYW43439 chip is a dedicated WiFi/BT chip. Its SPI connection to the RP2040 is a private link — only for WiFi traffic and
  its own 3 GPIOs (including the onboard LED). It doesn't know or care about your sensors.

  The actual topology

  RP2040
  ├── PIO0 SM0 ──SPI──► CYW43439 (WiFi chip only)
  │
  ├── PIO1 SM0 ─────►  WS2812 RGB (1-wire, 800kHz precise timing)
  │
  ├── I2C0 (GP16/17) ► OLED SSD1315
  ├── I2C1 (GP4/5)  ► IMU LSM6DS3TR-C
  │   (GP8/9)       ► Light sensor LTR-381RGB-01
  │   (GP12/13)     ► Magnetometer MMC5603NJ
  │   (GP24/25)     ► SHT30 temp/humidity
  │   (GP0/1)       ► Microphone ZTS6531S
  │
  └── GPIO ──────────► Button, Encoder, PIR, Buzzer

  Every peripheral talks directly to the RP2040 over its own bus. Embassy-rp maps each bus to the right hardware driver — nothing
  is routed through the CYW43.

  Why PIO for WS2812 (not I2C or SPI)?

  PIO is used when hardware peripherals can't meet the timing:

  ┌─────────────┬────────────────────┬───────────────────────────────────────────────────────────────────────────────────────┐
  │     Bus     │       Speed        │                                       Use case                                        │
  ├─────────────┼────────────────────┼───────────────────────────────────────────────────────────────────────────────────────┤
  │ I2C         │ 100–400 kHz        │ Sensors — slow but only 2 wires, many devices share the bus                           │
  ├─────────────┼────────────────────┼───────────────────────────────────────────────────────────────────────────────────────┤
  │ SPI         │ up to ~62 MHz      │ Fast bulk data (displays, flash)                                                      │
  │ hardware    │                    │                                                                                       │
  ├─────────────┼────────────────────┼───────────────────────────────────────────────────────────────────────────────────────┤
  │ PIO         │ any arbitrary      │ CYW43 SPI (needs specific framing), WS2812 (800kHz precise pulses), encoders at       │
  │             │ timing             │ interrupt speed                                                                       │
  └─────────────┴────────────────────┴───────────────────────────────────────────────────────────────────────────────────────┘

  WS2812 needs bit-perfect 250ns/875ns pulses — I2C and SPI can't produce that protocol. PIO programs a tiny state machine in
  silicon to generate it exactly.

  Why I2C for all the sensors?

  I2C is the right choice here — not slower for sensors because:
  - Sensors update at 10–100 Hz (you'd never saturate even 100kHz I2C)
  - Multiple sensors share 2 wires via addressing — otherwise you'd need 10 SPI chip-selects
  - All those sensor chips only speak I2C (or I2C + SPI, but we use I2C for wiring simplicity)

  The short answer

  Embassy doesn't centralize anything. Each peripheral owns its bus directly. PIO is used where hardware peripherals can't meet
  protocol timing — not as a general speedup.
---

## 12. Lessons Learnt

### L1 — `static_cell` on Cortex-M0+ needs `portable-atomic`
**Problem:** `static_cell` v2 uses `portable-atomic` internally, which requires compare-and-exchange (CAS). Cortex-M0+ has no hardware CAS.
**Fix:** Add `portable-atomic = { version = "1", features = ["critical-section"] }` to `Cargo.toml`. This makes `portable-atomic` emulate CAS using a critical section.
**Don't do:** Add `features = ["critical-section"]` to `static_cell` itself — that feature doesn't exist in v2.

### L2 — `write!` clashes with `defmt::*` glob
**Problem:** `use defmt::*` imports `defmt::write!`, which shadows `core::write!`. Calling `write!(my_string, ...)` resolves to the defmt version and fails with a type mismatch.
**Fix:** Use `core::fmt::write(&mut buf, format_args!("...", val))` instead of the `write!` macro when formatting into a `heapless::String`.
**Don't do:** Import both `defmt::*` and `core::fmt::Write` in the same file if you need `write!` for string formatting.

### L3 — `pio_asm!` is not from `pio-proc` directly
**Problem:** `pio_proc::pio_asm!` from the user-facing `pio-proc` crate doesn't work with embassy-rp 0.9 — version and type mismatches with `pio-core`.
**Fix:** Import the macro that embassy-rp re-exports: `use embassy_rp::pio::program::pio_asm;`. This is how `cyw43-pio` uses it internally.
**Don't do:** Add `pio-proc` or `pio` as direct dependencies — embassy-rp vendors the right versions internally.

### L4 — PIO pins need `Peri<'d, impl PioPin + 'd>` wrapper (embassy-rp 0.9)
**Problem:** `make_pio_pin` expects `Peri<'d, impl PioPin + 'd>`. Passing `impl PioPin` fails because `Peri<'_, PIN_N>` doesn't implement `PioPin` — the inner type does.
**Fix:** Function signatures that receive a pin for PIO use must take `Peri<'d, impl PioPin + 'd>`. Import with `use embassy_rp::Peri;`. At the call site, pass `p.PIN_N` directly — it's already `Peri<'_, PIN_N>`.
**Don't do:** Use `impl PioPin` in a function that calls `make_pio_pin` — it won't accept the peripheral wrapper.

### L5 — PIO state machine needs `mut` to configure
**Problem:** `sm.set_config()` and `sm.set_enable()` take `&mut self`, so the `StateMachine` parameter must be `mut`.
**Fix:** Declare `mut sm0: StateMachine<'d, PIO1, 0>` in the function parameter.

### L6 — PIO1 interrupt must be bound when using two PIO blocks
**Problem:** CYW43 uses PIO0. Adding WS2812 on PIO1 without binding `PIO1_IRQ_0` causes the async `wait_push` to hang (no interrupt to wake the task).
**Fix:** Add `PIO1_IRQ_0 => InterruptHandler<PIO1>` to the `bind_interrupts!` block. Both PIO0 and PIO1 bindings can coexist in the same `Irqs` struct.

### L10 — PIR AS312: always use Pull::Down, not Pull::None
**Problem:** AS312 output is open-drain on some variants. With `Pull::None` the pin floats when idle → reads HIGH randomly → always shows "Motion!" regardless of actual motion.
**Fix:** Use `Pull::Down` so the line is actively pulled LOW when the sensor isn't driving it HIGH.
**Don't do:** Assume push-pull output on PIR sensors — use `Pull::Down` as the safe default for any active-high open-drain sensor.

### L11 — I2C sensors (temp, IMU, light, mag, mic) are not GPIO — they need separate implementation
**Problem:** Adding a GPIO `Input` for a sensor address won't work for I2C devices. Each sensor (SHT30, LSM6DS3, LTR-381, MMC5603, ZTS6531) needs:
  1. A dedicated I2C bus init (`I2c::new_async` or `new_blocking`)
  2. A crate or manual register reads for that sensor's protocol
  3. Slow polling is fine (1–10 Hz) — sensors don't need fast sampling
**How to apply:** Each I2C sensor gets its own `src/<sensor>.rs` module. Share the I2C bus using `embassy_rp::i2c::I2c` with address-based multiplexing (no extra hardware needed — I2C supports multiple devices per bus).

### L9 — PIO side-set pins must be explicitly set to output direction
**Problem:** `make_pio_pin` hands the GPIO to PIO control but leaves direction as input. Side-set writes silently do nothing — the LED stays dark with no error.
**Fix:** After `sm.set_config(&cfg)`, call `sm.set_pin_dirs(Direction::Out, &[&pin])` before `set_enable(true)`. The embassy-rp docs on `use_program` say this explicitly (line 718 of pio/mod.rs).
**Don't do:** Assume `make_pio_pin` or `use_program` sets the direction — it doesn't.

### L8 — `U24F8` clock divider holds the *divider value*, not raw frequencies
**Problem:** `U24F8::from_num(125_000_000u32)` panics — U24F8 (24 integer bits) can only hold up to 16,777,215. Dividing two overflow values produces garbage or a panic, causing a hang with OLED stuck at the last message before the panic.
**Fix:** Set the divider value directly using bit layout: `U24F8::from_bits((integer << 8) | (frac_256))`. For 125 MHz / 8 MHz = 15.625: `U24F8::from_bits((15u32 << 8) | 160u32)`.
**Don't do:** Pass raw MHz frequencies into `from_num` — the field represents the *divider* (range 1–65535), not frequencies.

### L7 — Store the `Pin<'d, PIO>` returned by `make_pio_pin`
**Problem:** `make_pio_pin` returns a `Pin<'d, PIO>` that configures the GPIO for PIO use. If it's dropped at the end of `new()`, it may release the GPIO from PIO control.
**Fix:** Store it in the struct as `_pin: embassy_rp::pio::Pin<'d, PIO1>`. The leading `_` suppresses dead-code warnings while keeping the value alive.
