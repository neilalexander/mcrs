use esp_hal::{
    analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation},
    gpio::{Flex, Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    rng::Rng,
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
    timer::timg::TimerGroup,
    uart::{Config as UartConfig, Uart},
};

use embedded_hal_async::delay::DelayNs as _;

use crate::board::heltec;
use crate::board::heltec::RadioFrontend as _;

struct HeltecV4Frontend {
    pa_power: heltec::RadioOutput,
    shared_enable: Flex<'static>,
    kct_ctx: heltec::RadioOutput,
    gc_tx_enable: heltec::RadioOutput,
    kind: HeltecV4FrontendKind,
}

#[derive(Clone, Copy)]
enum HeltecV4FrontendKind {
    Gc1109,
    Kct8103l,
}

impl HeltecV4Frontend {
    async fn detect(
        pa_power: heltec::RadioOutput,
        mut shared_enable: Flex<'static>,
        kct_ctx: heltec::RadioOutput,
        gc_tx_enable: heltec::RadioOutput,
        input_config: &InputConfig,
    ) -> Self {
        shared_enable.apply_input_config(input_config);
        shared_enable.set_input_enable(true);
        let mut delay = crate::platform::radio_delay();
        delay.delay_ms(1).await;

        let kind = if shared_enable.is_high() {
            HeltecV4FrontendKind::Kct8103l
        } else {
            HeltecV4FrontendKind::Gc1109
        };
        let mut frontend = Self::new(pa_power, shared_enable, kct_ctx, gc_tx_enable, kind);
        frontend.set_rx_mode();
        match kind {
            HeltecV4FrontendKind::Gc1109 => {
                crate::platform::log_fmt(format_args!("Radio frontend: Heltec v4 GC1109"))
            }
            HeltecV4FrontendKind::Kct8103l => {
                crate::platform::log_fmt(format_args!("Radio frontend: Heltec v4.3 KCT8103L"))
            }
        }
        frontend
    }

    fn new(
        pa_power: heltec::RadioOutput,
        mut shared_enable: Flex<'static>,
        kct_ctx: heltec::RadioOutput,
        gc_tx_enable: heltec::RadioOutput,
        kind: HeltecV4FrontendKind,
    ) -> Self {
        shared_enable.set_level(Level::High);
        shared_enable.apply_output_config(&OutputConfig::default());
        shared_enable.set_output_enable(true);
        shared_enable.set_input_enable(false);

        Self {
            pa_power,
            shared_enable,
            kct_ctx,
            gc_tx_enable,
            kind,
        }
    }
}

impl heltec::RadioFrontend for HeltecV4Frontend {
    fn set_rx_mode(&mut self) {
        let _ = self.pa_power.set_high();
        self.shared_enable.set_level(Level::High);
        match self.kind {
            HeltecV4FrontendKind::Gc1109 => {
                let _ = self.gc_tx_enable.set_low();
            }
            HeltecV4FrontendKind::Kct8103l => {
                let _ = self.kct_ctx.set_high();
            }
        }
    }

    fn set_tx_mode(&mut self) {
        let _ = self.pa_power.set_high();
        self.shared_enable.set_level(Level::High);
        match self.kind {
            HeltecV4FrontendKind::Gc1109 => {
                let _ = self.gc_tx_enable.set_high();
            }
            HeltecV4FrontendKind::Kct8103l => {
                let _ = self.kct_ctx.set_high();
            }
        }
    }
}

#[esp_hal_embassy::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let platform = crate::platform::init();

    init(platform).await
}

