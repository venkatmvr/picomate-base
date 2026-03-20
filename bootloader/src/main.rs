// bootloader/src/main.rs — minimal direct-jump bootloader (debug version)
//
// Skips all embassy-boot state machine logic.
// Just reads the vector table at APP_START and jumps directly.
// Use this to verify the bootloader→app jump works before adding OTA back.

#![no_std]
#![no_main]

use {defmt_rtt as _, panic_probe as _, embassy_rp as _};

const APP_START: u32 = 0x10009000;

#[cortex_m_rt::entry]
fn main() -> ! {
    // Do NOT call embassy_rp::init() — it reconfigures clocks and XIP flash
    // timing (SSI), which corrupts reads from 0x10009000 before the app can
    // reinitialize them. BOOT2 already set up safe XIP mode; leave it alone.

    // Direct vector-table jump: read SP and reset handler from app slot.
    unsafe {
        let sp = core::ptr::read_volatile(APP_START as *const u32);
        let rv = core::ptr::read_volatile((APP_START + 4) as *const u32);
        // Set VTOR so the app's interrupt handlers are used.
        (*cortex_m::peripheral::SCB::PTR).vtor.write(APP_START);
        core::arch::asm!(
            "msr msp, {sp}",
            "bx  {rv}",
            sp = in(reg) sp,
            rv = in(reg) rv,
            options(noreturn),
        );
    }
}
