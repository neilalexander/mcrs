/* FROM https://www.pavelp.cz/posts/eng-nrf52840-rust/#source-code */
MEMORY
{
  /*
     Need to leave space for the SoftDevice
     These values are confirmed working for S140 7.3.0

     They were extracted from the Arduino IDE plugin linked indirectly at https://wiki.seeedstudio.com/XIAO_BLE/
  */
  FLASH (rx)     : ORIGIN = 0x27000, LENGTH = 0xED000 - 0x27000

  /*
     Nothing is linked into the next two regions. With no attributes, the linker never
     picks them for a section by itself

     The filesystem in src/littlefs.rs, which gets its bounds from here. The bootloader
     keeps these pages through firmware updates
  */
  LITTLEFS       : ORIGIN = 0xED000, LENGTH = 0xF4000 - 0xED000

  /* The bootloader and its settings, up to the end of the flash */
  BOOTLOADER     : ORIGIN = 0xF4000, LENGTH = 0x100000 - 0xF4000

  /* SRAM required by Softdevice depend on
   * - Attribute Table Size (Number of Services and Characteristics)
   * - Vendor UUID count
   * - Max ATT MTU
   * - Concurrent connection peripheral + central + secure links
   * - Event Len, HVN queue, Write CMD queue
   */
  RAM (rwx) :  ORIGIN = 0x20006000, LENGTH = 0x20040000 - 0x20006000
}

__littlefs_start = ORIGIN(LITTLEFS);
__littlefs_end = ORIGIN(LITTLEFS) + LENGTH(LITTLEFS);

/* The linker doesn't check that memory regions stay apart */
ASSERT(ORIGIN(FLASH) + LENGTH(FLASH) <= ORIGIN(LITTLEFS), "FLASH overlaps LITTLEFS");
ASSERT(ORIGIN(LITTLEFS) + LENGTH(LITTLEFS) <= ORIGIN(BOOTLOADER), "LITTLEFS overlaps BOOTLOADER");
