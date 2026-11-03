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
}
