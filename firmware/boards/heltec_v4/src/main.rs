#![no_std]
#![no_main]

use defmt::info;
use embedded_hal_async::delay::DelayNs;
use esp_hal::{
    Async,
    gpio::{Flex, InputConfig, Level, Output, OutputConfig},
    usb_serial_jtag::UsbSerialJtag,
};
use mcrs_esp32::{
    CliSerial,
    board::heltec::{RadioFrontend, RadioOutput},
    esp::radio_delay,
    platform::BoardConfig,
};

//    #[cfg(feature = "board-heltec-v4")]
//     const MODEL: &str = "Heltec V4";

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let (platform, board) = mcrs_esp32::platform::init();

    let cli_serial = UsbSerial(UsbSerialJtag::new(board.USB_DEVICE).into_async());

    let output_config = OutputConfig::default();
    let frontend = HeltecV4Frontend::detect(
        Output::new(board.GPIO7, Level::High, output_config),
        Flex::new(board.GPIO2),
        Output::new(board.GPIO5, Level::High, output_config),
        Output::new(board.GPIO46, Level::Low, output_config),
        &InputConfig::default(),
    )
    .await;

    mcrs_esp32::platform::run(
        platform,
        BoardConfig::heltec_v4(),
        Some(cli_serial),
        frontend,
    )
    .await
}

/// The CLI on the USB serial/JTAG port.
struct UsbSerial(UsbSerialJtag<'static, Async>);

impl CliSerial for UsbSerial {
    type Error = <UsbSerialJtag<'static, Async> as embedded_io_async::ErrorType>::Error;

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        embedded_io_async::Read::read(&mut self.0, buf).await
    }
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
        pa_power: RadioOutput,
        mut shared_enable: Flex<'static>,
        kct_ctx: RadioOutput,
        gc_tx_enable: RadioOutput,
        input_config: &InputConfig,
    ) -> Self {
        shared_enable.apply_input_config(input_config);
        shared_enable.set_input_enable(true);
        let mut delay = radio_delay();
        delay.delay_ms(1).await;

        let kind = if shared_enable.is_high() {
            HeltecV4FrontendKind::Kct8103l
        } else {
            HeltecV4FrontendKind::Gc1109
        };
        let mut frontend = Self::new(pa_power, shared_enable, kct_ctx, gc_tx_enable, kind);
        frontend.set_rx_mode();
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

impl RadioFrontend for HeltecV4Frontend {
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
