// ota.rs — Over-the-air firmware update via TCP
//
// Protocol (port 4242):
//   1. Connect
//   2. Send 4-byte big-endian firmware size
//   3. Send firmware bytes (raw .bin, not ELF)
//   4. Server writes to DFU, marks updated, resets → bootloader swaps DFU→ACTIVE
//
// Rollback: if new app crashes before mark_booted(), watchdog fires and
// bootloader restores the previous ACTIVE image on next boot.

use core::cell::RefCell;
use defmt::*;
use embassy_boot::BlockingFirmwareUpdater;
use embassy_boot_rp::{AlignedBuffer, FirmwareUpdaterConfig, WatchdogFlash};
use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embassy_rp::flash::ERASE_SIZE;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::Duration;

const FLASH_SIZE: usize = 2 * 1024 * 1024;
const OTA_PORT:   u16   = 4242;

pub type FlashMutex = Mutex<NoopRawMutex, RefCell<WatchdogFlash<'static, FLASH_SIZE>>>;

/// OTA listener task. Accepts one firmware delivery per connection.
/// Shared flash mutex is created in main and passed here for 'static lifetime.
pub async fn listen(stack: Stack<'static>, flash: &'static FlashMutex) -> ! {
    let mut rx_buf = [0u8; 4096];
    let mut tx_buf = [0u8; 256];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(60)));

        info!("OTA: waiting on :{}", OTA_PORT);
        if let Err(e) = socket.accept(OTA_PORT).await {
            warn!("OTA: accept error: {:?}", e);
            continue;
        }
        info!("OTA: client connected");

        // 4-byte big-endian firmware size
        let mut size_buf = [0u8; 4];
        if read_exact(&mut socket, &mut size_buf).await.is_err() {
            warn!("OTA: failed to read size header");
            continue;
        }
        let fw_size = u32::from_be_bytes(size_buf) as usize;
        info!("OTA: receiving {} bytes", fw_size);

        let config = FirmwareUpdaterConfig::from_linkerfile_blocking(flash, flash);
        let mut aligned = AlignedBuffer([0u8; ERASE_SIZE]);
        let mut updater = BlockingFirmwareUpdater::new(config, &mut aligned.0);

        let mut chunk = [0u8; ERASE_SIZE];
        let mut offset = 0usize;
        let mut ok = true;

        while offset < fw_size {
            let to_read = (fw_size - offset).min(ERASE_SIZE);
            if read_exact(&mut socket, &mut chunk[..to_read]).await.is_err() {
                warn!("OTA: read error at offset {}", offset);
                ok = false;
                break;
            }
            if updater.write_firmware(offset, &chunk[..to_read]).is_err() {
                warn!("OTA: flash write error at offset {}", offset);
                ok = false;
                break;
            }
            offset += to_read;
            info!("OTA: {}/{}", offset, fw_size);
        }

        if !ok {
            warn!("OTA: transfer aborted");
            continue;
        }

        if updater.mark_updated().is_err() {
            warn!("OTA: mark_updated failed");
            continue;
        }

        info!("OTA: complete — rebooting into bootloader");
        embassy_time::Timer::after(Duration::from_millis(200)).await;
        cortex_m::peripheral::SCB::sys_reset();
    }
}

async fn read_exact(socket: &mut TcpSocket<'_>, buf: &mut [u8]) -> Result<(), ()> {
    let mut pos = 0;
    while pos < buf.len() {
        match socket.read(&mut buf[pos..]).await {
            Ok(0) => return Err(()),
            Ok(n) => pos += n,
            Err(_) => return Err(()),
        }
    }
    Ok(())
}
