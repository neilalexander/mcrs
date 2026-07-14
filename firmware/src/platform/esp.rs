use core::{
    alloc::{GlobalAlloc, Layout},
    cell::{Cell, RefCell},
    fmt,
    hint::spin_loop,
    mem::MaybeUninit,
    ptr,
};

use alloc::vec;
use critical_section::Mutex;
use embassy_time::{Duration as EmbassyDuration, Timer};
use embedded_alloc::LlffHeap as Heap;
use embedded_hal::delay::DelayNs as BlockingDelayNs;
use embedded_hal_async::delay::DelayNs;
use esp_bootloader_esp_idf::{
    ota_updater::OtaUpdater,
    partitions::{AppPartitionSubType, PartitionType},
};
use esp_hal::{clock::CpuClock, peripherals::Peripherals, rtc_cntl::Rtc, time::Instant};
use esp_println::{print, println};

use esp_backtrace as _;

// esp-hal 1.0.0-rc.0's linker script keeps the ESP-IDF app descriptor in
// .rodata_desc. esp-bootloader-esp-idf 0.5 emits .flash.appdesc by default,
// which the bootloader can miss and then misread unrelated bytes as metadata.
#[unsafe(export_name = "esp_app_desc")]
#[unsafe(link_section = ".rodata_desc")]
#[used]
pub static ESP_APP_DESC: esp_bootloader_esp_idf::EspAppDesc =
    esp_bootloader_esp_idf::EspAppDesc::new_internal(
        env!("MESHCORE_FIRMWARE_VERSION"),
        env!("CARGO_PKG_NAME"),
        esp_bootloader_esp_idf::BUILD_TIME,
        esp_bootloader_esp_idf::BUILD_DATE,
        esp_bootloader_esp_idf::ESP_IDF_COMPATIBLE_VERSION,
        0,
        u16::MAX,
        esp_bootloader_esp_idf::MMU_PAGE_SIZE,
        esp_bootloader_esp_idf::SECURE_VERSION,
    );

const HEAP_SIZE: usize = crate::board::MEMORY_PROFILE.heap_size;
const WIFI_ALLOC_HEADER_SIZE: usize = core::mem::size_of::<usize>();
const WIFI_ALLOC_ALIGN: usize = core::mem::align_of::<usize>();
const FLASH_SECTOR_SIZE: usize = 4096;
const OTA_WRITE_WORDS: usize = 256;
const ESP_IMAGE_MAGIC: u8 = 0xe9;
const ESP_IMAGE_HEADER_LEN: usize = 24;
const ESP_SEGMENT_HEADER_LEN: usize = 8;
const ESP_IMAGE_CHECKSUM_SEED: u8 = 0xef;
const ESP_APP_DESC_OFFSET: u32 = 0x20;
const ESP_APP_DESC_MAGIC: u32 = 0xabcd_5432;
const STORAGE_MAGIC: [u8; 4] = *b"MCFS";
const STORAGE_VERSION: u8 = 1;
const STORAGE_HEADER_LEN: usize = 12;
const STORAGE_MAX_KEY_LEN: usize = 64;
const WALL_CLOCK_RTC_MAGIC: u32 = 0x4d435254;
const WALL_CLOCK_RTC_CHECK: u32 = 0xa5a5_5a5a;

static WALL_CLOCK_OFFSET_SECONDS: Mutex<Cell<u32>> = Mutex::new(Cell::new(0));
static RTC_CLOCK: Mutex<RefCell<Option<Rtc<'static>>>> = Mutex::new(RefCell::new(None));

#[global_allocator]
static HEAP: Heap = Heap::empty();

pub struct Platform {
    pub peripherals: Peripherals,
}

pub fn init() -> Platform {
    init_heap();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::_80MHz);
    let peripherals = esp_hal::init(config);
    init_rtc_clock();

    log_starting();

    Platform { peripherals }
}

