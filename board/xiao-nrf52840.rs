//! XIAO nRF52840 with Wio-SX1262 radio and a USB CDC terminal.
#![no_std]
#![no_main]

extern crate alloc;

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
    platform::{NoButton, NoDisplay, Radio},
};
use mcrs_sx1262::{BoardConfig, Sx1262, TcxoCtrlVoltage};
use panic_probe as _;

const MODEL: &str = "XIAO nRF52840";

#[global_allocator]
static HEAP: Heap = Heap::empty();

bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
    SAADC => embassy_nrf::saadc::InterruptHandler;
    TWISPI1 => spim::InterruptHandler<peripherals::TWISPI1>;
});

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    unsafe {
        embedded_alloc::init!(HEAP, HEAP_SIZE);
    }

    let mut config = embassy_nrf::config::Config::default();
    // XIAO nRF52840 has the external components for the VDD DC/DC converter.
    config.dcdc.reg1 = true;
    config.hfclk_source = embassy_nrf::config::HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(config);

    let driver = embassy_nrf::usb::Driver::new(
        p.USBD,
        Irqs,
        embassy_nrf::usb::vbus_detect::HardwareVbusDetect::new(Irqs),
    );
    // Match the upstream board's USB identity for local bring-up.
    let mut usb_config = embassy_usb::Config::new(0x2886, 0x8044);
    usb_config.manufacturer = Some("Seeed Studio");
    usb_config.product = Some("MCRS XIAO nRF52840");
    let serial = alloc::format!(
        "{:08X}{:08X}",
        embassy_nrf::pac::FICR.deviceid(1).read(),
        embassy_nrf::pac::FICR.deviceid(0).read()
    );
    usb_config.serial_number = Some(&serial);
    let mut usb_buffers = mcrs_nrf::usb_console::UsbBuffers::new();
    let (mut usb, mut console) =
        mcrs_nrf::usb_console::build_console(driver, usb_config, &mut usb_buffers);

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
        Some(Output::new(p.P0_05, Level::Low, OutputDrive::Standard)),
        Delay,
        BoardConfig {
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V8),
            use_dcdc: true,
            rx_boost: true,
        },
    );
    let platform = NrfPlatform::new(MODEL, Rng::new_blocking(p.RNG));
    let storage = NrfStorage::new(Nvmc::new(p.NVMC));

    let mut display = NoDisplay;
    let mut button = NoButton;
    let mut battery = NrfBattery::new(
        p.SAADC,
        Irqs,
        p.P0_31,
        Some(Output::new(p.P0_14, Level::Low, OutputDrive::Standard)),
        BatteryConfig::xiao(),
    );

    let repeater = Repeater::new(
        platform,
        storage,
        &mut radio,
        &mut console,
        &mut display,
        &mut button,
        &mut battery,
    );
    // Keep USB responsive during mesh processing, including without a host.
    embassy_futures::select::select(usb.run(), repeater.run(0)).await;
    // Brownout has no timer recovery on these boards yet.
    let _ = radio.shutdown().await;
    core::future::pending().await
}
