// bootloader/src/main.rs — embassy-boot-rp bootloader for picomate-base
//
// Runs at 0x10000000. Checks STATE partition for a pending DFU swap.
// If pending: copies DFU → ACTIVE atomically (power-fail-safe via state machine).
// Then unconditionally jumps to ACTIVE slot at 0x10009000.
//
// Flash once via BOOTSEL. All subsequent updates arrive over WiFi (OTA).

#![no_std]
#![no_main]

use core::cell::RefCell;
use embassy_boot_rp::{BootLoader, BootLoaderConfig, WatchdogFlash};
use embassy_rp::flash::ERASE_SIZE;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::Duration;
use {defmt_rtt as _, panic_probe as _};

const FLASH_SIZE: usize = 2 * 1024 * 1024;
const APP_START:  u32   = 0x10009000;

#[cortex_m_rt::entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());

    let flash = WatchdogFlash::<FLASH_SIZE>::start(p.FLASH, p.WATCHDOG, Duration::from_secs(8));
    let flash = Mutex::<NoopRawMutex, _>::new(RefCell::new(flash));

    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let bl = BootLoader::<ERASE_SIZE>::prepare(config);

    unsafe { bl.load(APP_START) }
}
