//! MCRS repeater for an nRF52840 with an SX1262 radio. It has no storage,
//! status screen, button, console or battery monitor.
#![no_std]
#![no_main]

use crate::platform::{HEAP_SIZE, NrfPlatform, NrfRadio};
use embassy_nrf::{
    bind_interrupts,
    gpio::{Input, Level, Output, OutputDrive, Pull},
    peripherals,
    rng::Rng,
    spim::{self, Spim},
};
use embassy_time::Delay;
use embedded_alloc::LlffHeap as Heap;
use embedded_hal_bus::spi::ExclusiveDevice;
use lora_phy::sx126x::TcxoCtrlVoltage;
use mcrs_repeater::{
    Repeater,
    modules::sx1262::BoardConfig,
    platform::{NoBattery, NoButton, NoConsole, NoDisplay, NoStorage},
};
use {defmt_rtt as _, panic_probe as _};

mod platform;

const MODEL: &str = "nRF52840";

#[global_allocator]
static HEAP: Heap = Heap::empty();

bind_interrupts!(struct Irqs {
    TWISPI1 => spim::InterruptHandler<peripherals::TWISPI1>;
});

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    unsafe {
        embedded_alloc::init!(HEAP, HEAP_SIZE);
    }

    let p = embassy_nrf::init(Default::default());

    let nss = Output::new(p.P0_04, Level::High, OutputDrive::Standard);
    let reset = Output::new(p.P0_28, Level::High, OutputDrive::Standard);
    let dio1 = Input::new(p.P0_03, Pull::Down);
    let busy = Input::new(p.P0_29, Pull::None);

    let mut spi_config = spim::Config::default();
    spi_config.frequency = spim::Frequency::M16;
    let spim = Spim::new(p.TWISPI1, Irqs, p.P1_13, p.P1_14, p.P1_15, spi_config);
    let spi = ExclusiveDevice::new(spim, nss, Delay).unwrap();

    let radio = NrfRadio::new(
        spi,
        reset,
        dio1,
        busy,
        BoardConfig {
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V7),
            use_dcdc: true,
            rx_boost: false,
        },
    );
    let platform = NrfPlatform::new(MODEL, Rng::new_blocking(p.RNG));

    Repeater::new(
        platform, NoStorage, radio, NoConsole, NoDisplay, NoButton, NoBattery,
    )
    .run()
    .await
}
