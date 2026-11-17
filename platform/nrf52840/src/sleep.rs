use embassy_nrf::pac::{CLOCK, POWER};
use embassy_time::{Duration, Timer};
use mcrs_repeater::{
    app::config::{AppConfig, LowVoltageConfig},
    platform::{Battery, Radio, Storage},
};

pub async fn check_startup_voltage(battery: &mut impl Battery, storage: &mut impl Storage) {
    let config = AppConfig::load_lowvoltage(storage);
    if config.enabled
        && !battery
            .millivolts()
            .await
            .is_some_and(|mv| mv >= config.wake_mv)
    {
        recover(battery, config).await;
    }
}

pub async fn recover(battery: &mut impl Battery, config: LowVoltageConfig) -> ! {
    CLOCK.tasks_hfclkstop().write_value(1);
    POWER.tasks_lowpwr().write_value(1);
    loop {
        Timer::after(Duration::from_secs(u64::from(config.interval_seconds))).await;
        if !config.enabled
            || battery
                .millivolts()
                .await
                .is_some_and(|mv| mv >= config.wake_mv)
        {
            cortex_m::peripheral::SCB::sys_reset();
        }
    }
}

pub async fn shutdown_radio(radio: &mut impl Radio) -> bool {
    match embassy_time::with_timeout(Duration::from_secs(2), radio.shutdown()).await {
        Ok(Ok(())) => true,
        Ok(Err(())) => {
            defmt::error!("Radio shutdown failed");
            false
        }
        Err(_) => {
            defmt::error!("Radio shutdown timed out");
            false
        }
    }
}