pub fn init_storage(layout: crate::platform::storage::Layout) -> EspStorage {
    let mut flash = RomFlash;
    let mut partition_table = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    let inner =
        esp_bootloader_esp_idf::partitions::read_partition_table(&mut flash, &mut partition_table)
            .ok()
            .and_then(|table| {
                table
                    .iter()
                    .find(|partition| partition.label_as_str() == layout.partition_label)
            })
            .and_then(|partition| {
                if partition.is_read_only() || partition.len() < layout.partition_size as u32 {
                    None
                } else {
                    Some(PartitionFileStorage {
                        layout,
                        offset: partition.offset(),
                        size: partition.len() as usize,
                    })
                }
            });

    match inner {
        Some(storage) => {
            log_fmt(format_args!(
                "Storage ready: partition={} offset=0x{:x} size={} max_file_size={}",
                layout.partition_label, storage.offset, storage.size, layout.max_file_size
            ));
            EspStorage {
                inner: Some(storage),
            }
        }
        None => {
            log_fmt(format_args!(
                "Storage unavailable: partition={} partition_size={} max_file_size={}",
                layout.partition_label, layout.partition_size, layout.max_file_size
            ));
            EspStorage { inner: None }
        }
    }
}

pub fn ota_status() -> OtaStatus {
    let mut flash = RomFlash;
    let mut partition_table = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    let Ok(mut updater) = OtaUpdater::new(&mut flash, &mut partition_table) else {
        return OtaStatus {
            available: false,
            selected: "unavailable",
            next: None,
            next_size: 0,
        };
    };

    let selected = updater
        .selected_partition()
        .map(app_partition_name)
        .unwrap_or("unknown");
    let (next, next_size) = updater
        .next_partition()
        .map(|(partition, slot)| (Some(app_partition_name(slot)), partition.partition_size()))
        .unwrap_or((None, 0));

    OtaStatus {
        available: true,
        selected,
        next,
        next_size,
    }
}

pub fn begin_ota_update() -> Result<OtaUpdate, OtaError> {
    let mut flash = RomFlash;
    let mut partition_table = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    let mut updater =
        OtaUpdater::new(&mut flash, &mut partition_table).map_err(|_| OtaError::NotAvailable)?;
    let (_partition, slot) = updater
        .next_partition()
        .map_err(|_| OtaError::NotAvailable)?;
    let (offset, capacity) = find_app_partition(slot).ok_or(OtaError::NotAvailable)?;
    log_fmt(format_args!(
        "OTA: writing {} at offset=0x{:x} size={}",
        app_partition_name(slot),
        offset,
        capacity
    ));
    Ok(OtaUpdate {
        slot,
        offset,
        capacity,
        written: 0,
        flushed: 0,
        erased: 0,
        tail: [0xff; 4],
        tail_len: 0,
        saw_header: false,
    })
}

pub fn write_ota_update(update: &mut OtaUpdate, data: &[u8]) -> Result<(), OtaError> {
    update.write(data)
}

pub fn finish_ota_update(mut update: OtaUpdate) -> Result<(), OtaError> {
    update.finish()?;
    let mut flash = RomFlash;
    let mut partition_table = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    let mut updater =
        OtaUpdater::new(&mut flash, &mut partition_table).map_err(|_| OtaError::NotAvailable)?;
    updater
        .ota_data()
        .and_then(|mut ota| ota.set_current_app_partition(update.slot))
        .map_err(|_| OtaError::Storage)?;
    let selected = updater
        .selected_partition()
        .map_err(|_| OtaError::Storage)?;
    if selected != update.slot {
        log_fmt(format_args!(
            "OTA: activation readback mismatch: wanted={} selected={}",
            app_partition_name(update.slot),
            app_partition_name(selected)
        ));
        return Err(OtaError::Storage);
    }
    log_fmt(format_args!(
        "OTA: activated {} and verified selection, reboot to apply",
        app_partition_name(selected)
    ));
    Ok(())
}

impl OtaUpdate {
    fn write(&mut self, mut data: &[u8]) -> Result<(), OtaError> {
        if data.is_empty() {
            return Ok(());
        }
        if self.written == 0 {
            if data[0] != ESP_IMAGE_MAGIC {
                return Err(OtaError::InvalidImage);
            }
            self.saw_header = true;
        }
        if self.written.saturating_add(data.len()) > self.capacity {
            return Err(OtaError::TooLarge);
        }

        while self.tail_len > 0 && !data.is_empty() {
            self.tail[self.tail_len] = data[0];
            self.tail_len += 1;
            self.written += 1;
            data = &data[1..];
            if self.tail_len == self.tail.len() {
                self.write_aligned_tail()?;
                self.flushed += self.tail.len();
                self.tail = [0xff; 4];
                self.tail_len = 0;
            }
        }

        let aligned_len = data.len() & !0x03;
        if aligned_len > 0 {
            self.ensure_erased(self.flushed + aligned_len)?;
            write_ota_aligned(self.offset + self.flushed as u32, &data[..aligned_len])?;
            self.flushed += aligned_len;
            self.written += aligned_len;
            data = &data[aligned_len..];
        }

        for byte in data {
            self.tail[self.tail_len] = *byte;
            self.tail_len += 1;
            self.written += 1;
        }

        Ok(())
    }

