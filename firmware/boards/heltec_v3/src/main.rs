#![no_std]
#![no_main]

use defmt::error;
use esp_hal::uart::{Config as UartConfig, Uart};
use mcrs_esp32::{board::heltec::NoRadioFrontend, platform::BoardConfig};

const MODEL: &str = "Heltec V3";

// #[cfg(feature = "board-heltec-wsl3")]
// const MODEL: &str = "Heltec Wireless Stick Lite V3";

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let (platform, board) = mcrs_esp32::platform::init();

    let cli_serial = match Uart::new(board.UART0, UartConfig::default()) {
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

    mcrs_esp32::platform::run(
        platform,
        BoardConfig::heltec_v3(),
        cli_serial,
        NoRadioFrontend,
    )
    .await
}
