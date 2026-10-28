use core::future::Future;

pub mod spi_device;
pub mod ssd1306;
pub mod sx1262;

pub struct ReceivedPacket {
    pub len: usize,
    pub rssi: i16,
    pub snr_quarters: i16,
}

pub trait Radio {
    fn wait_for_read(
        &mut self,
        buffer: &mut [u8],
    ) -> impl Future<Output = Result<ReceivedPacket, ()>>;

    fn channel_is_busy(&mut self) -> impl Future<Output = Result<bool, ()>>;

    fn transmit(&mut self, payload: &[u8]) -> impl Future<Output = Result<(), ()>>;
}
