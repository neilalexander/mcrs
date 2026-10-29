#![no_std]

extern crate alloc;

pub mod board;
pub mod esp;
mod littlefs;
pub mod platform;

/// The serial port the CLI reads commands from.
#[allow(async_fn_in_trait)]
pub trait CliSerial {
    type Error;

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error>;
}

impl CliSerial for esp_hal::uart::Uart<'static, esp_hal::Async> {
    type Error = esp_hal::uart::RxError;

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.read_async(buf).await
    }
}
