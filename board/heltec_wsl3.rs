#![no_std]
#![no_main]

use defmt::{error, info};
use embassy_futures::select::select;
use esp_hal::uart::{Config as UartConfig, Uart};
use mcrs_esp32s3::{
    BoardConfig, Devices, Repeater,
    heltec::{NoRadioFrontend, wifi_task},
    idle_loop,
};

const MODEL: &str = "Heltec Wireless Stick Lite V3";
// Provisional values; validate against this board's battery and regulator.
const STARTUP_MIN_MV: u16 = 3300;
const VOLTAGE_WARNING_MV: u16 = 3000;

#[esp_rtos::main]
async fn main(_spawner: embassy_executor::Spawner) -> ! {
    let (peripherals, board) = mcrs_esp32s3::init();

    let (platform, storage, mut devices) = Devices::new(
        peripherals,
        MODEL,
        BoardConfig::heltec_v3(),
        STARTUP_MIN_MV,
        async { NoRadioFrontend },
    )
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