    fn finish(&mut self) -> Result<(), OtaError> {
        if !self.saw_header || self.written == 0 {
            return Err(OtaError::InvalidImage);
        }
        if self.tail_len > 0 {
            if self.flushed.saturating_add(self.tail.len()) > self.capacity {
                return Err(OtaError::TooLarge);
            }
            self.write_aligned_tail()?;
            self.flushed += self.tail.len();
            self.tail = [0xff; 4];
            self.tail_len = 0;
        }
        self.verify_image()
    }

    fn verify_image(&self) -> Result<(), OtaError> {
        let mut header = [0u8; ESP_IMAGE_HEADER_LEN];
        read_flash(self.offset, &mut header)?;
        let segment_count = header[1] as usize;
        if header[0] != ESP_IMAGE_MAGIC || segment_count == 0 || segment_count > 16 {
            return Err(OtaError::InvalidImage);
        }

        let mut app_desc_magic = [0u8; 4];
        read_flash(self.offset + ESP_APP_DESC_OFFSET, &mut app_desc_magic)?;
        if u32::from_le_bytes(app_desc_magic) != ESP_APP_DESC_MAGIC {
            return Err(OtaError::InvalidImage);
        }

        let mut position = ESP_IMAGE_HEADER_LEN;
        let mut checksum = ESP_IMAGE_CHECKSUM_SEED;
        let mut buffer = [0u8; 1024];
        for _ in 0..segment_count {
            let header_end = position
                .checked_add(ESP_SEGMENT_HEADER_LEN)
                .ok_or(OtaError::InvalidImage)?;
            if header_end > self.written {
                return Err(OtaError::InvalidImage);
            }
            let mut segment_header = [0u8; ESP_SEGMENT_HEADER_LEN];
            read_flash(self.offset + position as u32, &mut segment_header)?;
            let segment_len = u32::from_le_bytes([
                segment_header[4],
                segment_header[5],
                segment_header[6],
                segment_header[7],
            ]) as usize;
            position = header_end;
            let segment_end = position
                .checked_add(segment_len)
                .ok_or(OtaError::InvalidImage)?;
            if segment_end > self.written {
                return Err(OtaError::InvalidImage);
            }
            while position < segment_end {
                let count = (segment_end - position).min(buffer.len());
                read_flash(self.offset + position as u32, &mut buffer[..count])?;
                for byte in &buffer[..count] {
                    checksum ^= byte;
                }
                position += count;
            }
        }

        let checksum_offset =
            align_up(position.checked_add(1).ok_or(OtaError::InvalidImage)?, 16) - 1;
        if checksum_offset >= self.written {
            return Err(OtaError::InvalidImage);
        }
        let mut stored_checksum = [0u8; 1];
        read_flash(self.offset + checksum_offset as u32, &mut stored_checksum)?;
        if stored_checksum[0] != checksum {
            return Err(OtaError::InvalidImage);
        }
        Ok(())
    }

    fn write_aligned_tail(&mut self) -> Result<(), OtaError> {
        self.ensure_erased(self.flushed + self.tail.len())?;
        write_ota_aligned(self.offset + self.flushed as u32, &self.tail)?;
        Ok(())
    }

    fn ensure_erased(&mut self, end: usize) -> Result<(), OtaError> {
        while self.erased < end {
            erase_flash_sector(self.offset + self.erased as u32)?;
            self.erased += FLASH_SECTOR_SIZE;
        }
        Ok(())
    }
}

