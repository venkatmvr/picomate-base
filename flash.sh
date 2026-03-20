#!/usr/bin/env bash
# flash.sh — PicoMate build + flash + OTA tool
#
# Commands:
#   ./flash.sh                — build app + flash via BOOTSEL (day-to-day dev)
#   ./flash.sh build          — build everything (app + bootloader), no flash
#   ./flash.sh bootloader     — build bootloader + flash via BOOTSEL (one-time setup)
#   ./flash.sh combined       — build both + flash combined image via BOOTSEL (full initial flash)
#   ./flash.sh ota [ip]       — build app + send over WiFi to running device
#                               ip defaults to 192.168.1.180 (last known DHCP address)
#   ./flash.sh help           — show this message
#
# OTA wire format: 4-byte big-endian size header + raw binary (.bin)
# OTA port: 4242
#
# First-time setup:
#   1. ./flash.sh combined    → flashes bootloader + app together
#   2. From then on: ./flash.sh ota  (no USB needed)
#   3. For dev iteration: ./flash.sh  (BOOTSEL, app only — boots directly without bootloader)

set -euo pipefail

TARGET="thumbv6m-none-eabi"
APP_BIN="picomate-base"
BL_BIN="bootloader"
APP_ELF="target/${TARGET}/release/${APP_BIN}"
BL_ELF="target/${TARGET}/release/${BL_BIN}"
BOOTSEL_VOLUME="/Volumes/RPI-RP2"
OTA_IP="${OTA_IP:-192.168.1.180}"
OTA_PORT=4242

# ─────────────────────────────────────────────────────────────────────────────

usage() {
  grep '^#' "$0" | sed 's/^# \?//'
  exit 0
}

build_app() {
  echo "==> Building app (release)..."
  cargo build --release -p picomate-base
  echo "    $(ls -lh "${APP_ELF}" | awk '{print $5, $9}')"
}

build_bootloader() {
  echo "==> Building bootloader (release)..."
  cargo build --release -p bootloader
  echo "    $(ls -lh "${BL_ELF}" | awk '{print $5, $9}')"
}

build_all() {
  build_bootloader
  build_app
}

wait_for_bootsel() {
  if [ -d "${BOOTSEL_VOLUME}" ]; then return 0; fi
  echo ""
  echo "  *** BOOTSEL mode required ***"
  echo "  Hold BOOTSEL button, plug USB, then release."
  echo "  Waiting for ${BOOTSEL_VOLUME}..."
  echo ""
  while [ ! -d "${BOOTSEL_VOLUME}" ]; do sleep 0.5; printf "."; done
  echo ""
  sleep 1   # let macOS finish mounting
  echo "  Pico W detected."
}

flash_elf() {
  local elf="$1"
  wait_for_bootsel
  echo "==> Flashing ${elf}..."
  elf2uf2-rs -d "${elf}"
  echo "==> Done. Pico W rebooting."
}

COMBINED_UF2="target/${TARGET}/release/combined.uf2"
BL_UF2="target/${TARGET}/release/${BL_BIN}.uf2"
APP_UF2="target/${TARGET}/release/${APP_BIN}.uf2"

send_ota() {
  local ip="${1:-${OTA_IP}}"
  local bin="target/${TARGET}/release/${APP_BIN}.bin"

  echo "==> Converting ELF → raw binary..."
  rust-objcopy -O binary "${APP_ELF}" "${bin}"
  local size
  size=$(wc -c < "${bin}" | tr -d ' ')
  echo "    Binary: ${size} bytes"

  echo "==> Sending OTA to ${ip}:${OTA_PORT}..."
  # Send 4-byte big-endian size header followed by the binary
  python3 - "${ip}" "${OTA_PORT}" "${bin}" "${size}" <<'PYEOF'
import sys, socket, struct
ip, port, path, size = sys.argv[1], int(sys.argv[2]), sys.argv[3], int(sys.argv[4])
data = open(path, 'rb').read()
s = socket.socket()
s.settimeout(90)
s.connect((ip, port))
s.sendall(struct.pack('>I', size))
sent = 0
chunk = 4096
while sent < size:
    n = s.send(data[sent:sent+chunk])
    sent += n
    pct = sent * 100 // size
    print(f'\r  {sent}/{size} bytes ({pct}%)', end='', flush=True)
print()
s.close()
print('OTA sent. Device will reboot.')
PYEOF
}

# ─── Commands ────────────────────────────────────────────────────────────────

cmd="${1:-flash}"
shift || true

case "${cmd}" in
  flash)
    build_app
    flash_elf "${APP_ELF}"
    ;;
  build)
    build_all
    ;;
  bootloader)
    build_bootloader
    flash_elf "${BL_ELF}"
    ;;
  combined)
    build_all
    echo "==> Creating combined UF2 (bootloader + app)..."
    elf2uf2-rs "${BL_ELF}"  "${BL_UF2}"
    elf2uf2-rs "${APP_ELF}" "${APP_UF2}"
    cat "${BL_UF2}" "${APP_UF2}" > "${COMBINED_UF2}"
    echo "    $(ls -lh "${COMBINED_UF2}" | awk '{print $5}') → ${COMBINED_UF2}"
    wait_for_bootsel
    echo "==> Flashing combined image..."
    # -X: skip extended attributes — macOS tries to write them after the UF2 is
    # received, but the Pico reboots immediately, making the volume disappear.
    # If the volume is gone after cp exits, the copy succeeded.
    cp -X "${COMBINED_UF2}" "${BOOTSEL_VOLUME}/" 2>/dev/null || {
        if [ ! -d "${BOOTSEL_VOLUME}" ]; then
            echo "==> Done. Pico W rebooted — bootloader + app flashed."
        else
            echo "ERROR: copy failed and volume is still mounted." >&2; exit 1
        fi
    }
    ;;
  ota)
    build_app
    send_ota "${1:-${OTA_IP}}"
    ;;
  help|-h|--help)
    usage
    ;;
  *)
    echo "Unknown command: ${cmd}"
    usage
    ;;
esac
