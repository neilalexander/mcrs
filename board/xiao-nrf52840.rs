//! MCRS repeater for an XIAO nRF52840 with an SX1262 radio. It has no status
//! screen, button, console or battery monitor.
#![no_std]
#![no_main]

use defmt_rtt as _;
use embassy_nrf::{
    bind_interrupts,
    gpio::{Input, Level, Output, OutputDrive, Pull},
    nvmc::Nvmc,
    peripherals,
    rng::Rng,
    spim::{self, Spim},
};
use embassy_time::Delay;
use embedded_alloc::LlffHeap as Heap;
use embedded_hal_bus::spi::ExclusiveDevice;
use mcrs_nrf::{HEAP_SIZE, NrfPlatform, NrfStorage};
use mcrs_repeater::{
    Repeater,
    platform::{NoBattery, NoButton, NoConsole, NoDisplay, Radio},
};
use mcrs_sx1262::{BoardConfig, Sx1262, TcxoCtrlVoltage};
use panic_probe as _;

const MODEL: &str = "XIAO nRF52840";

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

    let mut radio = Sx1262::new(
        spi,
        reset,
        dio1,
        busy,
        Delay,
        BoardConfig {
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V7),
            use_dcdc: true,
            rx_boost: false,
        },
    );
    let platform = NrfPlatform::new(MODEL, Rng::new_blocking(p.RNG));
    let storage = NrfStorage::new(Nvmc::new(p.NVMC));

    let mut console = NoConsole;
    let mut display = NoDisplay;
    let mut button = NoButton;
    let mut battery = NoBattery;

    Repeater::new(
        platform,
        storage,
        &mut radio,
        &mut console,
        &mut display,
        &mut button,
        &mut battery,
    )
    .run(0) // Battery monitoring disabled.
    .await;
    // Brownout has no timer recovery on these boards yet.
    let _ = radio.shutdown().await;
    core::future::pending().await
}