pub struct EspStorage {
    inner: Option<PartitionFileStorage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtaStatus {
    pub available: bool,
    pub selected: &'static str,
    pub next: Option<&'static str>,
    pub next_size: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtaError {
    NotAvailable,
    Storage,
    TooLarge,
    InvalidImage,
}

impl From<crate::platform::storage::Error> for OtaError {
    fn from(_error: crate::platform::storage::Error) -> Self {
        Self::Storage
    }
}

pub struct OtaUpdate {
    slot: AppPartitionSubType,
    offset: u32,
    capacity: usize,
    written: usize,
    flushed: usize,
    erased: usize,
    tail: [u8; 4],
    tail_len: usize,
    saw_header: bool,
}

impl crate::platform::storage::Storage for EspStorage {
    fn read(
        &mut self,
        key: &str,
        buffer: &mut [u8],
    ) -> Result<usize, crate::platform::storage::Error> {
        self.inner
            .as_mut()
            .ok_or(crate::platform::storage::Error::NotAvailable)?
            .read(key, buffer)
    }

    fn write_atomic(
        &mut self,
        key: &str,
        data: &[u8],
    ) -> Result<(), crate::platform::storage::Error> {
        self.inner
            .as_mut()
            .ok_or(crate::platform::storage::Error::NotAvailable)?
            .write_atomic(key, data)
    }
}

impl embedded_storage::nor_flash::NorFlashError for crate::platform::storage::Error {
    fn kind(&self) -> embedded_storage::nor_flash::NorFlashErrorKind {
        match self {
            crate::platform::storage::Error::BufferTooSmall => {
                embedded_storage::nor_flash::NorFlashErrorKind::OutOfBounds
            }
            _ => embedded_storage::nor_flash::NorFlashErrorKind::Other,
        }
    }
}

struct PartitionFileStorage {
    layout: crate::platform::storage::Layout,
    offset: u32,
    size: usize,
}

impl PartitionFileStorage {
    fn read(
        &mut self,
        key: &str,
        buffer: &mut [u8],
    ) -> Result<usize, crate::platform::storage::Error> {
        validate_key(key)?;

        let mut header = [0u8; STORAGE_HEADER_LEN];
        read_flash(self.offset, &mut header)?;

        if header.iter().all(|byte| *byte == 0xff) {
            return Err(crate::platform::storage::Error::NotFound);
        }

        let record = RecordHeader::decode(&header)?;
        if record.data_len > self.layout.max_file_size {
            return Err(crate::platform::storage::Error::Corrupt);
        }

        let record_len = STORAGE_HEADER_LEN + record.key_len + record.data_len;
        if record_len > self.size {
            return Err(crate::platform::storage::Error::Corrupt);
        }

        let mut record_buffer = vec![0u8; record_len];
        read_flash(self.offset, &mut record_buffer)?;

        let key_end = STORAGE_HEADER_LEN + record.key_len;
        if &record_buffer[STORAGE_HEADER_LEN..key_end] != key.as_bytes() {
            return Err(crate::platform::storage::Error::NotFound);
        }

        if buffer.len() < record.data_len {
            return Err(crate::platform::storage::Error::BufferTooSmall);
        }

        let data = &record_buffer[key_end..key_end + record.data_len];
        buffer[..data.len()].copy_from_slice(data);
        Ok(data.len())
    }

    fn write_atomic(
        &mut self,
        key: &str,
        data: &[u8],
    ) -> Result<(), crate::platform::storage::Error> {
        validate_key(key)?;

        if data.len() > self.layout.max_file_size {
            return Err(crate::platform::storage::Error::BufferTooSmall);
        }

        let record_len = STORAGE_HEADER_LEN + key.len() + data.len();
        if record_len > self.size {
            return Err(crate::platform::storage::Error::BufferTooSmall);
        }

        let padded_len = align_up(record_len, 4);
        let mut words = vec![u32::MAX; padded_len / 4];
        let record = words_as_bytes_mut(&mut words);
        record[..4].copy_from_slice(&STORAGE_MAGIC);
        record[4] = STORAGE_VERSION;
        record[5] = key.len() as u8;
        record[8..12].copy_from_slice(&(data.len() as u32).to_le_bytes());

        let key_end = STORAGE_HEADER_LEN + key.len();
        record[STORAGE_HEADER_LEN..key_end].copy_from_slice(key.as_bytes());
        record[key_end..key_end + data.len()].copy_from_slice(data);

        erase_flash_range(self.offset, padded_len)?;
        write_flash_words(self.offset, &words)
    }
}

struct RecordHeader {
    key_len: usize,
    data_len: usize,
}

impl RecordHeader {
    fn decode(header: &[u8; STORAGE_HEADER_LEN]) -> Result<Self, crate::platform::storage::Error> {
        if header[..4] != STORAGE_MAGIC || header[4] != STORAGE_VERSION {
            return Err(crate::platform::storage::Error::Corrupt);
        }

        let key_len = header[5] as usize;
        if key_len == 0 || key_len > STORAGE_MAX_KEY_LEN {
            return Err(crate::platform::storage::Error::Corrupt);
        }

        let data_len = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as usize;
        Ok(Self { key_len, data_len })
    }
}

struct RomFlash;

impl embedded_storage::ReadStorage for RomFlash {
    type Error = crate::platform::storage::Error;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        read_flash(offset, bytes)
    }

    fn capacity(&self) -> usize {
        usize::MAX
    }
}

impl embedded_storage::Storage for RomFlash {
    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        let padded_len = align_up(bytes.len(), 4);
        let mut words = vec![u32::MAX; padded_len / 4];
        words_as_bytes_mut(&mut words)[..bytes.len()].copy_from_slice(bytes);
        erase_flash_range(offset, padded_len)?;
        write_flash_words(offset, &words)
    }
}

