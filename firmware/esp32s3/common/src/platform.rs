//! Start-up shared by the Heltec ESP32-S3 boards. A board supplies only what
//! differs: its CLI serial port, its SX1262 configuration and its radio frontend.
use defmt::error;
use esp_hal::{
    analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation},
    gpio::{Flex, Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    interrupt::software::SoftwareInterruptControl,
    peripherals::{
        ADC1, GPIO0, GPIO1, GPIO2, GPIO5, GPIO7, GPIO8, GPIO9, GPIO10, GPIO11, GPIO12, GPIO13,
        GPIO14, GPIO17, GPIO18, GPIO21, GPIO36, GPIO37, GPIO43, GPIO44, GPIO46, I2C0, RNG, SPI2,
        UART0, USB_DEVICE, WIFI,
    },
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
    timer::timg::TimerGroup,
};
use mcrs_repeater::{
    app::{AppContext, config::AppConfig},
    modules::ssd1306::{Display, PowerActiveLevel},
};

pub use mcrs_repeater::modules::sx1262::BoardConfig;

use crate::{
    CliSerial,
    board::heltec::{
        self, BatteryMonitor, EspPlatform, MEMORY_PROFILE, RadioFrontend, RadioResources,
        STORAGE_LAYOUT, WifiResources,
    },
    esp,
};

/// The peripherals that every Heltec board wires the same way.
pub struct Platform {
    radio_spi: SPI2<'static>,
    radio_sck: GPIO9<'static>,
    radio_mosi: GPIO10<'static>,
    radio_miso: GPIO11<'static>,
    radio_cs: GPIO8<'static>,
    radio_reset: GPIO12<'static>,
    radio_dio1: GPIO14<'static>,
    radio_busy: GPIO13<'static>,
    display_i2c: I2C0<'static>,
    display_sda: GPIO17<'static>,
    display_scl: GPIO18<'static>,
    display_reset: GPIO21<'static>,
    display_power: GPIO36<'static>,
    button: GPIO0<'static>,
    rng: RNG<'static>,
    adc: ADC1<'static>,
    battery_sense: GPIO1<'static>,
    battery_ctrl: GPIO37<'static>,
    wifi: WIFI<'static>,
}

/// The peripherals left for the board's CLI serial port and radio frontend.
/// Add a peripheral here when a board needs another one.
#[allow(non_snake_case)]
pub struct BoardPeripherals {
    pub UART0: UART0<'static>,
    pub USB_DEVICE: USB_DEVICE<'static>,
    pub GPIO2: GPIO2<'static>,
    pub GPIO5: GPIO5<'static>,
    pub GPIO7: GPIO7<'static>,
    pub GPIO43: GPIO43<'static>,
    pub GPIO44: GPIO44<'static>,
    pub GPIO46: GPIO46<'static>,
}

/// Initializes the chip and the scheduler, then splits the peripherals between
/// the shared platform and the board.
pub fn init() -> (Platform, BoardPeripherals) {
    let peripherals = esp::init().peripherals;
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let software_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, software_interrupt.software_interrupt0);

    let platform = Platform {
        radio_spi: peripherals.SPI2,
        radio_sck: peripherals.GPIO9,
        radio_mosi: peripherals.GPIO10,
        radio_miso: peripherals.GPIO11,
        radio_cs: peripherals.GPIO8,
        radio_reset: peripherals.GPIO12,
        radio_dio1: peripherals.GPIO14,
        radio_busy: peripherals.GPIO13,
        display_i2c: peripherals.I2C0,
        display_sda: peripherals.GPIO17,
        display_scl: peripherals.GPIO18,
        display_reset: peripherals.GPIO21,
        display_power: peripherals.GPIO36,
        button: peripherals.GPIO0,
        rng: peripherals.RNG,
        adc: peripherals.ADC1,
        battery_sense: peripherals.GPIO1,
        battery_ctrl: peripherals.GPIO37,
        wifi: peripherals.WIFI,
    };
    let board = BoardPeripherals {
        UART0: peripherals.UART0,
        USB_DEVICE: peripherals.USB_DEVICE,
        GPIO2: peripherals.GPIO2,
        GPIO5: peripherals.GPIO5,
        GPIO7: peripherals.GPIO7,
        GPIO43: peripherals.GPIO43,
        GPIO44: peripherals.GPIO44,
        GPIO46: peripherals.GPIO46,
    };
    (platform, board)
}

