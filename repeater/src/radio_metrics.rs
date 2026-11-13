//! Signal units at the radio boundary: RSSI in dBm, SNR in quarter-dB.
use core::fmt;

/// Format quarter-dB as a decimal dB value without floating point or allocation.
#[derive(Clone, Copy, Debug)]
pub struct SnrDb(pub i16);

impl fmt::Display for SnrDb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let magnitude = self.0.unsigned_abs();
        if self.0 < 0 {
            f.write_str("-")?;
        }
        write!(f, "{}.{:02}", magnitude / 4, (magnitude % 4) * 25)
    }
}

impl defmt::Format for SnrDb {
    fn format(&self, f: defmt::Formatter) {
        let magnitude = self.0.unsigned_abs();
        if self.0 < 0 {
            defmt::write!(f, "-");
        }
        defmt::write!(f, "{}.{:02}", magnitude / 4, (magnitude % 4) * 25)
    }
}

const SAMPLE_INTERVAL_MS: u64 = 60 * 1000;

pub(crate) struct NoiseFloor {
    next_sample_at_ms: u64,
    // Keep fractional dBm so repeated small changes accumulate.
    average_x256: Option<i32>,
}

impl NoiseFloor {
    pub(crate) fn new(now_ms: u64) -> Self {
        Self {
            next_sample_at_ms: now_ms.saturating_add(SAMPLE_INTERVAL_MS),
            average_x256: None,
        }
    }

    pub(crate) fn next_sample_at_ms(&self) -> u64 {
        self.next_sample_at_ms
    }

    /// Restart the interval even when no sample was available.
    pub(crate) fn complete(&mut self, now_ms: u64, sample_dbm: Option<i16>) -> Option<i16> {
        self.next_sample_at_ms = now_ms.saturating_add(SAMPLE_INTERVAL_MS);
        if let Some(sample) = sample_dbm {
            let sample = i32::from(sample) * 256;
            let average = self.average_x256.get_or_insert(sample);
            *average += (sample - *average) / 8;
        }
        self.average_x256.map(|average| {
            // Round to the nearest dBm, symmetrically about zero.
            ((average + average.signum() * 128) / 256) as i16
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn displays_db_including_negative_fractional_values() {
        for (quarters, expected) in [
            (32, "8.00"),
            (33, "8.25"),
            (34, "8.50"),
            (35, "8.75"),
            (-1, "-0.25"),
            (-33, "-8.25"),
            (0, "0.00"),
            (i16::MIN, "-8192.00"),
        ] {
            assert_eq!(format!("{}", SnrDb(quarters)), expected);
        }
    }

    #[test]
    fn waits_one_minute_after_every_attempt_without_catching_up() {
        let mut floor = NoiseFloor::new(123);
        assert_eq!(floor.next_sample_at_ms(), 60_123);
        assert_eq!(floor.complete(60_123, None), None);
        assert_eq!(floor.next_sample_at_ms(), 120_123);
        assert_eq!(floor.complete(300_000, Some(-120)), Some(-120));
        assert_eq!(floor.next_sample_at_ms(), 360_000);
        assert_eq!(floor.complete(360_000, None), Some(-120));
        assert_eq!(floor.next_sample_at_ms(), 420_000);
    }

    #[test]
    fn seeds_from_first_reading_and_smooths_in_both_directions() {
        let mut floor = NoiseFloor::new(0);
        assert_eq!(floor.complete(60_000, Some(-120)), Some(-120));
        assert_eq!(floor.complete(120_000, Some(-104)), Some(-118));
        assert_eq!(floor.complete(180_000, Some(-126)), Some(-119));
        assert_eq!(floor.complete(240_000, None), Some(-119));
    }

    #[test]
    fn small_changes_accumulate() {
        let mut floor = NoiseFloor::new(0);
        floor.complete(0, Some(-120));
        for interval in 1..=64 {
            floor.complete(interval * SAMPLE_INTERVAL_MS, Some(-119));
        }
        assert_eq!(floor.complete(65 * SAMPLE_INTERVAL_MS, None), Some(-119));
    }
}
