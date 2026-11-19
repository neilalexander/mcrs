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
use mcrs_nrf::{
    HEAP_SIZE, NrfPlatform, NrfStorage,
    battery::{BatteryConfig, NrfBattery},
    sleep, usb_console,
};
use mcrs_repeater::{
    Repeater,
    app::config::AppConfig,
    platform::{
        NoButton, NoDisplay,
        led::{GpioLed, PinState},
    },
};
use mcrs_sx1262::{BoardConfig, Sx1262, TcxoCtrlVoltage};
use panic_probe as _;

const MODEL: &str = "RAK4631";

#[global_allocator]
static HEAP: Heap = Heap::empty();

bind_interrupts!(struct Irqs {
    TEMP => embassy_nrf::temp::InterruptHandler;
    SAADC => embassy_nrf::saadc::InterruptHandler;
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
    config.lfclk_source = embassy_nrf::config::LfclkSource::ExternalXtal;
    // RAK4631 has the external components for the nRF's VDD DC/DC converter.
    config.dcdc.reg1 = true;
    let p = embassy_nrf::init(config);

    let mut radio_power = Output::new(p.P1_05, Level::Low, OutputDrive::Standard);
    let mut nss = Output::new(p.P1_10, Level::Low, OutputDrive::Standard);
    let mut reset = Output::new(p.P1_06, Level::Low, OutputDrive::Standard);

    let mut battery = NrfBattery::new(p.SAADC, Irqs, p.P0_05, None, BatteryConfig::rak4631());
    let mut storage = NrfStorage::new(Nvmc::new(p.NVMC));
    sleep::check_startup_voltage(&mut battery, &mut storage).await;
    storage.initialize();
    radio_power.set_high();
    embassy_time::Timer::after_millis(10).await;
    nss.set_high();
    reset.set_high();

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
    let mut usb_buffers = usb_console::UsbBuffers::new();
    let (mut usb, mut console) = usb_console::build_console(
        AppConfig::load_console_enabled(&mut storage),
        || {
            embassy_nrf::usb::Driver::new(
                p.USBD,
                Irqs,
                embassy_nrf::usb::vbus_detect::HardwareVbusDetect::new(Irqs),
            )
        },
        usb_config,
        &mut usb_buffers,
    );

    let dio1 = Input::new(p.P1_15, Pull::Down);
    let busy = Input::new(p.P1_14, Pull::None);

    let mut spi_config = spim::Config::default();
    spi_config.frequency = spim::Frequency::M2;
    let mut spim = Spim::new(p.TWISPI1, Irqs, p.P1_11, p.P1_13, p.P1_12, spi_config);
    let spi = ExclusiveDevice::new(&mut spim, &mut nss, Delay).unwrap();

    let mut radio = Sx1262::new(
        spi,
        &mut reset,
        dio1,
        busy,
        None,
        Delay,
        BoardConfig {
            tcxo_ctrl: Some(TcxoCtrlVoltage::Ctrl1V8),
            use_dcdc: true,
        },
    );
    let platform = NrfPlatform::new(MODEL, Rng::new_blocking(p.RNG));

    let mut led = GpioLed::new(
        Output::new(p.P1_03, Level::Low, OutputDrive::Standard),
        PinState::High,
    )
    .unwrap();
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
        &mut led,
    );
    // USB enumeration and mesh processing both run without a host connected.
    let context = repeater.context();
    let sleep_request = repeater
        .run(true, async {
            if let Some(usb) = &mut usb {
                usb.run().await;
            }
        })
        .await;
    if let Some(usb) = &mut usb {
        usb.disable().await;
    }
    let lowvoltage = context.with_config(|config| config.lowvoltage()).await;
    battery.shutdown();
    sleep::shutdown_radio(&mut radio).await;
    drop(radio);
    drop(spim);
    drop(sensors);
    nss.set_low();
    reset.set_low();
    radio_power.set_low();
    sleep::enter(sleep_request, lowvoltage, &mut battery).await
}
