//! MCRS repeater for the SenseCAP Solar Node P1. It has no status screen,
//! button or console.
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
use mcrs_nrf::{
    HEAP_SIZE, NrfPlatform, NrfStorage,
    battery::{BatteryConfig, NrfBattery},
};
use mcrs_repeater::{
    Repeater,
    platform::{NoButton, NoConsole, NoDisplay, Radio},
};
use mcrs_sx1262::{BoardConfig, Sx1262, TcxoCtrlVoltage};
use panic_probe as _;

const MODEL: &str = "SenseCAP Solar Node P1";

#[global_allocator]
static HEAP: Heap = Heap::empty();

bind_interrupts!(struct Irqs {
    SAADC => embassy_nrf::saadc::InterruptHandler;
    TWISPI1 => spim::InterruptHandler<peripherals::TWISPI1>;
});

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    unsafe {
        embedded_alloc::init!(HEAP, HEAP_SIZE);
    }

    let mut config = embassy_nrf::config::Config::default();
    // Solar P1 (XIAO nRF52840 Plus) has the external components for the VDD DC/DC converter.
    config.dcdc.reg1 = true;
    let p = embassy_nrf::init(config);

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
        None,
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
    let mut battery = NrfBattery::new(
        p.SAADC,
        Irqs,
        p.P0_31,
        Some(Output::new(p.P0_14, Level::Low, OutputDrive::Standard)),
        BatteryConfig::xiao(),
    );

    Repeater::new(
        platform,
        storage,
        &mut radio,
        &mut console,
        &mut display,
        &mut button,
        &mut battery,
    )
    .run(0) // Report voltage without automatic low-voltage shutdown.
    .await;
    // Brownout has no timer recovery on these boards yet.
    let _ = radio.shutdown().await;
    core::future::pending().await
}
