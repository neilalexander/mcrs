//! RAK4631 repeater with a USB CDC terminal.
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
use mcrs_nrf::{HEAP_SIZE, NrfPlatform, NrfStorage};
use mcrs_repeater::{
    Repeater,
    platform::{NoBattery, NoButton, NoDisplay, Radio},
};
use mcrs_sx1262::{BoardConfig, Sx1262, TcxoCtrlVoltage};
use panic_probe as _;

const MODEL: &str = "RAK4631";

#[global_allocator]
static HEAP: Heap = Heap::empty();

bind_interrupts!(struct Irqs {
    USBD => embassy_nrf::usb::InterruptHandler<peripherals::USBD>;
    CLOCK_POWER => embassy_nrf::usb::vbus_detect::InterruptHandler;
    TWISPI1 => spim::InterruptHandler<peripherals::TWISPI1>;
});

#[embassy_executor::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    unsafe {
        embedded_alloc::init!(HEAP, HEAP_SIZE);
    }

    let mut config = embassy_nrf::config::Config::default();
    config.hfclk_source = embassy_nrf::config::HfclkSource::ExternalXtal;
    let p = embassy_nrf::init(config);

    // Upstream RAK4631Board::begin enables the SX1262 supply before use.
    let _radio_power = Output::new(p.P1_05, Level::High, OutputDrive::Standard);
    embassy_time::Timer::after_millis(10).await;

    let driver = embassy_nrf::usb::Driver::new(
        p.USBD,
        Irqs,
        embassy_nrf::usb::vbus_detect::HardwareVbusDetect::new(Irqs),
    );
    // Match the upstream board's USB identity for local bring-up.
    let mut usb_config = embassy_usb::Config::new(0x239a, 0x8029);
    usb_config.manufacturer = Some("RAKwireless");
    usb_config.product = Some("MCRS RAK4631");
    let serial = alloc::format!(
        "{:08X}{:08X}",
        embassy_nrf::pac::FICR.deviceid(1).read(),
        embassy_nrf::pac::FICR.deviceid(0).read()
    );
    usb_config.serial_number = Some(&serial);
    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut control_buf = [0; 64];
    let mut state = embassy_usb::class::cdc_acm::State::new();
    let mut builder = embassy_usb::Builder::new(
        driver,
        usb_config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut [],
        &mut control_buf,
    );
    let class = embassy_usb::class::cdc_acm::CdcAcmClass::new(&mut builder, &mut state, 64);
    let mut usb = builder.build();
    let mut console = mcrs_nrf::usb_console::UsbConsole::new(class);

    let nss = Output::new(p.P1_10, Level::High, OutputDrive::Standard);
    let reset = Output::new(p.P1_06, Level::High, OutputDrive::Standard);
    let dio1 = Input::new(p.P1_15, Pull::Down);
    let busy = Input::new(p.P1_14, Pull::None);

    let mut spi_config = spim::Config::default();
    spi_config.frequency = spim::Frequency::M16;
    let spim = Spim::new(p.TWISPI1, Irqs, p.P1_11, p.P1_13, p.P1_12, spi_config);
    let spi = ExclusiveDevice::new(spim, nss, Delay).unwrap();

    let mut radio = Sx1262::new(
        spi,
        reset,
        dio1,
        busy,
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
    let mut battery = NoBattery;

    let repeater = Repeater::new(
        platform,
        storage,
        &mut radio,
        &mut console,
        &mut display,
        &mut button,
        &mut battery,
    );
    // USB enumeration and mesh processing both run without a host connected.
    embassy_futures::select::select(usb.run(), repeater.run(0)).await;
    // Brownout has no timer recovery on these boards yet.
    let _ = radio.shutdown().await;
    core::future::pending().await
}
