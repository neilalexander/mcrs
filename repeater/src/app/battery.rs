//! Samples the battery for the status screen, telemetry and MQTT.
use crate::{
    app::AppContext,
    platform::{Battery, Platform, Storage},
};
use defmt::info;
use embassy_futures::select::{Either, select};
use embedded_hal_async::delay::DelayNs;

const SAMPLE_INTERVAL_MS: u32 = 60_000;
const LOW_BATTERY_SAMPLES: u8 = 3;
// TODO: These percentages may not apply to all platforms/batteries.
const PERCENT_MIN_MV: u16 = 3000;
const PERCENT_MAX_MV: u16 = 4200;

pub(crate) async fn run<P, S, B>(
    battery: &mut B,
    context: &AppContext<P, S>,
    lowvoltage_supported: bool,
) where
    P: Platform,
    S: Storage,
    B: Battery,
{
    let mut delay = context.platform.delay();

    let mut low_samples = 0;
    loop {
        context.lowvoltage_changed.reset();
        let reading = battery.millivolts().await;
        match reading {
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

        let voltage_warning_mv = if lowvoltage_supported {
            context
                .with_config(|config| {
                    let lowvoltage = config.lowvoltage();
                    if lowvoltage.enabled {
                        lowvoltage.sleep_mv
                    } else {
                        0
                    }
                })
                .await
        } else {
            0
        };
        if context.lowvoltage_changed.signaled() {
            low_samples = 0;
            continue;
        }
        if update_low_samples(&mut low_samples, reading, voltage_warning_mv) {
            return;
        }

        if matches!(
            select(
                context.lowvoltage_changed.wait(),
                delay.delay_ms(SAMPLE_INTERVAL_MS)
            )
            .await,
            Either::First(())
        ) {
            low_samples = 0;
        }
    }
}

fn update_low_samples(count: &mut u8, reading: Option<u16>, voltage_warning_mv: u16) -> bool {
    if voltage_warning_mv != 0 && reading.is_some_and(|mv| mv <= voltage_warning_mv) {
        *count = count.saturating_add(1);
    } else {
        *count = 0;
    }
    *count >= LOW_BATTERY_SAMPLES
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
    use super::{level_percent, update_low_samples};

    #[test]
    fn shutdown_requires_consecutive_valid_low_readings() {
        let mut count = 0;
        assert!(!update_low_samples(&mut count, Some(3300), 3300));
        assert!(!update_low_samples(&mut count, Some(3200), 3300));
        assert!(!update_low_samples(&mut count, Some(3301), 3300));
        assert_eq!(count, 0);
        assert!(!update_low_samples(&mut count, Some(3200), 3300));
        assert!(!update_low_samples(&mut count, None, 3300));
        assert_eq!(count, 0);
        assert!(!update_low_samples(&mut count, Some(3300), 3300));
        assert!(!update_low_samples(&mut count, Some(3300), 3300));
        assert!(update_low_samples(&mut count, Some(3300), 3300));
    }

    #[test]
    fn voltage_cutoff_uses_the_supplied_setting() {
        let mut count = 0;
        for _ in 0..3 {
            assert!(!update_low_samples(&mut count, Some(3400), 3300));
        }
        assert!(!update_low_samples(&mut count, Some(3400), 3400));
        assert!(!update_low_samples(&mut count, Some(3400), 3400));
        assert!(update_low_samples(&mut count, Some(3400), 3400));
    }

    #[test]
    fn level_is_linear_between_empty_and_full() {
        assert_eq!(level_percent(0), 0);
        assert_eq!(level_percent(3000), 0);
        assert_eq!(level_percent(3600), 50);
        assert_eq!(level_percent(3700), 58);
        assert_eq!(level_percent(4200), 100);
        assert_eq!(level_percent(u16::MAX), 100);
    }
}
