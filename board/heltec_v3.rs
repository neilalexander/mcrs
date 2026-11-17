#![no_std]
#![no_main]

use defmt::error;
use esp_hal::uart::{Config as UartConfig, Uart};
use mcrs_esp32s3::{
    BoardConfig, Devices, Repeater,
    heltec::{NoRadioFrontend, wifi_task},
    idle_loop,
};

const MODEL: &str = "Heltec V3";

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let (peripherals, board) = mcrs_esp32s3::init();

    let (platform, storage, mut devices) =
        Devices::new(peripherals, MODEL, BoardConfig::heltec_v3(), async {
            NoRadioFrontend
        })
        .await;
    let mut console = match Uart::new(board.UART0, UartConfig::default()) {
        Ok(uart) => uart
            .with_rx(board.GPIO44)
            .with_tx(board.GPIO43)
            .into_async(),
        Err(e) => {
            error!("Failed to configure CLI UART: {:?}", e);
            idle_loop();
        }
    };

    let repeater = Repeater::new(
        platform,
        storage,
        &mut devices.radio,
        &mut console,
        &mut devices.display,
        &mut devices.button,
        &mut devices.battery,
        &mut devices.sensors,
    );
    let context = repeater.context();
    let wifi_task = wifi_task(&mut devices.wifi, context);
    let sleep_request = repeater.run(true, wifi_task).await;
    let lowvoltage = context.with_config(|config| config.lowvoltage()).await;
    devices.sleep(sleep_request, lowvoltage).await
}
