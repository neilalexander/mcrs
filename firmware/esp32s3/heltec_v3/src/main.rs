#![no_std]
#![no_main]

use defmt::error;
use esp_hal::uart::{Config as UartConfig, Uart};
use mcrs_esp32s3::{EspPlatform, Repeater, board::heltec::NoRadioFrontend, platform::BoardConfig};

const MODEL: &str = "Heltec V3";

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let (peripherals, board) = mcrs_esp32s3::platform::init();

    let console = match Uart::new(board.UART0, UartConfig::default()) {
        Ok(uart) => Some(
            uart.with_rx(board.GPIO44)
                .with_tx(board.GPIO43)
                .into_async(),
        ),
        Err(e) => {
            error!("Failed to configure CLI UART: {:?}", e);
            None
        }
    };

    let platform = EspPlatform::new(
        peripherals,
        MODEL,
        BoardConfig::heltec_v3(),
        console,
        NoRadioFrontend,
    );
    Repeater::new(platform).run().await
}
