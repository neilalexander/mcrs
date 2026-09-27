/// Monotonic advert countdown. Zero disables it; changing the interval restarts it.
pub(super) struct Schedule {
    interval_ms: u64,
    started_ms: u64,
}

impl Schedule {
    pub(super) fn new(interval_ms: u64, now_ms: u64) -> Self {
        Self {
            interval_ms,
            started_ms: now_ms,
        }
    }

    pub(super) fn due(&mut self, interval_ms: u64, now_ms: u64) -> bool {
        if interval_ms != self.interval_ms {
            *self = Self::new(interval_ms, now_ms);
            return false;
        }
        if interval_ms == 0 || now_ms.saturating_sub(self.started_ms) < interval_ms {
            return false;
        }
        // Missed intervals do not cause a burst of adverts.
        self.started_ms = now_ms;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
