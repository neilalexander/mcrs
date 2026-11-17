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

    let recovery_interval;
    {
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
        select(repeater.run(true), wifi_task(&mut devices.wifi, context)).await;
        recovery_interval = context
            .with_config(|config| config.lowvoltage().interval_seconds)
            .await;
    } // Both futures are dropped; main owns the devices again.

    info!("Entering low-power recovery");
    devices.shutdown().await;
    mcrs_esp32s3::deep_sleep(core::time::Duration::from_secs(u64::from(
        recovery_interval,
    )))
}
