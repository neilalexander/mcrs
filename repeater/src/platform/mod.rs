//! The boundary between the repeater and the hardware it runs on.
//!
//! A board crate implements [`Platform`] and passes it to [`crate::Repeater`].
//! `Platform` covers the board as a whole: clocks, randomness, rebooting, and
//! handing each device to the repeater as it starts. Every device the repeater
//! drives has its own trait in this module, and `Platform` names the board's
//! implementation of each one as an associated type. Devices that a standard
//! embedded trait already describes use that trait instead: the console is an
//! `embedded-io-async` stream, and the button and delays come from
//! `embedded-hal-async`.

pub mod storage;

use core::{convert::Infallible, future::Future};

use embedded_hal_async::{delay::DelayNs, digital::Wait};
use embedded_io_async::{Read, Write};

use crate::{app::config::RadioConfig, memory::MemoryProfile};

/// A board the repeater runs on.
///
/// While it starts, [`crate::Repeater`] calls each `&mut self` method once.
/// After that it shares the platform between its tasks, which only use the
/// `&self` methods.
pub trait Platform {
    /// Where the configuration is kept.
    type Storage: Storage;
    /// The LoRa radio.
    type Radio: Radio;
    /// The serial port for the local CLI.
    type Console: Read + Write;
    /// The status screen, or [`Absent`].
    type Display: StatusDisplay;
    /// The user button, or [`Absent`]. A press pulls the input low.
    type Button: Wait;
    /// The battery monitor, or [`Absent`].
    type Battery: Battery;
    /// Delays for the repeater's timers.
    type Delay: DelayNs;

    /// The board's name, as reported over MQTT.
    fn model(&self) -> &'static str;

    /// Sizes the repeater's queues and tables to the board's memory.
    fn memory_profile(&self) -> MemoryProfile;

    /// Random bytes for a new identity, used when storage holds none.
    fn identity_seed(&mut self) -> [u8; 32];

    fn take_storage(&mut self) -> Self::Storage;

    /// Starts the radio with the stored radio settings.
    fn start_radio(
        &mut self,
        config: RadioConfig,
    ) -> impl Future<Output = Result<Self::Radio, ()>>;

    fn take_console(&mut self) -> Option<Self::Console>;

    fn take_display(&mut self) -> Option<Self::Display> {
        None
    }

    fn take_button(&mut self) -> Option<Self::Button> {
        None
    }

    fn take_battery(&mut self) -> Option<Self::Battery> {
        None
    }

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

/// Stands in for a device the board doesn't have. It has no values, so the
/// board's `take_*` method for that device returns `None`.
pub enum Absent {}

impl StatusDisplay for Absent {
    async fn init<D: DelayNs>(&mut self, _delay: &mut D) -> Result<(), ()> {
        match *self {}
    }

    fn show(&mut self, _status: &DisplayStatus<'_>) -> Result<(), ()> {
        match *self {}
    }

    fn sleep(&mut self) -> Result<(), ()> {
        match *self {}
    }

    async fn wake<D: DelayNs>(&mut self, _delay: &mut D) -> Result<(), ()> {
        match *self {}
    }
}

impl embedded_hal::digital::ErrorType for Absent {
    type Error = Infallible;
}

impl Wait for Absent {
    async fn wait_for_high(&mut self) -> Result<(), Self::Error> {
        match *self {}
    }

    async fn wait_for_low(&mut self) -> Result<(), Self::Error> {
        match *self {}
    }

    async fn wait_for_rising_edge(&mut self) -> Result<(), Self::Error> {
        match *self {}
    }

    async fn wait_for_falling_edge(&mut self) -> Result<(), Self::Error> {
        match *self {}
    }

    async fn wait_for_any_edge(&mut self) -> Result<(), Self::Error> {
        match *self {}
    }
}

impl Battery for Absent {
    async fn millivolts(&mut self) -> Option<u16> {
        match *self {}
    }
}
