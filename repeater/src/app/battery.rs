//! Samples the battery for the status screen, telemetry and MQTT.
use crate::app::AppContext;
use crate::platform::{Battery, Platform, Storage};
use defmt::info;
use embedded_hal_async::delay::DelayNs;

const SAMPLE_INTERVAL_MS: u32 = 60_000;
const PERCENT_MIN_MV: u16 = 3300;
const PERCENT_MAX_MV: u16 = 4100;

pub(crate) async fn run<P, S, B>(mut battery: B, context: &AppContext<P, S>) -> !
where
    P: Platform,
    S: Storage,
    B: Battery,
{
    let mut delay = context.platform.delay();

    loop {
        match battery.millivolts().await {
            Some(millivolts) => {
                let percent = level_percent(millivolts);
                info!("Battery: {}mV {}%", millivolts, percent);
                context.set_battery_millivolts(Some(millivolts));
                context.set_battery_level_percent(Some(percent));
            }
            None => {
                context.set_battery_millivolts(None);
                context.set_battery_level_percent(None);
            }
        }

        delay.delay_ms(SAMPLE_INTERVAL_MS).await;
    }
}

fn level_percent(millivolts: u16) -> u8 {
    if millivolts <= PERCENT_MIN_MV {
        return 0;
    }

    if millivolts >= PERCENT_MAX_MV {
        return 100;
    }

    let range = u32::from(PERCENT_MAX_MV - PERCENT_MIN_MV);
    let offset = u32::from(millivolts - PERCENT_MIN_MV);
    ((offset * 100) / range) as u8
}

#[cfg(test)]
mod tests {
    use super::level_percent;

    #[test]
    fn level_is_linear_between_empty_and_full() {
        assert_eq!(level_percent(0), 0);
        assert_eq!(level_percent(3300), 0);
        assert_eq!(level_percent(3700), 50);
        assert_eq!(level_percent(4100), 100);
        assert_eq!(level_percent(u16::MAX), 100);
    }
}
