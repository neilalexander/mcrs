use super::{AppContext, config::AppConfig};
use crate::platform::{
    Platform, Storage,
    sensors::{SensorReadings, Sensors},
};
use defmt::{error, info};
use embedded_hal_async::delay::DelayNs;

/// Monotonic periodic countdown. Zero disables it; changing the interval restarts it.
struct Schedule {
    interval_ms: u64,
    started_ms: u64,
}

impl Schedule {
    fn new(interval_ms: u64, now_ms: u64) -> Self {
        Self {
            interval_ms,
            started_ms: now_ms,
        }
    }

    /// No deadline when disabled; zero means work is already due.
    fn remaining_ms(&self, now_ms: u64) -> Option<u64> {
        (self.interval_ms != 0).then(|| {
            self.interval_ms
                .saturating_sub(now_ms.saturating_sub(self.started_ms))
        })
    }

    fn due(&mut self, interval_ms: u64, now_ms: u64) -> bool {
        if interval_ms != self.interval_ms {
            *self = Self::new(interval_ms, now_ms);
            return false;
        }
        if self.remaining_ms(now_ms) != Some(0) {
            return false;
        }
        // Missed intervals do not cause a burst of adverts.
        self.started_ms = now_ms;
        true
    }
}

const SENSOR_SAMPLE_TIMEOUT_MS: u32 = 2_000;

/// Attempt one zero-hop advert 30 seconds after successful radio initialization.
/// This runs independently of the configured periodic advert intervals.
pub(crate) async fn startup_advert<P: Platform, S: Storage>(context: &AppContext<P, S>) {
    context.platform.delay().delay_ms(30_000).await;
    let packet = context
        .with_config(|config| {
            super::discovery::zero_hop_advert(config, context.platform.now_seconds())
        })
        .await;
    send_advert(context, packet, "startup zero-hop").await;
}

pub(super) fn intervals(config: &AppConfig) -> [u64; 3] {
    [
        u64::from(config.advert_interval_minutes()) * 60_000,
        u64::from(config.flood_advert_interval_hours()) * 3_600_000,
        u64::from(config.sensor_interval_minutes()) * 60_000,
    ]
}

/// Runs due work, then sleeps until the next deadline or an interval change.
pub(crate) async fn run<P: Platform, S: Storage, T: Sensors>(
    context: &AppContext<P, S>,
    sensors: &mut T,
) {
    let mut delay = context.platform.delay();
    let initial = context.with_config(intervals).await;
    let now_ms = context.platform.now_millis();
    let mut schedules = initial.map(|interval| Schedule::new(interval, now_ms));
    let mut first_poll = true;
    loop {
        // Reset before reading config so changes during processing remain latched.
        context.periodic_changed.reset();
        let current = context.with_config(intervals).await;
        let now_ms = context.platform.now_millis();
        let [zero_hop_due, flood_due, sensor_due] =
            core::array::from_fn(|i| schedules[i].due(current[i], now_ms));
        if current[2] == 0 {
            *context.sensor_readings.borrow_mut() = SensorReadings::default();
        } else if sensor_due || first_poll {
            let readings = sample_sensors(sensors, &mut delay).await;
            *context.sensor_readings.borrow_mut() = readings;
        }
        first_poll = false;
        if flood_due {
            let packet = context
                .with_config(|config| {
                    super::discovery::flood_advert(config, context.platform.now_seconds())
                })
                .await;
            send_advert(context, packet, "flood").await;
        }
        if zero_hop_due {
            let packet = context
                .with_config(|config| {
                    super::discovery::zero_hop_advert(config, context.platform.now_seconds())
                })
                .await;
            send_advert(context, packet, "zero-hop").await;
        }
        let now_ms = context.platform.now_millis();
        match next_wakeup_ms(&schedules, now_ms) {
            Some(remaining_ms) => {
                embassy_futures::select::select(
                    sleep_ms(&mut delay, remaining_ms),
                    context.periodic_changed.wait(),
                )
                .await;
            }
            None => context.periodic_changed.wait().await,
        }
    }
}

