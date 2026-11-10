#![no_std]

//! The repeater platform for an nRF52840. The board chooses the radio.
extern crate alloc;

use core::cell::{Cell, RefCell};
use embassy_nrf::{mode::Blocking, pac::POWER, rng::Rng};
use embassy_time::{Delay, Instant};
use mcrs_repeater::{
    memory::MemoryProfile,
    platform::{OtaStatus, Platform},
};

pub mod battery;
mod littlefs;
pub mod usb_console;

pub use littlefs::NrfStorage;

/// The heap the repeater's tables and queues are allocated from.
pub const HEAP_SIZE: usize = 64 * 1024;
const MEMORY_PROFILE: MemoryProfile = MemoryProfile::new(HEAP_SIZE, 8, 16, 64, 256);

/// https://github.com/adafruit/Adafruit_nRF52_Bootloader/blob/master/src/main.c#L96
///
/// Will need changing if we ever use SoftDevice
const DFU_MAGIC_OTA_RESET: u8 = 0xA8;

/// The services the nRF52840 provides.
pub struct NrfPlatform {
    model: &'static str,
    identity_seed: [u8; 32],
    rng: RefCell<Rng<'static, Blocking>>,
    /// Added to the uptime to give Unix time. The wall clock is lost on reboot.
    wall_clock_offset: Cell<u32>,
}

impl NrfPlatform {
    /// `model` names the board in MQTT status reports.
    pub fn new(model: &'static str, mut rng: Rng<'static, Blocking>) -> Self {
        // The seed becomes the private key, so remove any bias from the raw bits.
        rng.set_bias_correction(true);
        let mut identity_seed = [0u8; 32];
        rng.blocking_fill_bytes(&mut identity_seed);

        Self {
            model,
            identity_seed,
            rng: RefCell::new(rng),
            wall_clock_offset: Cell::new(0),
        }
    }
}

impl Platform for NrfPlatform {
    type Delay = Delay;

    fn model(&self) -> &'static str {
        self.model
    }

    fn memory_profile(&self) -> MemoryProfile {
        MEMORY_PROFILE
    }

    fn identity_seed(&self) -> [u8; 32] {
        self.identity_seed
    }

    fn delay(&self) -> Self::Delay {
        Delay
    }

    fn now_millis(&self) -> u64 {
        Instant::now().as_millis()
    }

    fn now_seconds(&self) -> u32 {
        uptime_seconds().saturating_add(self.wall_clock_offset.get())
    }

    fn set_wall_clock_if_forward(&self, unix_seconds: u32) -> bool {
        if unix_seconds <= self.now_seconds() {
            return false;
        }

        self.wall_clock_offset
            .set(unix_seconds.saturating_sub(uptime_seconds()));
        true
    }

    fn random_u32(&self) -> u32 {
        self.rng.borrow_mut().blocking_next_u32()
    }

    fn reboot(&self) -> ! {
        cortex_m::peripheral::SCB::sys_reset()
    }

    fn start_ota(&self) {
        // GPREGRET keeps its value through the reboot the repeater does once
        // it has replied, and the bootloader then finds the magic value and
        // starts its update mode instead of the firmware
        POWER
            .gpregret()
            .write(|w| w.set_gpregret(DFU_MAGIC_OTA_RESET));
    }

    fn stop_ota(&self) {
        POWER.gpregret().write(|w| w.set_gpregret(0));
    }

    fn ota_status(&self) -> OtaStatus {
        if POWER.gpregret().read().gpregret() == DFU_MAGIC_OTA_RESET {
            OtaStatus::PendingBootloader
        } else {
            OtaStatus::Stopped
        }
    }
}

fn uptime_seconds() -> u32 {
    Instant::now().as_secs().min(u64::from(u32::MAX)) as u32
}
