/* App memory map — must match bootloader/memory.x partition layout exactly.
 *
 * Flash layout (2MB):
 *   0x10000000  32KB  Bootloader
 *   0x10008000   4KB  STATE  (swap flags)
 *   0x10009000 512KB  ACTIVE (app runs here)
 *   0x10089000 516KB  DFU    (new firmware written here)
 */
MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10009000, LENGTH = 512K
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}

/* Partition symbols — read by embassy-boot BootLoaderConfig / FirmwareUpdaterConfig */
__bootloader_state_start  = 0x10008000;
__bootloader_state_end    = 0x10009000;
__bootloader_active_start = 0x10009000;
__bootloader_active_end   = 0x10009000 + 512K;
__bootloader_dfu_start    = 0x10089000;
__bootloader_dfu_end      = 0x10089000 + 512K + 4K;
