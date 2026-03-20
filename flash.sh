#!/usr/bin/env bash
# flash.sh — build and flash picomate-base to Pico W via UF2
#
# Usage:
#   ./flash.sh          — build release + wait for BOOTSEL + flash
#   ./flash.sh build    — build only (no flash)
#   ./flash.sh help     — show this message
#
# Requirements:
#   cargo, elf2uf2-rs (cargo install elf2uf2-rs)
#   Pico W must be in BOOTSEL mode (hold BOOTSEL, plug USB) — script will wait

set -euo pipefail

TARGET="thumbv6m-none-eabi"
BIN="picomate-base"
ELF="target/${TARGET}/release/${BIN}"
UF2="target/${TARGET}/release/${BIN}.uf2"
BOOTSEL_VOLUME="/Volumes/RPI-RP2"

usage() {
  grep '^#' "$0" | sed 's/^# \?//'
  exit 0
}

build() {
  echo "==> Building (release)..."
  cargo build --release
  echo ""
  echo "Artifacts:"
  echo "  ELF : ${ELF}"
  if [ -f "${UF2}" ]; then
    echo "  UF2 : ${UF2}"
  fi
  ls -lh "${ELF}" 2>/dev/null || true
}

wait_for_bootsel() {
  if [ -d "${BOOTSEL_VOLUME}" ]; then
    return 0
  fi

  echo ""
  echo "  *** Put Pico W into BOOTSEL mode ***"
  echo "  Hold the BOOTSEL button, plug USB, then release."
  echo "  Waiting for ${BOOTSEL_VOLUME} to appear..."
  echo ""

  local dots=0
  while [ ! -d "${BOOTSEL_VOLUME}" ]; do
    sleep 0.5
    printf "."
    dots=$((dots + 1))
    if [ $((dots % 40)) -eq 0 ]; then
      echo ""
    fi
  done
  echo ""
  echo "  Pico W detected in BOOTSEL mode."
}

flash() {
  build
  wait_for_bootsel
  echo ""
  echo "==> Flashing..."
  elf2uf2-rs -d "${ELF}"
  echo "==> Done. Pico W will reboot into the new firmware."
}

case "${1:-flash}" in
  flash)  flash ;;
  build)  build ;;
  help|-h|--help) usage ;;
  *)
    echo "Unknown command: $1"
    usage
    ;;
esac
