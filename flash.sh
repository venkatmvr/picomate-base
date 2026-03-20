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
  app-ota)
    # Flash ONLY the app partition (0x10009000+) and STATE erase.
    # Run AFTER ./flash.sh bootloader. Does NOT touch 0x10000000-0x10007FFF,
    # so the bootloader code written in the first session is preserved.
    build_app
    elf2uf2-rs "${APP_ELF}" "${APP_UF2}"
    OTA_UF2="target/${TARGET}/release/app-ota.uf2"
    python3 - "${APP_UF2}" "${OTA_UF2}" <<'PYEOF'
import sys, struct
MAGIC1, MAGIC2, MAGIC3 = 0x0A324655, 0x9E5D5157, 0x0AB16F30
UF2_FLAG_FAMILYID = 0x00002000
RP2040_FAMILY_ID  = 0xe48bff56

def make_uf2_block(addr, data256):
    blk = bytearray(512)
    struct.pack_into('<IIIIII', blk, 0, MAGIC1, MAGIC2, UF2_FLAG_FAMILYID, addr, 256, 0)
    struct.pack_into('<I', blk, 24, 0)
    struct.pack_into('<I', blk, 28, RP2040_FAMILY_ID)
    blk[32:32+256] = data256
    struct.pack_into('<I', blk, 508, MAGIC3)
    return blk

def read_uf2(path):
    data = open(path, 'rb').read()
    return [bytearray(data[i:i+512]) for i in range(0, len(data), 512)
            if len(data[i:i+512]) == 512 and struct.unpack_from('<I', data[i:i+512], 0)[0] == MAGIC1]

# Only app blocks (>= 0x10009000) — no BOOT2, no gap-fill
app_blocks = [b for b in read_uf2(sys.argv[1]) if struct.unpack_from('<I', b, 12)[0] >= 0x10009000]
state_blocks = [make_uf2_block(0x10008000 + i*256, b'\xff'*256) for i in range(16)]
blocks = state_blocks + app_blocks
total = len(blocks)
out = bytearray()
for i, blk in enumerate(blocks):
    struct.pack_into('<I', blk, 20, i)
    struct.pack_into('<I', blk, 24, total)
    out += blk
open(sys.argv[2], 'wb').write(out)
print(f"    App-OTA UF2: {total} blocks → {sys.argv[2]}")
PYEOF
    echo "    $(ls -lh "${OTA_UF2}" | awk '{print $5}') (app + STATE only, no bootloader overwrite)"
    wait_for_bootsel
    echo "==> Flashing app-ota image..."
    python3 - "${OTA_UF2}" "${BOOTSEL_VOLUME}" <<'FLASH_PY'
import sys, os
src, dst = sys.argv[1], os.path.join(sys.argv[2], os.path.basename(sys.argv[1]))
size = os.path.getsize(src)
written = 0
with open(src, 'rb') as fin, open(dst, 'wb') as fout:
    while True:
        data = fin.read(65536)
        if not data: break
        fout.write(data)
        written += len(data)
        print(f'\r  {written//1024}K / {size//1024}K ({written*100//size}%)', end='', flush=True)
    fout.flush(); os.fsync(fout.fileno())
print(f'\n  Write complete ({written} bytes).')
FLASH_PY
    for i in $(seq 1 10); do
        sleep 1
        if [ ! -d "${BOOTSEL_VOLUME}" ]; then
            echo "==> Done. Pico W rebooted — app flashed."; break
        fi
        [ "$i" -eq 10 ] && echo "ERROR: volume still mounted." >&2 && exit 1
    done
    ;;
  combined)
    build_all
    echo "==> Creating combined UF2 (bootloader + app)..."
    elf2uf2-rs "${BL_ELF}"  "${BL_UF2}"
    elf2uf2-rs "${APP_ELF}" "${APP_UF2}"
    # Merge UF2 files with correct block numbering.
    # Simple cat doesn't work: each UF2 has its own block count and the Pico
    # reboots when it sees the last block of the first file, before the second
    # file's blocks arrive. We must renumber all blocks as one sequence.
    python3 - "${BL_UF2}" "${APP_UF2}" "${COMBINED_UF2}" <<'PYEOF'
