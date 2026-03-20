# Architecture: Embassy Rust on Pico W

## 1. The Hardware Context

The Pico W is an **RP2040** microcontroller (dual-core ARM Cortex-M0+) with a **CYW43439** WiFi chip attached via SPI. It has:
- 2 MB flash (external, XIP — execute-in-place)
- 264 KB SRAM
- No OS, no heap by default, no `std`

---

## 2. `memory.x` — Flash/RAM Map

**With OTA bootloader (current layout):**

```
Address       Size    Region
0x10000000    32KB    Bootloader  (embassy-boot-rp binary, flashed once)
0x10008000     4KB    STATE       (swap flags — written by bootloader + app)
0x10009000   512KB    ACTIVE      (your running app — FLASH in memory.x)
0x10089000   516KB    DFU         (staging area for incoming OTA firmware)
```

```
MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10009000, LENGTH = 512K   ← app lives here
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}

__bootloader_state_start  = 0x10008000;
__bootloader_state_end    = 0x10009000;
__bootloader_active_start = 0x10009000;
__bootloader_active_end   = 0x10009000 + 512K;
__bootloader_dfu_start    = 0x10089000;
__bootloader_dfu_end      = 0x10089000 + 512K + 4K;
```

The linker uses this to assign your code/data to physical addresses:

| Region | What lives here |
|--------|----------------|
| `BOOT2` | 256-byte second-stage bootloader (from `embassy-rp`). Configures flash XIP so the chip can read code after reset. |
| `FLASH` (ACTIVE) | Your compiled binary — `.text` (code), `.rodata` (constants, CYW43 firmware blobs). |
| `STATE` | 4KB sector holding swap-state flags. The bootloader writes here before and after a DFU swap so a power failure mid-copy is recoverable. |
| `DFU` | New firmware written here over WiFi. Bootloader copies DFU→ACTIVE on next reset if STATE says swap is pending. |
| `RAM` | Stack, `.bss`, `.data`, executor task memory. |

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

### Phase 4: OTA (embassy-boot-rp)
- `bootloader/` crate: embassy-boot-rp BootLoader, WatchdogFlash → jumps to ACTIVE
- App calls `mark_updated()` → reset → bootloader swaps DFU→ACTIVE
- App calls `mark_booted()` on startup → confirms firmware good (enables rollback)
- Host sends `.bin` over TCP:4242 via `./flash.sh ota [ip]`

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

---

## 13. The CYW43 Is Not a Peripheral Hub

The CYW43439 is a **dedicated WiFi chip**. Its SPI link to the RP2040 is private — only for WiFi traffic and its own 3 GPIOs (including the onboard LED). Every other peripheral talks directly to the RP2040 over its own bus.

```
RP2040
├── PIO0 SM0 ──SPI──► CYW43439 (WiFi only)
├── PIO1 SM0 ─────►  WS2812 RGB (800kHz 1-wire, needs PIO timing precision)
├── I2C0 (GP16/17) ► OLED SSD1315
├── I2C1 (GP4/5)   ► IMU, Light, Mag, Temp, Mic sensors (all share bus by I2C address)
└── GPIO ──────────► Button, Encoder, PIR, Buzzer (PWM)
```

**Why PIO for WS2812 and not hardware SPI?**
WS2812 needs bit-perfect 250 ns / 875 ns pulses with a custom 1-wire protocol — hardware SPI can't produce that. PIO programs a tiny state machine in silicon to hit the timing exactly.

**Why I2C for sensors and not SPI?**
Sensors update at 10–100 Hz — even 100 kHz I2C has headroom to spare. More importantly, many sensors share 2 wires via I2C addressing; SPI would need a chip-select pin per device.

**Embassy doesn't centralize anything.** Each bus goes directly to its peripheral. PIO is used only where hardware peripherals can't meet protocol timing.

---

## 14. Why Embassy Over RTOS or Bare-Metal Async

| Approach | What you get | What you build yourself |
|---|---|---|
| FreeRTOS (C) | Thread scheduler, queues | Rust FFI bindings, RP2040 port, WiFi driver |
| Bare async (smoltcp) | TCP stack | Executor, timer driver, all HAL abstractions |
| **Embassy** | Executor, timer, full HAL, CYW43 driver, TCP stack | Your application |

Embassy's `async/await` maps naturally to embedded. A task waiting for a timer or TCP packet **yields the CPU** — no busy-wait, no thread context switch overhead. The compiler generates the state machines at compile time. No heap, no OS.

The result: `wifi.join(ssid, opts).await` just works. You write application code, not infrastructure.

---

## 15. Why embassy-boot-rp and Not picowota

**picowota** (`usedbytes/picowota`) is a C/CMake project. It cannot be a Cargo dependency. Using it from a Rust project would mean:
- Two build systems (CMake + Cargo) with separate toolchains
- The bootloader embeds the full CYW43 WiFi firmware **a second time** (~300 KB wasted)
- Your app must be linked to picowota's C SDK flash layout
- No rollback protection

**embassy-boot-rp** is the Rust-native equivalent — a 30-line binary that does one job: check if there's a pending firmware swap, do it atomically, jump to the app. WiFi lives in your app, not the bootloader. Rollback is built in.

---

## 16. How OTA Works End-to-End