/// Runs the repeater with the board's CLI serial port, SX1262 configuration
/// and radio frontend.
pub async fn run<S, F>(
    platform: Platform,
    board_config: BoardConfig,
    cli_serial: Option<S>,
    frontend: F,
) -> !
where
    S: CliSerial,
    F: RadioFrontend,
{
    let Platform {
        radio_spi,
        radio_sck,
        radio_mosi,
        radio_miso,
        radio_cs,
        radio_reset,
        radio_dio1,
        radio_busy,
        display_i2c,
        display_sda,
        display_scl,
        display_reset,
        display_power,
        button,
        rng,
        mut adc,
        battery_sense,
        battery_ctrl,
        wifi,
    } = platform;

    let spi = match Spi::new(
        radio_spi,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(4))
            .with_mode(Mode::_0),
    ) {
        Ok(spi) => spi
            .with_sck(radio_sck)
            .with_mosi(radio_mosi)
            .with_miso(radio_miso),
        Err(e) => {
            error!("Failed to configure sx1262 SPI bus: {}", e);
            esp::idle_loop();
        }
    };

    let output_config = OutputConfig::default();
    let input_config = InputConfig::default();
    let display = match I2c::new(
        display_i2c,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    ) {
        Ok(i2c) => {
            let power = Output::new(display_power, Level::Low, output_config);
            let reset = Output::new(display_reset, Level::High, output_config);
            let i2c = i2c.with_sda(display_sda).with_scl(display_scl);

            Some(Display::new_with_power_active_level(
                i2c,
                reset,
                power,
                PowerActiveLevel::Low,
            ))
        }
        Err(e) => {
            error!("Failed to configure OLED I2C bus: {:?}", e);
            None
        }
    };

    let prg_button = Input::new(button, InputConfig::default().with_pull(Pull::Up));
    let identity_seed = heltec::generate_identity_seed(rng, adc.reborrow());
    let mut battery = {
        let mut adc_config = AdcConfig::new();
        let sense =
            adc_config.enable_pin_with_cal::<_, AdcCalCurve<_>>(battery_sense, Attenuation::_0dB);
        let adc = Adc::new(adc, adc_config);
        let mut ctrl = Flex::new(battery_ctrl);
        ctrl.apply_input_config(&input_config);
        ctrl.set_input_enable(true);
        let ctrl_active_level = if ctrl.is_high() {
            Level::Low
        } else {
            Level::High
        };
        let ctrl_inactive_level = heltec::opposite_level(ctrl_active_level);
        ctrl.set_level(ctrl_inactive_level);
        ctrl.apply_output_config(&output_config);
        ctrl.set_output_enable(true);
        ctrl.set_input_enable(false);
        BatteryMonitor {
            adc,
            sense,
            ctrl,
            ctrl_active_level,
            ctrl_inactive_level,
        }
    };

    let radio = RadioResources {
        spi,
        cs: Output::new(radio_cs, Level::High, output_config),
        reset: Output::new(radio_reset, Level::High, output_config),
        dio1: Input::new(radio_dio1, input_config),
        busy: Input::new(radio_busy, input_config),
        board_config,
        frontend,
    };

    let wifi = WifiResources { wifi };

    let mut storage = esp::init_storage(STORAGE_LAYOUT);
    let defaults = AppConfig::generated_defaults(identity_seed);
    let config = AppConfig::load_or_create(&mut storage, defaults);
    let context = AppContext::new(EspPlatform {}, config, storage, MEMORY_PROFILE);
    heltec::log_memory_profile(MEMORY_PROFILE);
    heltec::log_effective_config(&context).await;
    heltec::publish_battery_sample(&mut battery, &context).await;

    heltec::run_board_tasks(
        display, prg_button, battery, radio, cli_serial, wifi, context,
    )
    .await
}
