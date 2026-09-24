use alloc::{vec, vec::Vec};
use littlefs_rust::{BlockDevice, Error as FsError, Filesystem, FilesystemOptions};

use super::{Error, Storage};
use crate::platform::esp::FlashPartition;

const MAX_KEY_LEN: usize = 64;
const TEMP_PATH: &str = ".pending";

pub struct LittleFsStorage {
    device: FlashPartition,
    max_file_size: usize,
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

impl LittleFsStorage {
    pub(in crate::platform) fn new(
        mut device: FlashPartition,
        max_file_size: usize,
    ) -> Result<Self, Error> {
        let mut header = [0xff; 12];
        device.read(0, 0, &mut header).map_err(map_error)?;
        let legacy = if &header[..4] == b"MCFS" {
            // Hold the complete configuration in RAM before formatting. Formatting
            // erases the entire partition, then app.conf is immediately restored.
            let config = legacy_config(&device, &header, max_file_size)?;
            Filesystem::format_device_with_options(&mut device, options()).map_err(map_error)?;
            Some(config)
        } else {
            match Filesystem::mount_device_with_options(&device, options()) {
                Ok(_) => {}
                Err(error) => {
                    // Never format a damaged filesystem or unknown format.
                    if error == FsError::Corrupt && is_erased(&device)? {
                        Filesystem::format_device_with_options(&mut device, options())
                            .map_err(map_error)?;
                    } else {
                        return Err(map_error(error));
                    }
                }
            }
            None
        };
        let mut storage = Self {
            device,
            max_file_size,
        };
        if let Some(config) = legacy {
            storage.write_atomic("app.conf", &config)?;
        } else {
            // Mutable mounting repairs any pending move left by a power cut.
            Filesystem::mount_device_mut_with_options(storage.device, options())
                .map_err(map_error)?;
        }
        Ok(storage)
    }
}

impl Storage for LittleFsStorage {
    fn read(&mut self, key: &str, buffer: &mut [u8]) -> Result<usize, Error> {
        validate_key(key)?;
        let fs =
            Filesystem::mount_device_with_options(&self.device, options()).map_err(map_error)?;
        let len = fs.stat(key).map_err(map_error)?.size as usize;
        if len > self.max_file_size {
            return Err(Error::Corrupt);
        }
        if len > buffer.len() {
            return Err(Error::BufferTooSmall);
        }
        fs.read_file_into(key, buffer).map_err(map_error)
    }

    fn write_atomic(&mut self, key: &str, data: &[u8]) -> Result<(), Error> {
        validate_key(key)?;
        if data.len() > self.max_file_size {
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

fn legacy_config(
    device: &FlashPartition,
    header: &[u8; 12],
    max_file_size: usize,
) -> Result<Vec<u8>, Error> {
    let key_len = header[5] as usize;
    let data_len = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
    if header[4] != 1 || key_len != b"app.conf".len() || data_len > max_file_size {
        return Err(Error::Corrupt);
    }
    let mut key = [0; 8];
    device.read(0, 12, &mut key).map_err(map_error)?;
    if &key != b"app.conf" {
        return Err(Error::Corrupt);
    }
    let mut config = vec![0; data_len];
    // The largest legacy record crosses the first erase-block boundary.
    let block_size = device.config().block_size;
    let mut offset = 20;
    let mut remaining = config.as_mut_slice();
    while !remaining.is_empty() {
        let len = remaining.len().min(block_size - offset % block_size);
        device
            .read(
                (offset / block_size) as u32,
                offset % block_size,
                &mut remaining[..len],
            )
            .map_err(map_error)?;
        remaining = &mut remaining[len..];
        offset += len;
    }
    Ok(config)
}

fn is_erased(device: &FlashPartition) -> Result<bool, Error> {
    let geometry = device.config();
    let mut buffer = [0; 256];
    for block in 0..geometry.block_count as u32 {
        for offset in (0..geometry.block_size).step_by(buffer.len()) {
            let len = buffer.len().min(geometry.block_size - offset);
            device
                .read(block, offset, &mut buffer[..len])
                .map_err(map_error)?;
            if buffer[..len].iter().any(|byte| *byte != 0xff) {
                return Ok(false);
            }
        }
    }
    Ok(true)
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
