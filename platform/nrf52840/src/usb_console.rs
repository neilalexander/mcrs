//! A USB CDC packet endpoint presented as a terminal byte stream.
use embassy_usb::{
    class::cdc_acm::{CdcAcmClass, CdcAcmError},
    driver::Driver,
};
use embedded_io_async::{ErrorType, Read, Write};

/// Owns a full USB packet so the CLI can safely read one byte at a time.
pub struct UsbConsole<'d, D: Driver<'d>> {
    class: CdcAcmClass<'d, D>,
    packet: [u8; 64],
    start: usize,
    end: usize,
}

impl<'d, D: Driver<'d>> UsbConsole<'d, D> {
    pub fn new(class: CdcAcmClass<'d, D>) -> Self {
        assert_eq!(class.max_packet_size(), 64);
        Self {
            class,
            packet: [0; 64],
            start: 0,
            end: 0,
        }
    }
}

impl<'d, D: Driver<'d>> ErrorType for UsbConsole<'d, D> {
    type Error = CdcAcmError;
}

impl<'d, D: Driver<'d>> Read for UsbConsole<'d, D> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        while self.start == self.end {
            // Sleep until enumeration, including after an unplug. Never spin
            // on a disabled endpoint while the repeater runs on battery.
            self.class.wait_connection().await;
            self.start = 0;
            self.end = 0;
            self.end = self
                .class
                .read_packet(&mut self.packet)
                .await
                .map_err(|_| CdcAcmError::NotConnected)?;
        }
        let count = buf.len().min(self.end - self.start);
        buf[..count].copy_from_slice(&self.packet[self.start..self.start + count]);
        self.start += count;
        Ok(count)
    }
}

impl<'d, D: Driver<'d>> Write for UsbConsole<'d, D> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        let count = buf.len().min(64);
        self.class
            .write_packet(&buf[..count])
            .await
            .map_err(|_| CdcAcmError::NotConnected)?;
        // A full packet needs a short packet to terminate the host transfer.
        if count == 64 {
            self.class
                .write_packet(&[])
                .await
                .map_err(|_| CdcAcmError::NotConnected)?;
        }
        Ok(count)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
