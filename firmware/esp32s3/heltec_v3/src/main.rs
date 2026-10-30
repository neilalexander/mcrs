#![no_std]
#![no_main]

use defmt::error;
use embassy_futures::join::join;
use esp_hal::uart::{Config as UartConfig, Uart};
use mcrs_esp32s3::board::heltec::wifi_task;
use mcrs_esp32s3::{
    Repeater,
    board::heltec::NoRadioFrontend,
    esp::idle_loop,
    platform::{BoardConfig, Devices},
};

const MODEL: &str = "Heltec V3";

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let (peripherals, board) = mcrs_esp32s3::platform::init();

    let console = match Uart::new(board.UART0, UartConfig::default()) {
        Ok(uart) => uart
            .with_rx(board.GPIO44)
            .with_tx(board.GPIO43)
            .into_async(),
        Err(e) => {
            error!("Failed to configure CLI UART: {:?}", e);
            idle_loop();
        }
    };

    let devices = Devices::new(
        peripherals,
        MODEL,
        BoardConfig::heltec_v3(),
        NoRadioFrontend,
    );
    let repeater = Repeater::new(
        devices.platform,
        devices.storage,
        devices.radio,
        console,
        devices.display,
        devices.button,
        devices.battery,
    );
    let context = repeater.context();
    let (never, _) = join(repeater.run(), wifi_task(devices.wifi, context)).await;
    never
}