```
NORMAL BOOT
  Power on
    → bootloader (0x10000000): reads STATE — empty → jump to ACTIVE (0x10009000)
    → app starts: mark_booted() confirms this firmware is good
    → OTA task listens on TCP:4242

OTA UPDATE
  Developer: ./flash.sh ota 192.168.1.180
    → builds app → rust-objcopy → raw .bin
    → Python: connect TCP:4242 → send [4-byte size][binary bytes]

  App (ota_task):
    → receives bytes in 4KB chunks → BlockingFirmwareUpdater::write_firmware(offset, chunk)
    → all bytes written → mark_updated() → writes SWAP_PENDING to STATE partition
    → sys_reset()

  Bootloader (next boot):
    → reads STATE = SWAP_PENDING
    → copies DFU → ACTIVE sector by sector (4KB at a time)
    → after each sector: updates STATE with progress marker (power-fail safe)
    → STATE = SWAP_DONE → jump to new ACTIVE app

  New app (first boot after update):
    → mark_booted() → STATE = CONFIRMED
    → if it crashes before mark_booted(): watchdog fires → bootloader sees UNCONFIRMED
      → rolls back: copies old ACTIVE backup → restores previous working firmware
```

**Power-fail safety:** The bootloader's STATE machine uses a progress marker per sector. If power is cut mid-copy, it resumes from where it left off on next boot.

**Rollback:** WatchdogFlash starts an 8s watchdog in the bootloader. Your app has 30s (WatchdogFlash timeout in app) to call `mark_booted()`. A bad update that crashes loops → watchdog → restore.

### flash.sh Commands

```bash
./flash.sh combined   # ONE TIME: BOOTSEL flash bootloader + app together
./flash.sh ota        # EVERY UPDATE: WiFi, no USB — defaults to 192.168.1.180
./flash.sh ota <ip>   # OTA to a specific IP
./flash.sh            # Dev shortcut: BOOTSEL flash app directly (skips bootloader)
./flash.sh build      # Build everything without flashing
./flash.sh bootloader # Re-flash just the bootloader via BOOTSEL
```

---

## 17. Lessons Learnt — WiFi and OTA (L12–L18)

### L12 — `Peri<'static, T>` is what `p.PINx` gives you
**Problem:** Writing module functions that accept bare `PIN_23` type fails — `p.PIN_23` from `embassy_rp::init()` is actually `Peri<'static, PIN_23>`.
**Fix:** Function signatures that receive peripherals must use `Peri<'static, T>` (or `Peri<'d, T>` with the right lifetime). At the call site, pass `p.PIN_23` directly.
**Don't do:** Accept `PIN_23` (the raw ZST) in a function — you can't pass `p.PIN_23` to it.

### L13 — Match embassy-net version to embassy-time version
**Problem:** embassy-net 0.6.0 requires embassy-time 0.4.0; embassy-net 0.8.0 requires embassy-time 0.5.0. Mixing versions causes dependency resolution failures.
**Fix:** Check `embassy-net`'s Cargo.toml for its `embassy-time` version requirement before adding it. For embassy-rp 0.9.0 + embassy-time 0.5.0, use **embassy-net 0.8.0**.

### L14 — `cyw43::ControlError` does not implement `defmt::Format`
**Problem:** `warn!("failed: {:?}", e)` where `e: ControlError` fails to compile — `ControlError` has no `defmt::Format` impl.
**Fix:** Log without the error value: `warn!("WiFi join failed, retrying...")`. Detailed error info is in the variant but not exposed via defmt.

### L15 — `Ipv4Address::octets()` not `as_bytes()`
**Problem:** smoltcp's `Ipv4Address` (re-exported by embassy-net) is a type alias for `core::net::Ipv4Addr`. It has `.octets() -> [u8; 4]`, not `.as_bytes()`.
**Fix:** `let o = ip.octets(); format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3])`.

### L16 — `FirmwareUpdaterConfig::from_linkerfile_blocking` takes `&Mutex<NoopRawMutex, RefCell<Flash>>`
**Problem:** The API doesn't take raw flash or `&mut Flash` — it takes a shared mutex wrapper so partitions can be accessed concurrently.
**Fix:**
```rust
let flash = WatchdogFlash::<FLASH_SIZE>::start(p.FLASH, p.WATCHDOG, timeout);
let mutex = Mutex::<NoopRawMutex, _>::new(RefCell::new(flash));
let config = FirmwareUpdaterConfig::from_linkerfile_blocking(&mutex, &mutex);
```
Both DFU and STATE partitions can share the same mutex because `BlockingPartition` addresses non-overlapping regions.

### L17 — WatchdogFlash / FlashMutex must be created once, outside the accept loop
**Problem:** Creating `WatchdogFlash::start(flash, watchdog, ...)` inside a loop moves `flash` and `watchdog` on the first iteration — the second iteration fails to compile with "use of moved value".
**Fix:** Create `WatchdogFlash` once before the loop and wrap it in a `StaticCell<Mutex>` so both startup (`mark_booted`) and the OTA task can share it.

### L18 — `BootLoaderConfig::from_linkerfile_blocking` needs all 6 linker symbols
**Problem:** Using only `__bootloader_state_*` and `__bootloader_dfu_*` symbols causes a link error — the function also requires `__bootloader_active_start` and `__bootloader_active_end`.
**Fix:** Define all 6 symbols in both `memory.x` (app) and `bootloader/memory.x`:
```
__bootloader_state_start  __bootloader_state_end
__bootloader_active_start __bootloader_active_end
__bootloader_dfu_start    __bootloader_dfu_end
```
