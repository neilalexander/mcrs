/// Monotonic advert countdown. Zero disables it; changing the interval restarts it.
pub(super) struct Schedule {
    interval_hours: u32,
    started_ms: u64,
}

impl Schedule {
    pub(super) fn new(interval_hours: u32, now_ms: u64) -> Self {
        Self {
            interval_hours,
            started_ms: now_ms,
        }
    }

    pub(super) fn due(&mut self, interval_hours: u32, now_ms: u64) -> bool {
        if interval_hours != self.interval_hours {
            *self = Self::new(interval_hours, now_ms);
            return false;
        }
        let interval_ms = u64::from(interval_hours) * 3_600_000;
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
        let mut schedule = Schedule::new(2, 1_000);
        assert!(!schedule.due(2, 7_200_999));
        assert!(schedule.due(2, 7_201_000));
        assert!(!schedule.due(2, 14_400_999));
        assert!(schedule.due(2, 30_000_000));
        assert!(!schedule.due(2, 30_000_000));
        assert!(schedule.due(2, 37_200_000));
    }

    #[test]
    fn interval_changes_and_reenabling_restart_countdown() {
        let mut schedule = Schedule::new(0, 0);
        assert!(!schedule.due(1, 10_000));
        assert!(!schedule.due(1, 3_609_999));
        assert!(schedule.due(1, 3_610_000));
        assert!(!schedule.due(2, 7_210_000));
        assert!(!schedule.due(2, 14_409_999));
        assert!(schedule.due(2, 14_410_000));
        assert!(!schedule.due(0, 21_610_000));
        assert!(!schedule.due(1, 30_000_000));
        assert!(schedule.due(1, 33_600_000));
    }

    #[test]
    fn largest_interval_does_not_overflow() {
        let mut schedule = Schedule::new(u32::MAX, 0);
        let deadline = u64::from(u32::MAX) * 3_600_000;
        assert!(!schedule.due(u32::MAX, deadline - 1));
        assert!(schedule.due(u32::MAX, deadline));
    }
}
