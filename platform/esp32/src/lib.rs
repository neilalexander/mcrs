//! The repeater platform and devices shared by the Heltec ESP32-S3 boards. A
//! board supplies only what differs: its console, its SX1262 configuration and
//! its radio frontend.
#![no_std]

extern crate alloc;

pub use mcrs_repeater::Repeater;

pub mod heltec;

mod esp;
mod littlefs;
pub(crate) mod ota;
mod spi_device;

#[cfg(feature = "mqtt")]
pub(crate) mod rng;

pub use esp::*;

/// Recheck the battery after five minutes in deep sleep.
pub const RECOVERY_SLEEP: core::time::Duration = core::time::Duration::from_secs(5 * 60);

use crate::{
    heltec::{
        BatteryMonitor, BoardRadio, MEMORY_PROFILE, RadioFrontend, STORAGE_LAYOUT, WifiResources,
    },
    spi_device::BlockingExclusiveSpiDevice,
};
use core::{
    cell::{Cell, RefCell},
    net::SocketAddrV4,
    task::Waker,
};
use defmt::{error, info};
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
    memory::MemoryProfile,
    platform::{Battery, OtaStatus, Platform, Radio},
};
use mcrs_ssd1306::PowerActiveLevel;
use mcrs_sx1262::Sx1262;

pub use mcrs_sx1262::BoardConfig;

/// The SX1262, with the board's frontend switched to match each operation.
pub type HeltecRadio<F> = BoardRadio<
    Sx1262<
        BlockingExclusiveSpiDevice<Spi<'static, Blocking>, Output<'static>, SpiDelay>,
        Output<'static>,
        Input<'static>,
        RadioDelay,
    >,
    F,
>;
pub type OledDisplay =
    mcrs_ssd1306::Display<I2c<'static, Blocking>, Output<'static>, Output<'static>>;

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
/// the shared devices and the board.
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

/// Owns the board devices across normal operation and shutdown.
pub struct Devices<F> {
    pub radio: HeltecRadio<F>,
    pub display: OledDisplay,
    pub button: Input<'static>,
    pub battery: BatteryMonitor<'static>,
    /// Reserved for the Wi-Fi tasks.
    pub wifi: WifiResources,
}

impl<F: RadioFrontend> Devices<F> {
    /// Shuts down externally powered devices after normal tasks have stopped.
    /// Drop the Wi-Fi task first so it releases its controller.
    /// Attempts every device, logging failures before the board enters sleep.
    pub async fn shutdown(&mut self) {
        self.battery.shutdown();
        if self.display.sleep().is_err() {
            error!("Display shutdown failed");
        }
        match embassy_time::with_timeout(
            embassy_time::Duration::from_secs(2),
            self.radio.shutdown(),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(())) => error!("Radio shutdown failed"),
            Err(_) => error!("Radio shutdown timed out"),
        }
    }

    /// Sets up the shared hardware. `model` names the board in MQTT status
    /// reports. Battery sensing runs first if minimum_mv is above zero.
    pub async fn new(
        peripherals: CommonPeripherals,
        model: &'static str,
        board_config: BoardConfig,
        minimum_mv: u16,
        frontend: impl core::future::Future<Output = F>,
    ) -> (EspPlatform, EspStorage, Self) {
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
            mut battery_sense,
            mut battery_ctrl,
            wifi,
        } = peripherals;

        let output_config = OutputConfig::default();
        let input_config = InputConfig::default();

        // Keep the display rail off and NSS high while checking the battery.
        // Do not initialise SPI, I2C, the frontend, entropy or storage yet.
        let display_power = Output::new(display_power, Level::High, output_config);
        let radio_cs = Output::new(radio_cs, Level::High, output_config);
        if minimum_mv > 0 {
            let mut battery = BatteryMonitor::new(
                adc.reborrow(),
                battery_sense.reborrow(),
                battery_ctrl.reborrow(),
            );
            let voltage = battery.millivolts().await;
            match voltage {
                Some(mv) => info!("Startup battery: {}mV (minimum {}mV)", mv, minimum_mv),
                None => info!("Startup battery: voltage unavailable"),
            }
            battery.shutdown();
            if !voltage.is_some_and(|mv| mv >= minimum_mv) {
                deep_sleep(RECOVERY_SLEEP);
            }
        }
        // ADC1 is free again for the entropy source before the permanent
        // battery monitor takes ownership. The frontend is constructed now.
        let identity_seed = heltec::generate_identity_seed(rng, adc.reborrow());
        let frontend = frontend.await;

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

        let display = match I2c::new(
            display_i2c,
            I2cConfig::default().with_frequency(Rate::from_khz(400)),
        ) {
            Ok(i2c) => {
                let power = display_power;
                let reset = Output::new(display_reset, Level::High, output_config);
                let i2c = i2c.with_sda(display_sda).with_scl(display_scl);

                mcrs_ssd1306::Display::new_with_power_active_level(
                    i2c,
                    reset,
                    power,
                    PowerActiveLevel::Low,
                )
            }
            Err(e) => {
                error!("Failed to configure OLED I2C bus: {:?}", e);
                esp::idle_loop();
            }
        };

        let platform = EspPlatform::new(model, identity_seed);
        let storage = esp::init_storage(STORAGE_LAYOUT);
        let devices = Self {
            radio: BoardRadio::new(
                Sx1262::new(
                    BlockingExclusiveSpiDevice::new(spi, radio_cs, esp::spi_delay()),
                    Output::new(radio_reset, Level::High, output_config),
                    Input::new(radio_dio1, input_config),
                    Input::new(radio_busy, input_config),
                    esp::radio_delay(),
                    board_config,
                ),
                frontend,
            ),
            display,
            button: Input::new(button, InputConfig::default().with_pull(Pull::Up)),
            battery: BatteryMonitor::new(adc, battery_sense, battery_ctrl),
            wifi: WifiResources { wifi },
        };
        (platform, storage, devices)
    }
}

