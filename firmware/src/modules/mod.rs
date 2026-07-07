pub mod spi_device;
pub mod ssd1306;
pub mod sx1262;

pub struct ReceivedPacket {
    pub len: usize,
    pub rssi: i16,
    pub snr: i16,
}

pub trait Receiver {
    async fn wait_for_read(&mut self, buffer: &mut [u8]) -> Result<ReceivedPacket, ()>;

    async fn channel_is_busy(&mut self) -> Result<bool, ()>;

    async fn transmit(&mut self, payload: &[u8]) -> Result<(), ()>;
}
