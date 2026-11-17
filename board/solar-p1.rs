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
    platform::{NoButton, NoConsole, NoDisplay},
};
use mcrs_sx1262::{BoardConfig, Sx1262, TcxoCtrlVoltage};
use panic_probe as _;

const MODEL: &str = "SenseCAP Solar Node P1";

#[global_allocator]
static HEAP: Heap = Heap::empty();

bind_interrupts!(struct Irqs {
    TEMP => embassy_nrf::temp::InterruptHandler;
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

    let mut nss = Output::new(p.P0_04, Level::High, OutputDrive::Standard);
    let mut reset = Output::new(p.P0_28, Level::Low, OutputDrive::Standard);

    let mut battery = NrfBattery::new(
        p.SAADC,
        Irqs,
        p.P0_31,
        Some(Output::new(p.P0_14, Level::Low, OutputDrive::Standard)),
        BatteryConfig::xiao(),
    );
    let mut storage = NrfStorage::new(Nvmc::new(p.NVMC));
    mcrs_nrf::sleep::check_startup_voltage(&mut battery, &mut storage).await;
    storage.initialize();
    reset.set_high();

    let dio1 = Input::new(p.P0_03, Pull::Down);
    let busy = Input::new(p.P0_29, Pull::None);

    let mut spi_config = spim::Config::default();
    spi_config.frequency = spim::Frequency::M16;
    let mut spim = Spim::new(p.TWISPI1, Irqs, p.P1_13, p.P1_14, p.P1_15, spi_config);
    let spi = ExclusiveDevice::new(&mut spim, &mut nss, Delay).unwrap();

    let mut radio = Sx1262::new(
        spi,
        &mut reset,
        dio1,
        busy,
        None,
        Delay,
        BoardConfig {
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V7),
            use_dcdc: true,
        },
    );
    let platform = NrfPlatform::new(MODEL, Rng::new_blocking(p.RNG));

    let mut console = NoConsole;
    let mut display = NoDisplay;
    let mut button = NoButton;
    let mut sensors = mcrs_nrf::sensors::NrfSensors(embassy_nrf::temp::Temp::new(p.TEMP, Irqs));

    let repeater = Repeater::new(
        platform,
        storage,
        &mut radio,
        &mut console,
        &mut display,
        &mut button,
        &mut battery,
        &mut sensors,
    );
    let context = repeater.context();
    repeater.run(true).await;
    let lowvoltage = context.with_config(|config| config.lowvoltage()).await;
    battery.shutdown();
    let radio_asleep = mcrs_nrf::sleep::shutdown_radio(&mut radio).await;
    drop(radio);
    drop(spim);
    drop(sensors);
    if !radio_asleep {
        reset.set_low();
    }
    mcrs_nrf::sleep::recover(&mut battery, lowvoltage).await
}
