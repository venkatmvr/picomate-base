# PicoMate Base — Task List

## Phase 1: Foundation
- [x] Project scaffold (Cargo.toml, .cargo/config.toml, build.rs, memory.x)
- [x] OLED display module (src/oled.rs)
- [x] main.rs: OLED "Ready" + LED blink + uptime counter
- [x] Build passes (cargo build --release)
- [x] Flash and verify on hardware

## Phase 2: Peripherals (each shown on OLED)
- [x] Button (GP26) — show "Pressed" / "Released"
- [x] WS2812 RGB LED (GP22, via PIO) — cycle colors
- [x] Rotary Encoder (GP6, GP7) — show count ±
- [ ] Buzzer (GP15) — beep on button press
- [~] PIR Motion (AS312, GP18) — GPIO/code correct, stuck LOW; needs hardware verify
      (check power rail, wait 60s warm-up, try slow close-range movement)
- [ ] IMU LSM6DS3TR-C (I2C1, GP4/GP5) — show accel X/Y/Z
- [ ] Light Sensor LTR-381RGB-01 (I2C, GP8/GP9) — show lux
- [ ] Magnetometer MMC5603NJ (I2C, GP12/GP13) — show heading
- [ ] Temp/Humidity SHT30-DIS (I2C, GP24/GP25) — show temp °C + RH%
- [ ] Microphone ZTS6531S (I2C, GP0/GP1) — show sound level

## Phase 2.5: Input Responsiveness (post WiFi+OTA)
- [ ] Drop main tick to 10ms (Level 1 fix) — encoder/button poll at 100Hz
- [ ] PIO-driven quadrature encoder on PIO1 SM1 (shares PIO1 with WS2812 SM0)
      — zero CPU polling, counts every edge in hardware regardless of rotation speed
      — verify PIO1 instruction budget: WS2812(4) + encoder(~8) << 32 slot limit
      — button stays GPIO + interrupt (wait_for_any_edge), no PIO needed

## Phase 3: WiFi
- [ ] Connect to WiFi (cyw43 + embassy-net, DHCP)
- [ ] Show IP address on OLED
- [ ] Verify connectivity (ping from host)

## Phase 4: OTA (picowota)
- [ ] Add picowota as git submodule
- [ ] Configure flash partitions (bootloader | active | DFU)
- [ ] App calls picowota_reboot(true) to enter OTA mode
- [ ] Axum server serves firmware.bin
- [ ] End-to-end OTA test: push new version over WiFi