import sys, struct
MAGIC1, MAGIC2, MAGIC3 = 0x0A324655, 0x9E5D5157, 0x0AB16F30
UF2_FLAG_FAMILYID = 0x00002000
RP2040_FAMILY_ID  = 0xe48bff56

def make_uf2_block(addr, data256):
    """Build one 512-byte UF2 block (payload must be exactly 256 bytes)."""
    blk = bytearray(512)
    struct.pack_into('<IIIIII', blk, 0,
        MAGIC1, MAGIC2,
        UF2_FLAG_FAMILYID,  # flags
        addr,               # target address
        256,                # payload size
        0,                  # block number (renumbered later)
    )
    struct.pack_into('<I', blk, 24, 0)              # total blocks (renumbered later)
    struct.pack_into('<I', blk, 28, RP2040_FAMILY_ID)
    blk[32:32+256] = data256
    struct.pack_into('<I', blk, 508, MAGIC3)
    return blk

def read_uf2(path):
    data = open(path, 'rb').read()
    return [bytearray(data[i:i+512]) for i in range(0, len(data), 512)
            if len(data[i:i+512]) == 512 and struct.unpack_from('<I', data[i:i+512], 0)[0] == MAGIC1]

bl_uf2_path, app_uf2_path = sys.argv[1], sys.argv[2]

# Bootloader: only take blocks from the bootloader partition (< 0x10008000).
# Excludes zero-fill gap blocks that elf2uf2-rs emits for the app UF2.
bl_blocks  = [b for b in read_uf2(bl_uf2_path)
              if struct.unpack_from('<I', b, 12)[0] < 0x10008000]

# App: only take blocks from the app partition (>= 0x10009000).
# Excludes the app's BOOT2 block and zero-filled gap blocks.
app_blocks = [b for b in read_uf2(app_uf2_path)
              if struct.unpack_from('<I', b, 12)[0] >= 0x10009000]

# Erase the STATE partition (0x10008000..0x10009000, 4KB) by writing 0xFF.
# Without this, old firmware data left at that address is misread as swap
# control flags, causing the bootloader to corrupt ACTIVE on first boot.
STATE_START = 0x10008000
STATE_SIZE  = 4 * 1024  # 4KB
state_blocks = [
    make_uf2_block(STATE_START + i * 256, b'\xff' * 256)
    for i in range(STATE_SIZE // 256)
]

blocks = bl_blocks + state_blocks + app_blocks

total = len(blocks)
out = bytearray()
for i, blk in enumerate(blocks):
    struct.pack_into('<I', blk, 20, i)
    struct.pack_into('<I', blk, 24, total)
    out += blk
open(sys.argv[-1], 'wb').write(out)
print(f"    Merged {total} blocks (incl. STATE erase) → {sys.argv[-1]}")
PYEOF
    echo "    $(ls -lh "${COMBINED_UF2}" | awk '{print $5}') total"
    wait_for_bootsel
    echo "==> Flashing combined image (${COMBINED_UF2})..."
    python3 - "${COMBINED_UF2}" "${BOOTSEL_VOLUME}" <<'FLASH_PY'
import sys, os, time
src, dst = sys.argv[1], os.path.join(sys.argv[2], os.path.basename(sys.argv[1]))
size = os.path.getsize(src)
written = 0
chunk = 65536
with open(src, 'rb') as fin, open(dst, 'wb') as fout:
    while True:
        data = fin.read(chunk)
        if not data:
            break
        fout.write(data)
        written += len(data)
        pct = written * 100 // size
        print(f'\r  {written//1024}K / {size//1024}K ({pct}%)', end='', flush=True)
    fout.flush()
    os.fsync(fout.fileno())
print(f'\n  Write complete ({written} bytes).')
FLASH_PY
    # Wait for Pico to reboot (volume disappears)
    for i in $(seq 1 10); do
        sleep 1
        if [ ! -d "${BOOTSEL_VOLUME}" ]; then
            echo "==> Done. Pico W rebooted — bootloader + app flashed."
            break
        fi
        if [ "$i" -eq 10 ]; then
            echo "ERROR: volume still mounted after 10s — flash may have failed." >&2; exit 1
        fi
    done
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
