//! The repeater platform shared by the Heltec ESP32-S3 boards. A board supplies
//! only what differs: its console, its SX1262 configuration and its radio
//! frontend.
use defmt::{debug, error};
use embedded_io_async::{Read, Write};
use esp_hal::{
    Blocking,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
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
    app::config::RadioConfig,
    memory::MemoryProfile,
    modules::{
        spi_device::BlockingExclusiveSpiDevice,
        ssd1306::{self, PowerActiveLevel},
        sx1262,
    },
    platform::Platform,
};

pub use mcrs_repeater::modules::sx1262::BoardConfig;

use crate::{
    board::heltec::{
        self, BatteryMonitor, BoardRadio, MEMORY_PROFILE, RadioFrontend, STORAGE_LAYOUT,
        WifiResources,
    },
    esp::{self, EspStorage, RadioDelay, SpiDelay},
};

type RadioReceiver = sx1262::Receiver<
    BlockingExclusiveSpiDevice<Spi<'static, Blocking>, Output<'static>, SpiDelay>,
    Output<'static>,
    Input<'static>,
    RadioDelay,
>;
type OledDisplay = ssd1306::Display<I2c<'static, Blocking>, Output<'static>, Output<'static>>;

/// The peripherals that every Heltec board wires the same way.
pub struct CommonPeripherals {
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

/// The peripherals left for the board's console and radio frontend.
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
pub fn init() -> (CommonPeripherals, BoardPeripherals) {
    let peripherals = esp::init();
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let software_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, software_interrupt.software_interrupt0);

    let common = CommonPeripherals {
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
    (common, board)
}

/// A Heltec ESP32-S3 board, ready to run a [`mcrs_repeater::Repeater`].
pub struct EspPlatform<C, F> {
    model: &'static str,
    identity_seed: [u8; 32],
    storage: Option<EspStorage>,
    radio: Option<RadioParts<F>>,
    console: Option<C>,
    display: Option<OledDisplay>,
    button: Option<Input<'static>>,
    battery: Option<BatteryMonitor>,
    /// Reserved for the Wi-Fi tasks.
    _wifi: WifiResources,
}

/// The SX1262 and its frontend, until the repeater starts the radio.
struct RadioParts<F> {
    spi: Spi<'static, Blocking>,
    cs: Output<'static>,
    reset: Output<'static>,
    dio1: Input<'static>,
    busy: Input<'static>,
    board_config: BoardConfig,
    frontend: F,
}

impl<C, F> EspPlatform<C, F>
where
    C: Read + Write,
    F: RadioFrontend,
{
    /// Sets up the shared hardware. `model` names the board in MQTT status
    /// reports.
    pub fn new(
        peripherals: CommonPeripherals,
        model: &'static str,
        board_config: BoardConfig,
        console: Option<C>,
        frontend: F,
    ) -> Self {
        let CommonPeripherals {
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
        } = peripherals;

        let output_config = OutputConfig::default();
        let input_config = InputConfig::default();

        let radio = match Spi::new(
            radio_spi,
            SpiConfig::default()
                .with_frequency(Rate::from_mhz(4))
                .with_mode(Mode::_0),
        ) {
            Ok(spi) => Some(RadioParts {
                spi: spi
                    .with_sck(radio_sck)
                    .with_mosi(radio_mosi)
                    .with_miso(radio_miso),
                cs: Output::new(radio_cs, Level::High, output_config),
                reset: Output::new(radio_reset, Level::High, output_config),
                dio1: Input::new(radio_dio1, input_config),
                busy: Input::new(radio_busy, input_config),
                board_config,
                frontend,
            }),
            Err(e) => {
                error!("Failed to configure sx1262 SPI bus: {}", e);
                None
            }
        };

        let display = match I2c::new(
            display_i2c,
            I2cConfig::default().with_frequency(Rate::from_khz(400)),
        ) {
            Ok(i2c) => {
                let power = Output::new(display_power, Level::Low, output_config);
                let reset = Output::new(display_reset, Level::High, output_config);
                let i2c = i2c.with_sda(display_sda).with_scl(display_scl);

                Some(ssd1306::Display::new_with_power_active_level(
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

        let button = Input::new(button, InputConfig::default().with_pull(Pull::Up));
        let identity_seed = heltec::generate_identity_seed(rng, adc.reborrow());
        let battery = BatteryMonitor::new(adc, battery_sense, battery_ctrl);

        Self {
            model,
            identity_seed,
            storage: Some(esp::init_storage(STORAGE_LAYOUT)),
            radio,
            console,
            display,
            button: Some(button),
            battery: Some(battery),
            _wifi: WifiResources { wifi },
        }
    }
}

impl<C, F> Platform for EspPlatform<C, F>
where
    C: Read + Write,
    F: RadioFrontend,
{
    type Storage = EspStorage;
    type Radio = BoardRadio<RadioReceiver, F>;
    type Console = C;
    type Display = OledDisplay;
    type Button = Input<'static>;
    type Battery = BatteryMonitor;
    type Delay = RadioDelay;

    fn model(&self) -> &'static str {
        self.model
    }

    fn memory_profile(&self) -> MemoryProfile {
        MEMORY_PROFILE
    }

    fn identity_seed(&mut self) -> [u8; 32] {
        self.identity_seed
    }

    fn take_storage(&mut self) -> Self::Storage {
        self.storage.take().expect("storage is taken once")
    }

    async fn start_radio(&mut self, config: RadioConfig) -> Result<Self::Radio, ()> {
        let Some(RadioParts {
            spi,
            cs,
            reset,
            dio1,
            busy,
            board_config,
            frontend,
        }) = self.radio.take()
        else {
            return Err(());
        };
        let spi = BlockingExclusiveSpiDevice::new(spi, cs, esp::spi_delay());

        match sx1262::init_receiver(
            spi,
            reset,
            dio1,
            busy,
            esp::radio_delay(),
            board_config,
            config.receive_config(),
        )
        .await
        {
            Ok(receiver) => {
                debug!("radio initialized");
                Ok(BoardRadio::new(receiver, frontend))
            }
            Err(e) => {
                error!("failed to initialize sx1262: {:?}", e);
                Err(())
            }
        }
    }

    fn take_console(&mut self) -> Option<Self::Console> {
        self.console.take()
    }

    fn take_display(&mut self) -> Option<Self::Display> {
        self.display.take()
    }

    fn take_button(&mut self) -> Option<Self::Button> {
        self.button.take()
    }

    fn take_battery(&mut self) -> Option<Self::Battery> {
        self.battery.take()
    }

    fn delay(&self) -> Self::Delay {
        esp::radio_delay()
    }

    fn now_millis(&self) -> u64 {
        esp::now_millis()
    }

    fn now_seconds(&self) -> u32 {
        esp::now_seconds()
    }

    fn set_wall_clock_if_forward(&self, unix_seconds: u32) -> bool {
        esp::set_wall_clock_if_forward(unix_seconds)
    }

    fn random_u32(&self) -> u32 {
        esp::random_u32()
    }

    fn reboot(&self) -> ! {
        esp::reboot()
    }
}
