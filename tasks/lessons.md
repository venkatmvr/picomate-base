# PicoMate Base — Lessons Learned

## OTA Networking: Mac Multi-Interface Routing Issue (2026-03-20)

**Problem**: Mac has both Ethernet (en0, 192.168.1.195) and WiFi (en1, 192.168.1.222) on the
same subnet. macOS routes all 192.168.1.x traffic via en0 (Ethernet) by default since it has a
lower interface number. The Pico W is on WiFi (192.168.1.180). ARP requests go out en0 but the
Pico on WiFi never sees them → "No route to host".

**What we tried**: `sudo route add -host 192.168.1.180 -interface en1` — ARP still didn't
resolve. Routing via gateway (192.168.1.1) was next step but abandoned.

**Decision**: Abandon WiFi OTA for now. The BOOTSEL + `./flash.sh app-ota` flow works
reliably for development iteration. Return to OTA when there's a clear solution (e.g., router
admin access to check client isolation, or switch to a push-from-device model).

**Root cause candidates still unresolved**:
1. macOS multi-interface routing prefers Ethernet over WiFi for same-subnet traffic
2. AP client isolation may block direct WiFi client ↔ client ARP
3. The Pico's CYW43 power management (fixed: `PowerManagementMode::None` added) was one bug

## WatchdogFlash Panic: 30-second timeout exceeds RP2040 max

**Problem**: `WatchdogFlash::start(p.FLASH, p.WATCHDOG, Duration::from_secs(30))` panics
silently (defmt RTT, not OLED). RP2040 watchdog max is `0xFFFFFF / 2` µs ≈ 8.38 seconds.
30 seconds >> 8.38 seconds → panic. Symptom: OLED freezes on "Flash init..." and appears hung.

**Fix**: Replaced `WatchdogFlash` with plain `embassy_rp::flash::Flash`. The watchdog-fed flash
is unnecessary with our direct-jump bootloader (no rollback mechanism). Also removed
`mark_booted()` which was also causing a blocking flash hang.

**Rule**: Never use `Duration::from_secs(N)` with `N > 8` in `WatchdogFlash::start()` on RP2040.

## Bootloader: LLD memory.x search order

**Problem**: LLD searches the current working directory (workspace root) BEFORE `-L` paths for
`INCLUDE memory.x`. Workspace root had `memory.x` targeting the app address (0x10009000), so
the bootloader was linked at the wrong address.

**Fix**: Renamed workspace `memory.x` → `memory-app.x`. Both bootloader and app `build.rs`
copy their respective `memory.x` to `OUT_DIR` and add `cargo:rustc-link-search=OUT_DIR`.

## CYW43 Power Management

**Problem**: CYW43 chip enters power-save sleep mode after WiFi join. Device gets a DHCP IP and
shows it on OLED but does not respond to ARP or any incoming packets.

**Fix**: Call `control.set_power_management(cyw43::PowerManagementMode::None).await` immediately
after `control.init(CLM).await` in wifi.rs.

## Embedded Debug Strategy: OLED before everything

**Rule**: Always init OLED as the VERY FIRST peripheral in `main()`. Print status before each
subsystem init. Any panic that occurs before OLED init shows as blank screen, making it
impossible to distinguish from hardware failure. With OLED first, you can see exactly where the
hang occurs.

## Bootloader: Do NOT call embassy_rp::init()

**Problem**: Calling `embassy_rp::init()` in the bootloader reconfigures XIP flash timing (SSI),
corrupting reads from 0x10009000 before the app can reinitialize them.

**Fix**: Remove `embassy_rp::init()` from bootloader. BOOT2 already set up safe XIP mode.
Add `embassy_rp as _` (without calling init) to pull in the critical section implementation.

## Combined UF2: elf2uf2-rs gap-fill overwrites bootloader

**Problem**: The app ELF spans BOOT2 (0x10000000) to FLASH (0x10009000). `elf2uf2-rs` generates
zero-filled gap blocks for the entire span (0x10000100–0x10000F00), overwriting the bootloader's
vector table and code in a naively merged combined UF2.

**Fix**: In the Python UF2 merger in `flash.sh`, filter source-aware:
- Bootloader blocks: only from bootloader UF2, address < 0x10008000
- App blocks: only from app UF2, address >= 0x10009000
- STATE partition: synthesize 0xFF blocks at 0x10008000

## macOS /Volumes/RPI-RP2 write reliability

**Problem**: `cp combined.uf2 /Volumes/RPI-RP2/` silently drops blocks for large files.
The Pico reboots when it sees the "last block" of the first merged file before all blocks arrive.

**Fix**: Use Python with `fout.flush(); os.fsync(fout.fileno())` and poll for volume
disappearance. For the bootloader specifically, use `elf2uf2-rs -d` (USB HID direct write)
which is the most reliable method.
