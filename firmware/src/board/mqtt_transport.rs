//! ESP transport adapters. Only used while Wi-Fi is running (hardware RNG entropy).
use core::{
    cell::RefCell,
    future::{Future, poll_fn},
    pin::pin,
};
use embassy_net::tcp::{Error, TcpSocket};
use embedded_io_async::{ErrorType, Read, Write};

// Each poll borrows the socket only for that poll. Embassy TCP read/write have
// no per-future state and are cancellation safe, allowing TLS's split halves to
// drive independent reads and writes without holding a RefCell borrow over await.
#[derive(Clone, Copy)]
pub struct SharedSocket<'a, 's>(pub &'a RefCell<TcpSocket<'s>>);
impl ErrorType for SharedSocket<'_, '_> {
    type Error = Error;
}
impl Read for SharedSocket<'_, '_> {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, Error> {
        poll_fn(|cx| {
            let mut socket = self.0.borrow_mut();
            pin!(socket.read(out)).poll(cx)
        })
        .await
    }
}
impl Write for SharedSocket<'_, '_> {
    async fn write(&mut self, data: &[u8]) -> Result<usize, Error> {
        poll_fn(|cx| {
            let mut socket = self.0.borrow_mut();
            pin!(socket.write(data)).poll(cx)
        })
        .await
    }
    async fn flush(&mut self) -> Result<(), Error> {
        poll_fn(|cx| {
            let mut socket = self.0.borrow_mut();
            pin!(socket.flush()).poll(cx)
        })
        .await
    }
}

pub struct WifiRng(esp_hal::rng::Trng);
impl WifiRng {
    pub fn new() -> Self {
        // MQTT only runs while the Wi-Fi controller supplies entropy.
        Self(esp_hal::rng::Trng::try_new().expect("Wi-Fi entropy source enabled"))
    }
    pub fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut bytes = [0; N];
        self.0.read(&mut bytes);
        bytes
    }
}
impl rand_core::RngCore for WifiRng {
    fn next_u32(&mut self) -> u32 {
        self.0.random()
    }
    fn next_u64(&mut self) -> u64 {
        u64::from_le_bytes(self.bytes())
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.read(dest);
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}
impl rand_core::CryptoRng for WifiRng {}