fn next_wakeup_ms(schedules: &[Schedule], now_ms: u64) -> Option<u64> {
    schedules
        .iter()
        .filter_map(|schedule| schedule.remaining_ms(now_ms))
        .min()
}

async fn sleep_ms<D: DelayNs>(delay: &mut D, mut remaining_ms: u64) {
    // DelayNs accepts u32 milliseconds; preserve long configured intervals.
    while remaining_ms != 0 {
        let chunk = remaining_ms.min(u64::from(u32::MAX)) as u32;
        delay.delay_ms(chunk).await;
        remaining_ms -= u64::from(chunk);
    }
}

async fn send_advert<P: Platform, S: Storage>(
    context: &AppContext<P, S>,
    packet: Option<alloc::vec::Vec<u8>>,
    kind: &str,
) {
    let Some(packet) = packet else {
        error!("Periodic: {} advert encode failed", kind);
        return;
    };

    let len = packet.len();
    let region = context.outbound_region_label(&packet).await;
    match context.enqueue_outbound(packet) {
        Ok(()) => match region {
            Some(region) => info!(
                "Periodic: queued {} advert {} bytes region={}",
                kind, len, region
            ),
            None => info!("Periodic: queued {} advert {} bytes", kind, len),
        },
        Err(_) => error!("Periodic: {} advert queue full", kind),
    }
}