async fn init(platform: crate::platform::Platform) -> ! {
    let timg0 = TimerGroup::new(platform.peripherals.TIMG0);
    let ota_timer = timg0.timer1;
    esp_hal_embassy::init(timg0.timer0);

    let spi = match Spi::new(
        platform.peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(4))
            .with_mode(Mode::_0),
    ) {
        Ok(spi) => spi
            .with_sck(platform.peripherals.GPIO9)
            .with_mosi(platform.peripherals.GPIO10)
            .with_miso(platform.peripherals.GPIO11),
        Err(_) => {
            crate::platform::log_radio_spi_config_failed();
            crate::platform::idle_loop();
        }
    };

    let output_config = OutputConfig::default();
    let input_config = InputConfig::default();
    let display = match I2c::new(
        platform.peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    ) {
        Ok(i2c) => {
            let power = Output::new(platform.peripherals.GPIO36, Level::High, output_config);
            let reset = Output::new(platform.peripherals.GPIO21, Level::High, output_config);
            let i2c = i2c
                .with_sda(platform.peripherals.GPIO17)
                .with_scl(platform.peripherals.GPIO18);

            Some(
                crate::modules::ssd1306::Display::new_with_power_active_level(
                    i2c,
                    reset,
                    power,
                    crate::modules::ssd1306::PowerActiveLevel::High,
                ),
            )
        }
        Err(_) => {
            crate::platform::log_display_i2c_config_failed();
            None
        }
    };

    let prg_button = Input::new(
        platform.peripherals.GPIO0,
        InputConfig::default().with_pull(Pull::Up),
    );
    let mut battery = {
        let mut adc_config = AdcConfig::new();
        let sense = adc_config.enable_pin_with_cal::<_, AdcCalCurve<_>>(
            platform.peripherals.GPIO1,
            Attenuation::_0dB,
        );
        let adc = Adc::new(platform.peripherals.ADC1, adc_config);
        let mut ctrl = Flex::new(platform.peripherals.GPIO37);
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
        heltec::BatteryMonitor {
            adc,
            sense,
            ctrl,
            ctrl_active_level,
            ctrl_inactive_level,
        }
    };

    let radio_frontend = HeltecV4Frontend::detect(
        Output::new(platform.peripherals.GPIO7, Level::High, output_config),
        Flex::new(platform.peripherals.GPIO2),
        Output::new(platform.peripherals.GPIO5, Level::High, output_config),
        Output::new(platform.peripherals.GPIO46, Level::Low, output_config),
        &input_config,
    )
    .await;

    let radio = heltec::RadioResources {
        spi,
        cs: Output::new(platform.peripherals.GPIO8, Level::High, output_config),
        reset: Output::new(platform.peripherals.GPIO12, Level::High, output_config),
        dio1: Input::new(platform.peripherals.GPIO14, input_config),
        busy: Input::new(platform.peripherals.GPIO13, input_config),
        board_config: crate::modules::sx1262::BoardConfig::heltec_v4(),
        frontend: radio_frontend,
    };

    let cli_serial = match Uart::new(platform.peripherals.UART0, UartConfig::default()) {
        Ok(uart) => Some(
            uart.with_rx(platform.peripherals.GPIO44)
                .with_tx(platform.peripherals.GPIO43)
                .into_async(),
        ),
        Err(_) => {
            crate::platform::log_cli_uart_config_failed();
            None
        }
    };

    let mut rng = Rng::new(platform.peripherals.RNG);
    let identity_seed = heltec::generate_identity_seed(&mut rng);
    let ota = heltec::OtaResources {
        timer: ota_timer,
        rng,
        wifi: platform.peripherals.WIFI,
    };

    let mut storage = crate::platform::init_storage(crate::board::STORAGE_LAYOUT);
    let defaults = crate::app::config::AppConfig::generated_defaults(identity_seed);
    let config = crate::app::config::AppConfig::load_or_create(&mut storage, defaults);
    let context = crate::app::AppContext::new(config, storage, crate::board::MEMORY_PROFILE);
    heltec::log_memory_profile(crate::board::MEMORY_PROFILE);
    heltec::log_effective_config(&context).await;
    heltec::publish_battery_sample(&mut battery, &context).await;

    heltec::run_board_tasks(
        display, prg_button, battery, radio, cli_serial, ota, context,
    )
    .await
}
