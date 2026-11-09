//! The repeater's files, in a LittleFS filesystem in the nRF52840's own flash.
use alloc::boxed::Box;
use core::cell::RefCell;
use defmt::{Debug2Format, error, info, warn};
use embassy_nrf::nvmc::Nvmc;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use littlefs_rust::{BlockDevice, Config, Error as FsError, Filesystem, FilesystemOptions};
use mcrs_repeater::platform::{Storage, storage::Error};

unsafe extern "C" {
    // The bounds of the `LITTLEFS` region in `memory.x`. Only their addresses
    // mean anything.
    static __littlefs_start: u8;
    static __littlefs_end: u8;
}

const BLOCK_SIZE: usize = Nvmc::ERASE_SIZE;
const MAX_FILE_SIZE: usize = 4096;
const MAX_KEY_LEN: usize = 64;
const TEMP_PATH: &str = ".pending";

/// Keeps the repeater's files in flash.
pub struct NrfStorage {
    device: FlashRegion,
}

fn options() -> FilesystemOptions {
    FilesystemOptions {
        read_size: 1,
        prog_size: 4,
        cache_size: 256,
        lookahead_size: 8,
        block_cycles: Some(500),
        ..FilesystemOptions::default()
    }
}

impl NrfStorage {
    /// Mounts the filesystem, making a new one if the flash doesn't hold one.
    /// If that fails, reading and writing fail too and the repeater runs on its
    /// defaults.
    pub fn new(nvmc: Nvmc<'static>) -> Self {
        let start = &raw const __littlefs_start as u32;
        let end = &raw const __littlefs_end as u32;
        let device = FlashRegion {
            // The repeater runs until the board reboots, so this is never freed.
            nvmc: Box::leak(Box::new(RefCell::new(nvmc))),
            start,
            block_count: (end - start) as usize / BLOCK_SIZE,
        };
        match mount_or_format(device) {
            Ok(()) => info!("LittleFS ready"),
            Err(error) => error!("LittleFS initialization failed: {}", Debug2Format(&error)),
        }
        Self { device }
    }
}

fn mount_or_format(mut device: FlashRegion) -> Result<(), Error> {
    match Filesystem::mount_device_with_options(&device, options()) {
        Ok(_) => {}
        // A new chip has these pages erased, and a board that has run Arduino
        // firmware, such as MeshCore or Meshtastic, has its own filesystem
        // there, in a format this can't read. A damaged filesystem goes too:
        // without one the repeater would change identity at every boot, with no
        // way to fix it remotely.
        Err(FsError::Corrupt) => {
            warn!("LittleFS: no filesystem found, formatting");
            Filesystem::format_device_with_options(&mut device, options()).map_err(map_error)?;
        }
        Err(error) => return Err(map_error(error)),
    }
    // Mutable mounting repairs any pending move left by a power cut.
    Filesystem::mount_device_mut_with_options(device, options()).map_err(map_error)?;
    Ok(())
}

impl Storage for NrfStorage {
    fn read(&mut self, key: &str, buffer: &mut [u8]) -> Result<usize, Error> {
        validate_key(key)?;
        let fs =
            Filesystem::mount_device_with_options(&self.device, options()).map_err(map_error)?;
        let len = fs.stat(key).map_err(map_error)?.size as usize;
        if len > MAX_FILE_SIZE {
            return Err(Error::Corrupt);
        }
        if len > buffer.len() {
            return Err(Error::BufferTooSmall);
        }
        fs.read_file_into(key, buffer).map_err(map_error)
    }

    fn write_atomic(&mut self, key: &str, data: &[u8]) -> Result<(), Error> {
        validate_key(key)?;
        if data.len() > MAX_FILE_SIZE {
            return Err(Error::BufferTooSmall);
        }
        let mut fs =
            Filesystem::mount_device_mut_with_options(self.device, options()).map_err(map_error)?;
        // Remove abandoned staging data; the destination remains intact.
        match fs.remove_file(TEMP_PATH) {
            Ok(()) | Err(FsError::NotFound) => {}
            Err(error) => return Err(map_error(error)),
        }
        fs.write_file(TEMP_PATH, data).map_err(map_error)?;
        fs.sync().map_err(map_error)?;
        fs.rename_file(TEMP_PATH, key).map_err(map_error)?;
        fs.sync().map_err(map_error)
    }
}

/// The `LITTLEFS` region of flash. LittleFS takes the device by value to write
/// to it, so this is a handle that can be copied.
///
/// Writes go straight to the NVMC, which will need changing if we ever use
/// SoftDevice.
#[derive(Clone, Copy)]
struct FlashRegion {
    nvmc: &'static RefCell<Nvmc<'static>>,
    start: u32,
    block_count: usize,
}

impl FlashRegion {
    /// The address of `len` bytes at `offset` in `block`, if they're all in it.
    fn address(&self, block: u32, offset: usize, len: usize) -> littlefs_rust::Result<u32> {
        let in_block = offset.checked_add(len).is_some_and(|end| end <= BLOCK_SIZE);
        if block as usize >= self.block_count || !in_block {
            return Err(FsError::OutOfBounds);
        }
        Ok(self.start + block * BLOCK_SIZE as u32 + offset as u32)
    }
}

impl BlockDevice for FlashRegion {
    fn config(&self) -> Config {
        Config {
            block_size: BLOCK_SIZE,
            block_count: self.block_count,
        }
    }

    fn read(&self, block: u32, offset: usize, buffer: &mut [u8]) -> littlefs_rust::Result<()> {
        self.nvmc
            .borrow_mut()
            .read(self.address(block, offset, buffer.len())?, buffer)
            .map_err(|_| FsError::Io)
    }

    fn prog(&mut self, block: u32, offset: usize, data: &[u8]) -> littlefs_rust::Result<()> {
        self.nvmc
            .borrow_mut()
            .write(self.address(block, offset, data.len())?, data)
            .map_err(|_| FsError::Io)
    }

    fn erase(&mut self, block: u32) -> littlefs_rust::Result<()> {
        let from = self.address(block, 0, BLOCK_SIZE)?;
        self.nvmc
            .borrow_mut()
            .erase(from, from + BLOCK_SIZE as u32)
            .map_err(|_| FsError::Io)
    }
    // The NVMC finishes each operation before returning, so sync is a no-op.
}

fn validate_key(key: &str) -> Result<(), Error> {
    if key.is_empty()
        || key.len() > MAX_KEY_LEN
        || matches!(key, "." | ".." | ".pending")
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(Error::InvalidKey);
    }
    Ok(())
}

fn map_error(error: FsError) -> Error {
    match error {
        FsError::NotFound => Error::NotFound,
        FsError::NoSpace | FsError::FileTooLarge => Error::BufferTooSmall,
        FsError::Corrupt | FsError::Unsupported | FsError::Utf8 => Error::Corrupt,
        FsError::InvalidPath | FsError::NameTooLong => Error::InvalidKey,
        _ => Error::Io,
    }
}