async fn sample_sensors<T: Sensors, D: DelayNs>(sensors: &mut T, delay: &mut D) -> SensorReadings {
    let mut readings = SensorReadings::default();
    // Drivers must be cancellation-safe. Keep successful readings if another
    // sensor in a collection stalls, and retry the collection next interval.
    embassy_futures::select::select(
        sensors.sample(&mut readings),
        delay.delay_ms(SENSOR_SAMPLE_TIMEOUT_MS),
    )
    .await;
    readings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::sensors::{CHANNEL_SELF, SensorValue};

    #[test]
    fn remaining_time_tracks_deadlines_and_interval_changes() {
        let mut schedule = Schedule::new(60_000, 1_000);
        assert_eq!(schedule.remaining_ms(1_000), Some(60_000));
        assert_eq!(schedule.remaining_ms(60_999), Some(1));
        assert_eq!(schedule.remaining_ms(61_000), Some(0));
        assert_eq!(schedule.remaining_ms(90_000), Some(0));
        assert!(schedule.due(60_000, 90_000));
        assert_eq!(schedule.remaining_ms(90_000), Some(60_000));
        assert!(!schedule.due(120_000, 100_000));
        assert_eq!(schedule.remaining_ms(100_000), Some(120_000));
        assert!(!schedule.due(0, 110_000));
        assert_eq!(schedule.remaining_ms(u64::MAX), None);
    }

    #[test]
    fn disabled_schedule_never_fires() {
        assert!(!Schedule::new(0, 0).due(0, u64::MAX));
    }

    #[test]
    fn repeats_in_hours_without_catching_up_missed_intervals() {
        let mut schedule = Schedule::new(7_200_000, 1_000);
        assert!(!schedule.due(7_200_000, 7_200_999));
        assert!(schedule.due(7_200_000, 7_201_000));
        assert!(!schedule.due(7_200_000, 14_400_999));
        assert!(schedule.due(7_200_000, 30_000_000));
        assert!(!schedule.due(7_200_000, 30_000_000));
        assert!(schedule.due(7_200_000, 37_200_000));
    }

    #[test]
    fn interval_changes_and_reenabling_restart_countdown() {
        let mut schedule = Schedule::new(0, 0);
        assert!(!schedule.due(3_600_000, 10_000));
        assert!(!schedule.due(3_600_000, 3_609_999));
        assert!(schedule.due(3_600_000, 3_610_000));
        assert!(!schedule.due(7_200_000, 7_210_000));
        assert!(!schedule.due(7_200_000, 14_409_999));
        assert!(schedule.due(7_200_000, 14_410_000));
        assert!(!schedule.due(0, 21_610_000));
        assert!(!schedule.due(3_600_000, 30_000_000));
        assert!(schedule.due(3_600_000, 33_600_000));
    }

    #[test]
    fn minute_intervals_preserve_sub_hour_precision() {
        let interval_ms = 61 * 60_000;
        let mut schedule = Schedule::new(interval_ms, 0);
        assert!(!schedule.due(interval_ms, 3_600_000));
        assert!(!schedule.due(interval_ms, 3_659_999));
        assert!(schedule.due(interval_ms, 3_660_000));
    }

    #[test]
    fn largest_interval_does_not_overflow() {
        let deadline = u64::from(u32::MAX) * 3_600_000;
        let mut schedule = Schedule::new(deadline, 0);
        assert!(!schedule.due(deadline, deadline - 1));
        assert!(schedule.due(deadline, deadline));
    }

    #[test]
    fn next_wakeup_uses_earliest_enabled_deadline() {
        let schedules = [
            Schedule::new(240 * 60_000, 0),
            Schedule::new(0, 0),
            Schedule::new(60_000, 0),
        ];
        assert_eq!(next_wakeup_ms(&schedules, 2_000), Some(58_000));
        assert_eq!(next_wakeup_ms(&schedules, 60_001), Some(0));
        assert_eq!(next_wakeup_ms(&[Schedule::new(0, 0)], 10), None);
    }

    #[test]
    fn long_sleeps_preserve_duration_without_secondly_polling() {
        struct RecordingDelay(alloc::vec::Vec<u32>);
        impl DelayNs for RecordingDelay {
            async fn delay_ns(&mut self, _: u32) {
                panic!("millisecond sleeps should use delay_ms directly");
            }
            async fn delay_ms(&mut self, ms: u32) {
                self.0.push(ms);
            }
        }
        embassy_futures::block_on(async {
            let mut delay = RecordingDelay(alloc::vec::Vec::new());
            sleep_ms(&mut delay, 60_000).await;
            assert_eq!(delay.0, [60_000]);
            delay.0.clear();
            sleep_ms(&mut delay, u64::from(u32::MAX) + 123).await;
            assert_eq!(delay.0, [u32::MAX, 123]);
            delay.0.clear();
            sleep_ms(&mut delay, 0).await;
            assert!(delay.0.is_empty());
        });
    }

    struct Driver {
        reading: Option<i16>,
    }
    impl Sensors for Driver {
        async fn sample(&mut self, readings: &mut SensorReadings) {
            if let Some(decicelsius) = self.reading {
                readings.push(CHANNEL_SELF, SensorValue::Temperature(decicelsius));
            }
        }
    }
    struct Stuck;
    impl Sensors for Stuck {
        async fn sample(&mut self, _: &mut SensorReadings) {
            core::future::pending::<()>().await;
        }
    }
    struct Timeout;
    impl DelayNs for Timeout {
        async fn delay_ns(&mut self, _: u32) {}
    }

    #[test]
    fn failed_readings_replace_old_data() {
        embassy_futures::block_on(async {
            let mut sensor = Driver { reading: Some(-55) };
            assert_eq!(
                sample_sensors(&mut sensor, &mut Timeout)
                    .await
                    .iter()
                    .count(),
                1
            );
            sensor.reading = None;
            assert_eq!(
                sample_sensors(&mut sensor, &mut Timeout)
                    .await
                    .iter()
                    .count(),
                0
            );
        });
    }

    #[test]
    fn collection_retains_successes_when_another_driver_times_out() {
        embassy_futures::block_on(async {
            let mut sensors = (Driver { reading: Some(253) }, Stuck);
            let snapshot = sample_sensors(&mut sensors, &mut Timeout).await;
            assert_eq!(
                snapshot.iter().next().unwrap().value,
                SensorValue::Temperature(253)
            );
        });
    }
}