impl embedded_storage::nor_flash::ErrorType for RomFlash {
    type Error = crate::platform::storage::Error;
}

impl embedded_storage::nor_flash::ReadNorFlash for RomFlash {
    const READ_SIZE: usize = 1;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        read_flash(offset, bytes)
    }

    fn capacity(&self) -> usize {
        8 * 1024 * 1024
    }
}

impl embedded_storage::nor_flash::NorFlash for RomFlash {
    const WRITE_SIZE: usize = 4;
    const ERASE_SIZE: usize = FLASH_SECTOR_SIZE;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        if from > to {
            return Err(crate::platform::storage::Error::Corrupt);
        }
        erase_flash_range(from, to.saturating_sub(from) as usize)
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        if !bytes.len().is_multiple_of(4) {
            return Err(crate::platform::storage::Error::BufferTooSmall);
        }
        write_ota_aligned(offset, bytes)
    }
}

impl embedded_storage::nor_flash::MultiwriteNorFlash for RomFlash {}

fn validate_key(key: &str) -> Result<(), crate::platform::storage::Error> {
    if key.is_empty() || key.len() > STORAGE_MAX_KEY_LEN {
        return Err(crate::platform::storage::Error::InvalidKey);
    }

    if key
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Ok(())
    } else {
        Err(crate::platform::storage::Error::InvalidKey)
    }
}

fn read_flash(offset: u32, bytes: &mut [u8]) -> Result<(), crate::platform::storage::Error> {
    let aligned_offset = offset & !3;
    let prefix_len = (offset - aligned_offset) as usize;
    let aligned_len = align_up(prefix_len + bytes.len(), 4);
    let mut words = vec![0u32; aligned_len / 4];

    let result = unsafe {
        esp_rom_sys::rom::spiflash::esp_rom_spiflash_read(
            aligned_offset,
            words.as_mut_ptr().cast_const(),
            aligned_len as u32,
        )
    };
    if result != esp_rom_sys::rom::spiflash::ESP_ROM_SPIFLASH_RESULT_OK {
        return Err(crate::platform::storage::Error::Io);
    }

    let aligned_bytes = words_as_bytes(&words);
    bytes.copy_from_slice(&aligned_bytes[prefix_len..prefix_len + bytes.len()]);
    Ok(())
}

#[esp_hal::ram]
fn write_flash_words(offset: u32, words: &[u32]) -> Result<(), crate::platform::storage::Error> {
    if !offset.is_multiple_of(4) {
        return Err(crate::platform::storage::Error::Io);
    }

    let result = critical_section::with(|_| unsafe {
        esp_rom_sys::rom::spiflash::esp_rom_spiflash_write(
            offset,
            words.as_ptr(),
            (words.len() * 4) as u32,
        )
    });
    if result == esp_rom_sys::rom::spiflash::ESP_ROM_SPIFLASH_RESULT_OK {
        Ok(())
    } else {
        Err(crate::platform::storage::Error::Io)
    }
}

