//! Hardware entropy while the ESP Wi-Fi controller is running.
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
