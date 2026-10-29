//! The repeater platform for an nRF52840 with an SX1262 radio.
use core::cell::{Cell, RefCell};

use defmt::{debug, error};
use embassy_nrf::{
    gpio::{Input, Output},
    mode::Blocking,
    rng::Rng,
    spim::Spim,
};
use embassy_time::{Delay, Instant};
use embedded_hal_bus::spi::ExclusiveDevice;
use mcrs_repeater::{
    app::config::RadioConfig,
    memory::MemoryProfile,
    modules::sx1262::{self, BoardConfig},
    platform::{Platform, RadioHardware},
};

/// The heap the repeater's tables and queues are allocated from.
pub const HEAP_SIZE: usize = 64 * 1024;
const MEMORY_PROFILE: MemoryProfile = MemoryProfile::new(HEAP_SIZE, 8, 16, 64, 256);

pub type RadioSpi = ExclusiveDevice<Spim<'static>, Output<'static>, Delay>;

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
}

fn uptime_seconds() -> u32 {
    Instant::now().as_secs().min(u64::from(u32::MAX)) as u32
}

/// The SX1262, until the repeater starts it.
pub struct NrfRadio {
    spi: RadioSpi,
    reset: Output<'static>,
    dio1: Input<'static>,
    busy: Input<'static>,
    board_config: BoardConfig,
}

impl NrfRadio {
    pub fn new(
        spi: RadioSpi,
        reset: Output<'static>,
        dio1: Input<'static>,
        busy: Input<'static>,
        board_config: BoardConfig,
    ) -> Self {
        Self {
            spi,
            reset,
            dio1,
            busy,
            board_config,
        }
    }
}

impl RadioHardware for NrfRadio {
    type Radio = sx1262::Receiver<RadioSpi, Output<'static>, Input<'static>, Delay>;

    async fn start(self, config: RadioConfig) -> Result<Self::Radio, ()> {
        let Self {
            spi,
            reset,
            dio1,
            busy,
            board_config,
        } = self;

        match sx1262::init_receiver(
            spi,
            reset,
            dio1,
            busy,
            Delay,
            board_config,
            config.receive_config(),
        )
        .await
        {
            Ok(receiver) => {
                debug!("radio initialized");
                Ok(receiver)
            }
            Err(e) => {
                error!("failed to initialize sx1262: {:?}", e);
                Err(())
            }
        }
    }
}