/// The services every Heltec board provides.
pub struct EspPlatform {
    model: &'static str,
    identity_seed: [u8; 32],
    /// Whether the OTA update page is wanted. The CLI sets it through
    /// [`Platform::start_ota`] and [`Platform::stop_ota`], and the Wi-Fi task
    /// serves the page while it's set.
    ota_requested: Cell<bool>,
    /// Where the Wi-Fi task serves the page, once it has an address.
    ota_address: Cell<Option<SocketAddrV4>>,
    /// Wakes the Wi-Fi task when the request changes.
    ota_waker: RefCell<Option<Waker>>,
    /// Counts the changes, so the Wi-Fi task notices a stop and a start
    /// between its polls.
    ota_generation: Cell<u32>,
}

impl EspPlatform {
    /// `model` names the board in MQTT status reports, and `identity_seed`
    /// seeds the identity created on first boot.
    pub fn new(model: &'static str, identity_seed: [u8; 32]) -> Self {
        Self {
            model,
            identity_seed,
            ota_requested: Cell::new(false),
            ota_address: Cell::new(None),
            ota_waker: RefCell::new(None),
            ota_generation: Cell::new(0),
        }
    }

    pub(crate) fn ota_requested(&self) -> bool {
        self.ota_requested.get()
    }

    pub(crate) fn ota_generation(&self) -> u32 {
        self.ota_generation.get()
    }

    pub(crate) fn register_ota_waker(&self, waker: &Waker) {
        self.ota_waker.borrow_mut().replace(waker.clone());
    }

    pub(crate) fn set_ota_address(&self, address: Option<SocketAddrV4>) {
        self.ota_address.set(address);
    }

    /// Starting again doesn't restart the page, so an upload in progress
    /// carries on.
    fn request_ota(&self, requested: bool) {
        if self.ota_requested.replace(requested) == requested {
            return;
        }
        self.ota_generation
            .set(self.ota_generation.get().wrapping_add(1));
        if let Some(waker) = self.ota_waker.borrow_mut().take() {
            waker.wake();
        }
    }
}

impl Platform for EspPlatform {
    type Delay = RadioDelay;

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

    fn start_ota(&self) {
        self.request_ota(true);
    }

    fn stop_ota(&self) {
        self.request_ota(false);
    }

    fn ota_status(&self) -> OtaStatus {
        match (self.ota_requested.get(), self.ota_address.get()) {
            (false, _) => OtaStatus::Stopped,
            (true, None) => OtaStatus::Waiting,
            (true, Some(address)) => OtaStatus::Serving(address),
        }
    }
}