fn write_ota_aligned(offset: u32, bytes: &[u8]) -> Result<(), crate::platform::storage::Error> {
    if !bytes.len().is_multiple_of(4) {
        return Err(crate::platform::storage::Error::BufferTooSmall);
    }

    let mut offset = offset;
    let mut bytes = bytes;
    let mut words = [0u32; OTA_WRITE_WORDS];
    while !bytes.is_empty() {
        let word_count = (bytes.len() / 4).min(words.len());
        for (word, chunk) in words.iter_mut().zip(bytes.chunks_exact(4)).take(word_count) {
            *word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        write_flash_words(offset, &words[..word_count])?;
        let written = word_count * 4;
        offset = offset.saturating_add(written as u32);
        bytes = &bytes[written..];
    }

    Ok(())
}

fn erase_flash_range(offset: u32, len: usize) -> Result<(), crate::platform::storage::Error> {
    if len == 0 {
        return Ok(());
    }

    let first_sector = offset as usize / FLASH_SECTOR_SIZE;
    let last_sector = (offset as usize + len - 1) / FLASH_SECTOR_SIZE;
    for sector in first_sector..=last_sector {
        erase_flash_sector((sector * FLASH_SECTOR_SIZE) as u32)?;
    }

    Ok(())
}

#[esp_hal::ram]
fn erase_flash_sector(offset: u32) -> Result<(), crate::platform::storage::Error> {
    if !(offset as usize).is_multiple_of(FLASH_SECTOR_SIZE) {
        return Err(crate::platform::storage::Error::Io);
    }

    let result = critical_section::with(|_| unsafe {
        esp_rom_sys::rom::spiflash::esp_rom_spiflash_erase_sector(offset / FLASH_SECTOR_SIZE as u32)
    });
    if result == esp_rom_sys::rom::spiflash::ESP_ROM_SPIFLASH_RESULT_OK {
        Ok(())
    } else {
        Err(crate::platform::storage::Error::Io)
    }
}

fn find_app_partition(slot: AppPartitionSubType) -> Option<(u32, usize)> {
    let mut flash = RomFlash;
    let mut partition_table = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    let table =
        esp_bootloader_esp_idf::partitions::read_partition_table(&mut flash, &mut partition_table)
            .ok()?;
    table
        .iter()
        .find(|partition| partition.partition_type() == PartitionType::App(slot))
        .map(|partition| (partition.offset(), partition.len() as usize))
}

fn app_partition_name(slot: AppPartitionSubType) -> &'static str {
    match slot {
        AppPartitionSubType::Factory => "factory",
        AppPartitionSubType::Test => "test",
        AppPartitionSubType::Ota0 => "ota_0",
        AppPartitionSubType::Ota1 => "ota_1",
        AppPartitionSubType::Ota2 => "ota_2",
        AppPartitionSubType::Ota3 => "ota_3",
        AppPartitionSubType::Ota4 => "ota_4",
        AppPartitionSubType::Ota5 => "ota_5",
        AppPartitionSubType::Ota6 => "ota_6",
        AppPartitionSubType::Ota7 => "ota_7",
        AppPartitionSubType::Ota8 => "ota_8",
        AppPartitionSubType::Ota9 => "ota_9",
        AppPartitionSubType::Ota10 => "ota_10",
        AppPartitionSubType::Ota11 => "ota_11",
        AppPartitionSubType::Ota12 => "ota_12",
        AppPartitionSubType::Ota13 => "ota_13",
        AppPartitionSubType::Ota14 => "ota_14",
        AppPartitionSubType::Ota15 => "ota_15",
    }
}

fn words_as_bytes(words: &[u32]) -> &[u8] {
    unsafe { core::slice::from_raw_parts(words.as_ptr().cast(), words.len() * 4) }
}

fn words_as_bytes_mut(words: &mut [u32]) -> &mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(words.as_mut_ptr().cast(), words.len() * 4) }
}

