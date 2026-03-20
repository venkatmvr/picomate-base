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
| Peripheral | Chip | Bus | Pins |
|------------|------|-----|------|
| OLED Display | SSD1315 | I2C0 | SDA=GP16, SCL=GP17 |
| Push Button | — | GPIO | GP26 |
| RGB LED | WS2812 | PIO | GP22 |
| Rotary Encoder | — | GPIO | GP6, GP7 |
| Buzzer | — | GPIO | GP15 |
| PIR Motion | AS312 | GPIO | GP18 |
| IMU | LSM6DS3TR-C | I2C1 | SDA=GP4, SCL=GP5 |
| Light Sensor | LTR-381RGB-01 | I2C | GP8, GP9 |
| Magnetometer | MMC5603NJ | I2C | GP12, GP13 |
| Temp/Humidity | SHT30-DIS | I2C | GP24, GP25 |
| Microphone | ZTS6531S | I2C | GP0, GP1 |

### Phase 3: WiFi
- Connect with cyw43 + embassy-net
- Show IP on OLED

### Phase 4: OTA (picowota)
- picowota submodule as bootloader
- Flash partitioned: bootloader | active | DFU
- App calls `picowota_reboot(true)` → enters OTA mode
- Server pushes new `.bin` over HTTP
