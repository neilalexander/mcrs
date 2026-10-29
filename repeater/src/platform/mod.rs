//! The boundary between the repeater and the hardware it runs on.
//!
//! A board crate implements [`Platform`] for the services the whole board
//! provides: clocks, randomness, delays and rebooting. It passes that to
//! [`crate::Repeater::new`] along with each device the repeater drives, and
//! every device has its own trait in this module. Devices that a standard
//! embedded trait already describes use that trait instead: the console is an
//! `embedded-io-async` stream and the button an `embedded-hal-async` input. A
//! board without storage, a console, status screen, button or battery monitor
//! passes [`NoStorage`], [`NoConsole`], [`NoDisplay`], [`NoButton`] or
//! [`NoBattery`].

pub mod storage;

use core::{convert::Infallible, future::Future};

use embedded_hal_async::{delay::DelayNs, digital::Wait};
use embedded_io_async::{Read, Write};

use crate::{app::config::RadioConfig, memory::MemoryProfile};

/// The services the whole board provides. The repeater shares the platform
/// between its tasks.
pub trait Platform {
    /// Delays for the repeater's timers.
    type Delay: DelayNs;

    /// The board's name, as reported over MQTT.
    fn model(&self) -> &'static str;

    /// Sizes the repeater's queues and tables to the board's memory.
    fn memory_profile(&self) -> MemoryProfile;

    /// Random bytes for a new identity, used when storage holds none.
    fn identity_seed(&self) -> [u8; 32];

    /// A new delay. Each task that waits gets its own.
    fn delay(&self) -> Self::Delay;

    /// Milliseconds since boot.
    fn now_millis(&self) -> u64;

    /// Unix time in seconds, or the uptime until the clock has been set.
    fn now_seconds(&self) -> u32;

    /// Sets the wall clock unless that would move it backwards, and returns
    /// whether it changed.
    fn set_wall_clock_if_forward(&self, unix_seconds: u32) -> bool;

    fn random_u32(&self) -> u32;

    fn reboot(&self) -> !;
}

/// Keeps small files, such as the configuration, across reboots.
pub trait Storage {
    /// Reads the file `key` into `buffer` and returns its length.
    fn read(&mut self, key: &str, buffer: &mut [u8]) -> Result<usize, storage::Error>;

    /// Replaces the file `key`. A power cut leaves either the old or the new
    /// contents.
    fn write_atomic(&mut self, key: &str, data: &[u8]) -> Result<(), storage::Error>;
}

/// Radio hardware that the repeater starts once it has loaded the radio
/// settings.
pub trait RadioHardware {
    type Radio: Radio;

    fn start(self, config: RadioConfig) -> impl Future<Output = Result<Self::Radio, ()>>;
}

/// A packet that [`Radio::wait_for_read`] copied into the caller's buffer.
pub struct ReceivedPacket {
    pub len: usize,
    /// Signal strength in dBm.
    pub rssi: i16,
    /// Signal-to-noise ratio in quarters of a dB.
    pub snr_quarters: i16,
}

/// Sends and receives LoRa packets with the configured modulation.
pub trait Radio {
    fn wait_for_read(
        &mut self,
        buffer: &mut [u8],
    ) -> impl Future<Output = Result<ReceivedPacket, ()>>;

    /// Runs channel activity detection.
    fn channel_is_busy(&mut self) -> impl Future<Output = Result<bool, ()>>;

    fn transmit(&mut self, payload: &[u8]) -> impl Future<Output = Result<(), ()>>;
}

/// What the status screen shows.
pub struct DisplayStatus<'a> {
    pub node_name: &'a str,
    pub version: &'a str,
    pub public_key_prefix: [u8; 3],
    pub packets_sent: u32,
    pub packets_received: u32,
    pub packet_errors: u32,
    pub battery_millivolts: Option<u16>,
    pub battery_level_percent: Option<u8>,
    /// A notice for the user, such as confirmation that an advert was sent.
    pub message: Option<&'a str>,
}

/// A screen that shows the repeater's status.
pub trait StatusDisplay {
    /// Powers the screen on and clears it.
    fn init<D: DelayNs>(&mut self, delay: &mut D) -> impl Future<Output = Result<(), ()>>;

    fn show(&mut self, status: &DisplayStatus<'_>) -> Result<(), ()>;

    /// Turns the screen off to save power.
    fn sleep(&mut self) -> Result<(), ()>;

    /// Turns the screen back on after [`StatusDisplay::sleep`].
    fn wake<D: DelayNs>(&mut self, delay: &mut D) -> impl Future<Output = Result<(), ()>>;
}

/// Measures the battery.
pub trait Battery {
    /// The battery voltage in millivolts, or `None` if the reading failed.
    fn millivolts(&mut self) -> impl Future<Output = Option<u16>>;
}

/// Storage for a board without any. Nothing is kept, so the repeater starts
/// from its defaults with a new identity on every boot.
pub struct NoStorage;

impl Storage for NoStorage {
    fn read(&mut self, _key: &str, _buffer: &mut [u8]) -> Result<usize, storage::Error> {
        Err(storage::Error::NotAvailable)
    }

    fn write_atomic(&mut self, _key: &str, _data: &[u8]) -> Result<(), storage::Error> {
        Err(storage::Error::NotAvailable)
    }
}

/// The console for a board without one. It never receives input and discards
/// output.
pub struct NoConsole;

impl embedded_io_async::ErrorType for NoConsole {
    type Error = Infallible;
}

impl Read for NoConsole {
    async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, Self::Error> {
        core::future::pending().await
    }
}

impl Write for NoConsole {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        Ok(buf.len())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// The status screen for a board without one. Every operation succeeds and
/// shows nothing.
pub struct NoDisplay;

impl StatusDisplay for NoDisplay {
    async fn init<D: DelayNs>(&mut self, _delay: &mut D) -> Result<(), ()> {
        Ok(())
    }

    fn show(&mut self, _status: &DisplayStatus<'_>) -> Result<(), ()> {
        Ok(())
    }

    fn sleep(&mut self) -> Result<(), ()> {
        Ok(())
    }

    async fn wake<D: DelayNs>(&mut self, _delay: &mut D) -> Result<(), ()> {
        Ok(())
    }
}

/// The user button for a board without one. Its input stays high, as if the
/// button were never pressed.
pub struct NoButton;

impl embedded_hal::digital::ErrorType for NoButton {
    type Error = Infallible;
}

impl Wait for NoButton {
    async fn wait_for_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn wait_for_low(&mut self) -> Result<(), Self::Error> {
        core::future::pending().await
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), Self::Error> {
        core::future::pending().await
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), Self::Error> {
        core::future::pending().await
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), Self::Error> {
        core::future::pending().await
    }
}

/// The battery monitor for a board without one. It never has a reading.
pub struct NoBattery;

impl Battery for NoBattery {
    async fn millivolts(&mut self) -> Option<u16> {
        None
    }
}