fn align_up(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

pub fn idle_loop() -> ! {
    loop {
        spin_loop();
    }
}

fn uptime_seconds() -> u32 {
    (now_millis() / 1000).min(u32::MAX as u64) as u32
}

pub struct RadioDelay;

impl RadioDelay {
    const fn new() -> Self {
        Self
    }
}

impl DelayNs for RadioDelay {
    async fn delay_ns(&mut self, ns: u32) {
        Timer::after(EmbassyDuration::from_micros(ns.div_ceil(1000) as u64)).await;
    }
}

pub fn radio_delay() -> RadioDelay {
    RadioDelay::new()
}

#[derive(Clone, Copy)]
pub struct SpiDelay;

impl SpiDelay {
    const fn new() -> Self {
        Self
    }
}

impl BlockingDelayNs for SpiDelay {
    fn delay_ns(&mut self, _ns: u32) {
        // The SX1262 driver does not rely on SPI transaction delay operations;
        // radio timing is handled by the radio delay instance.
    }
}

pub fn spi_delay() -> SpiDelay {
    SpiDelay::new()
}

pub fn now_millis() -> u64 {
    Instant::now().duration_since_epoch().as_millis()
}

pub fn now_seconds() -> u32 {
    if let Some(seconds) = retained_wall_clock_seconds() {
        return seconds;
    }

    let offset = critical_section::with(|cs| WALL_CLOCK_OFFSET_SECONDS.borrow(cs).get());
    uptime_seconds().saturating_add(offset)
}

pub fn set_wall_clock_if_forward(unix_seconds: u32) -> bool {
    let current = now_seconds();
    if unix_seconds <= current {
        return false;
    }

    let uptime = uptime_seconds();
    critical_section::with(|cs| {
        WALL_CLOCK_OFFSET_SECONDS
            .borrow(cs)
            .set(unix_seconds.saturating_sub(uptime));
    });
    refresh_retained_wall_clock(unix_seconds);
    true
}

pub fn reboot() -> ! {
    let seconds = now_seconds();
    refresh_retained_wall_clock(seconds);
    println!("Rebooting");
    esp_hal::system::software_reset()
}

#[unsafe(export_name = "custom_halt")]
pub extern "Rust" fn panic_reboot() -> ! {
    println!("Panic: rebooting");
    esp_hal::system::software_reset()
}

fn init_rtc_clock() {
    // The board code does not use LPWR directly. We keep one platform-owned RTC
    // handle so wall-clock time can be derived from RTC elapsed time.
    let rtc = Rtc::new(unsafe { esp_hal::peripherals::LPWR::steal() });
    critical_section::with(|cs| {
        RTC_CLOCK.borrow_ref_mut(cs).replace(rtc);
    });
}

fn retained_wall_clock_seconds() -> Option<u32> {
    let (unix_at_checkpoint, rtc_at_checkpoint) = read_retained_wall_clock()?;
    let rtc_now = rtc_elapsed_seconds().unwrap_or_else(uptime_seconds);
    let seconds = if rtc_now >= rtc_at_checkpoint {
        unix_at_checkpoint.saturating_add(rtc_now.saturating_sub(rtc_at_checkpoint))
    } else {
        unix_at_checkpoint.saturating_add(uptime_seconds())
    };
    refresh_retained_wall_clock(seconds);
    Some(seconds)
}

fn refresh_retained_wall_clock(unix_seconds: u32) {
    if let Some(rtc_seconds) = rtc_elapsed_seconds() {
        write_retained_wall_clock(unix_seconds, rtc_seconds);
    }
}

fn rtc_elapsed_seconds() -> Option<u32> {
    critical_section::with(|cs| {
        let rtc = RTC_CLOCK.borrow_ref(cs);
        let rtc = rtc.as_ref()?;
        Some((rtc.time_since_boot().as_micros() / 1_000_000).min(u64::from(u32::MAX)) as u32)
    })
}

fn read_retained_wall_clock() -> Option<(u32, u32)> {
    let (checksum, unix_seconds, rtc_seconds) = read_retained_wall_clock_words()?;
    (checksum == retained_wall_clock_checksum(unix_seconds, rtc_seconds))
        .then_some((unix_seconds, rtc_seconds))
}

fn write_retained_wall_clock(unix_seconds: u32, rtc_seconds: u32) {
    write_retained_wall_clock_words(
        retained_wall_clock_checksum(unix_seconds, rtc_seconds),
        unix_seconds,
        rtc_seconds,
    );
}

#[cfg(feature = "esp32s3")]
fn read_retained_wall_clock_words() -> Option<(u32, u32, u32)> {
    let rtc = esp_hal::peripherals::LPWR::regs();
    Some((
        rtc.store0().read().bits(),
        rtc.store6().read().bits(),
        rtc.store7().read().bits(),
    ))
}

#[cfg(feature = "esp32s3")]
fn write_retained_wall_clock_words(checksum: u32, unix_seconds: u32, rtc_seconds: u32) {
    let rtc = esp_hal::peripherals::LPWR::regs();
    rtc.store0().write(|w| unsafe { w.bits(checksum) });
    rtc.store6().write(|w| unsafe { w.bits(unix_seconds) });
    rtc.store7().write(|w| unsafe { w.bits(rtc_seconds) });
}

#[cfg(not(feature = "esp32s3"))]
fn read_retained_wall_clock_words() -> Option<(u32, u32, u32)> {
    None
}

#[cfg(not(feature = "esp32s3"))]
fn write_retained_wall_clock_words(_checksum: u32, _unix_seconds: u32, _rtc_seconds: u32) {}

fn retained_wall_clock_checksum(unix_seconds: u32, rtc_seconds: u32) -> u32 {
    WALL_CLOCK_RTC_MAGIC
        ^ unix_seconds.rotate_left(5)
        ^ rtc_seconds.rotate_left(13)
        ^ WALL_CLOCK_RTC_CHECK
}

pub fn battery_level_percent() -> Option<u8> {
    None
}

pub fn battery_millivolts() -> Option<u16> {
    None
}

fn init_heap() {
    static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];

    // SAFETY: The heap backing storage is static and initialized exactly once
    // before board initialization can allocate.
    unsafe {
        #[allow(static_mut_refs)]
        HEAP.init(HEAP_MEM.as_ptr() as usize, HEAP_SIZE);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn esp_wifi_free_internal_heap() -> usize {
    HEAP.free()
}

#[unsafe(no_mangle)]
pub extern "C" fn esp_wifi_allocate_from_internal_ram(size: usize) -> *mut u8 {
    let Some(total_size) = size.checked_add(WIFI_ALLOC_HEADER_SIZE) else {
        return ptr::null_mut();
    };
    let Ok(layout) = Layout::from_size_align(total_size, WIFI_ALLOC_ALIGN) else {
        return ptr::null_mut();
    };

    let allocation = unsafe { GlobalAlloc::alloc(&HEAP, layout) };
    if allocation.is_null() {
        return allocation;
    }

    unsafe {
        (allocation as *mut usize).write(total_size);
        allocation.add(WIFI_ALLOC_HEADER_SIZE)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn esp_wifi_deallocate_internal_ram(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }

    unsafe {
        let allocation = ptr.sub(WIFI_ALLOC_HEADER_SIZE);
        let total_size = (allocation as *const usize).read();
        let Ok(layout) = Layout::from_size_align(total_size, WIFI_ALLOC_ALIGN) else {
            return;
        };
        GlobalAlloc::dealloc(&HEAP, allocation, layout);
    }
}

pub fn log_starting() {
    println!(
        "MCRS firmware {} starting on ESP",
        env!("MESHCORE_FIRMWARE_VERSION")
    );
    esp_rom_sys::rom::ets_delay_us(10_000);
}

pub fn log_fmt(args: fmt::Arguments<'_>) {
    println!("{}", args);
}

pub fn log_hex_line(label: &str, bytes: &[u8], max_len: usize) {
    print!("{}", label);
    for byte in bytes.iter().take(max_len) {
        print!(" {:02x}", byte);
    }
    if bytes.len() > max_len {
        print!(" ...");
    }
    println!();
}

pub fn log_i8_line(label: &str, values: &[i8], max_len: usize) {
    print!("{}", label);
    for value in values.iter().take(max_len) {
        print!(" {}", value);
    }
    if values.len() > max_len {
        print!(" ...");
    }
    println!();
}

pub fn log_radio_spi_config_failed() {
    println!("Failed to configure SX1262 SPI bus");
}

pub fn log_display_i2c_config_failed() {
    println!("Failed to configure OLED I2C bus");
}

pub fn log_cli_uart_config_failed() {
    println!("Failed to configure CLI UART");
}

pub fn log_display_init_failed() {
    println!("Failed to initialize OLED display");
}

pub fn log_display_write_failed() {
    println!("Failed to write OLED display");
}

pub fn log_radio_initialized() {
    println!("SX1262 initialized");
}

pub fn log_radio_init_failed() {
    println!("Failed to initialize SX1262");
}

pub fn log_radio_packet_received(len: usize, rssi: i16, snr: i16, _payload: &[u8]) {
    println!(
        "SX1262 received packet: {} bytes, RSSI {}, SNR {}",
        len, rssi, snr
    );
}

pub fn log_radio_receive_failed() {
    println!("SX1262 receive failed");
}

pub fn log_radio_receive_error(stage: &str, error: &str) {
    println!("SX1262 receive {} error: {}", stage, error);
}
