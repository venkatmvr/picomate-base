/* Bootloader memory map — must match app memory.x partition layout exactly.
 *
 * Flash layout (2MB):
 *   0x10000000  32KB  Bootloader (this binary)
 *   0x10008000   4KB  STATE  (swap flags)
 *   0x10009000 512KB  ACTIVE (app jumps here after swap)
 *   0x10089000 516KB  DFU    (staging area)
 */
MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10000100, LENGTH = 32K - 0x100
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}

/* Partition symbols — read by embassy-boot BootLoaderConfig::from_linkerfile_blocking */
__bootloader_state_start  = 0x10008000;
__bootloader_state_end    = 0x10009000;
__bootloader_active_start = 0x10009000;
__bootloader_active_end   = 0x10009000 + 512K;
__bootloader_dfu_start    = 0x10089000;
__bootloader_dfu_end      = 0x10089000 + 512K + 4K;
