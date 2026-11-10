#![no_std]
#![no_main]

use defmt::info;
use embassy_futures::select::select;
use embedded_hal_async::delay::DelayNs;
use esp_hal::{
    gpio::{Flex, InputConfig, Level, Output, OutputConfig},
    usb_serial_jtag::UsbSerialJtag,
};
use mcrs_esp32s3::{
    BoardConfig, Devices, Repeater,
    heltec::{RadioFrontend, RadioOutput, wifi_task},
    radio_delay,
};

const MODEL: &str = "Heltec V4";
// Provisional values; validate against this board's battery and regulator.
const STARTUP_MIN_MV: u16 = 3300;
const VOLTAGE_WARNING_MV: u16 = 3000;

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let (peripherals, board) = mcrs_esp32s3::init();

    let (platform, storage, mut devices) = Devices::new(
        peripherals,
        MODEL,
        BoardConfig::heltec_v4(),
        STARTUP_MIN_MV,
        async {
            let output_config = OutputConfig::default();
            HeltecV4Frontend::detect(
                Output::new(board.GPIO7, Level::Low, output_config),
                Flex::new(board.GPIO2),
                Output::new(board.GPIO5, Level::Low, output_config),
                Output::new(board.GPIO46, Level::Low, output_config),
                &InputConfig::default(),
            )
            .await
        },
    )
    .await;
    let mut console = UsbSerialJtag::new(board.USB_DEVICE).into_async();

    {
        let repeater = Repeater::new(
            platform,
            storage,
            &mut devices.radio,
            &mut console,
            &mut devices.display,
            &mut devices.button,
            &mut devices.battery,
        );
        let context = repeater.context();
        select(
            repeater.run(VOLTAGE_WARNING_MV),
            wifi_task(&mut devices.wifi, context),
        )
        .await;
    } // Both futures are dropped; main owns the devices again.

    info!("Entering low-power recovery");
    devices.shutdown().await;
    mcrs_esp32s3::deep_sleep(mcrs_esp32s3::RECOVERY_SLEEP)
}

struct HeltecV4Frontend {
    pa_power: RadioOutput,
    shared_enable: Flex<'static>,
    kct_ctx: RadioOutput,
    gc_tx_enable: RadioOutput,
    kind: HeltecV4FrontendKind,
}

#[derive(Clone, Copy)]
enum HeltecV4FrontendKind {
    Gc1109,
    Kct8103l,
}

impl HeltecV4Frontend {
    async fn detect(
        mut pa_power: RadioOutput,
        mut shared_enable: Flex<'static>,
        kct_ctx: RadioOutput,
        gc_tx_enable: RadioOutput,
        input_config: &InputConfig,
    ) -> Self {
        let _ = pa_power.set_high();
        let mut delay = radio_delay();
        delay.delay_ms(1).await;
        shared_enable.apply_input_config(input_config);
        shared_enable.set_input_enable(true);
        delay.delay_ms(1).await;

        let kind = if shared_enable.is_high() {
            HeltecV4FrontendKind::Kct8103l
        } else {
            HeltecV4FrontendKind::Gc1109
        };
        let mut frontend = Self::new(pa_power, shared_enable, kct_ctx, gc_tx_enable, kind);
        frontend.shutdown();
        match kind {
            HeltecV4FrontendKind::Gc1109 => info!("Radio frontend: Heltec v4 GC1109"),
            HeltecV4FrontendKind::Kct8103l => info!("Radio frontend: Heltec v4.3 KCT8103L"),
        }
        frontend
    }

    fn new(
        pa_power: RadioOutput,
        mut shared_enable: Flex<'static>,
        kct_ctx: RadioOutput,
        gc_tx_enable: RadioOutput,
        kind: HeltecV4FrontendKind,
    ) -> Self {
        shared_enable.set_level(Level::Low);
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

impl RadioFrontend for HeltecV4Frontend {
    fn shutdown(&mut self) {
        let _ = self.gc_tx_enable.set_low();
        let _ = self.kct_ctx.set_low();
        self.shared_enable.set_level(Level::Low);
        let _ = self.pa_power.set_low();
    }

    fn set_rx_mode(&mut self) {
        let _ = self.pa_power.set_high();
        self.shared_enable.set_level(Level::High);
        match self.kind {
            HeltecV4FrontendKind::Gc1109 => {
                let _ = self.gc_tx_enable.set_low();
            }
            HeltecV4FrontendKind::Kct8103l => {
                let _ = self.kct_ctx.set_low();
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
