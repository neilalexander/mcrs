//! Transport-independent typed readings. Board code owns buses and drivers.
use core::future::Future;

/// Cayenne LPP channel reserved for measurements of the node itself.
pub const CHANNEL_SELF: u8 = 1;
const MAX_READINGS: usize = 32;
const MAX_ENCODED_BYTES: usize = 128;
const MAX_POLYLINE_BYTES: usize = 32;

/// A measurement and its units, encoded using MeshCore's Cayenne LPP types.
/// Integer payloads use the documented fixed-point units, avoiding floating-point
/// conversion or silently wrapping values during serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorValue {
    DigitalInput(u8),
    DigitalOutput(u8),
    AnalogInput(i16),  // 0.01 units
    AnalogOutput(i16), // 0.01 units
    Generic(u32),
    Luminosity(u16), // lux
    Presence(bool),
    Temperature(i16),        // 0.1 °C
    RelativeHumidity(u8),    // 0.5% RH, 0..=200
    Accelerometer([i16; 3]), // XYZ, 0.001 g
    BarometricPressure(u16), // 0.1 hPa
    Voltage(u16),            // 0.01 V
    Current(u16),            // 0.001 A
    Frequency(u32),          // Hz
    Percentage(u8),
    Altitude(i16),       // m
    Concentration(u16),  // ppm
    Power(u16),          // W
    Distance(u32),       // 0.001 m
    Energy(u32),         // 0.001 kWh
    Direction(u16),      // degrees
    UnixTime(u32),       // seconds
    Gyrometer([i16; 3]), // XYZ, 0.01 °/s
    Colour([u8; 3]),     // RGB
    Gps([i32; 3]),       // latitude/longitude: 0.0001°; altitude: 0.01 m; signed 24-bit
    Switch(bool),
    Polyline(Polyline),
}

/// A bounded, already encoded polyline. Coordinate compression belongs to its producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Polyline {
    bytes: [u8; MAX_POLYLINE_BYTES],
    len: u8,
}

impl Polyline {
    /// Accepts an LPP polyline header and compressed deltas (8..=32 bytes).
    /// The first byte must equal the full payload length, including itself.
    pub fn from_encoded(encoded: &[u8]) -> Option<Self> {
        if !(8..=MAX_POLYLINE_BYTES).contains(&encoded.len())
            || usize::from(encoded[0]) != encoded.len()
        {
            return None;
        }
        let mut bytes = [0; MAX_POLYLINE_BYTES];
        bytes[..encoded.len()].copy_from_slice(encoded);
        Some(Self {
            bytes,
            len: encoded.len() as u8,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

impl SensorValue {
    /// Cayenne LPP wire type, carried alongside each value.
    pub const fn lpp_type(&self) -> u8 {
        match self {
            Self::DigitalInput(..) => 0,
            Self::DigitalOutput(..) => 1,
            Self::AnalogInput(..) => 2,
            Self::AnalogOutput(..) => 3,
            Self::Generic(..) => 100,
            Self::Luminosity(..) => 101,
            Self::Presence(..) => 102,
            Self::Temperature(..) => 103,
            Self::RelativeHumidity(..) => 104,
            Self::Accelerometer(..) => 113,
            Self::BarometricPressure(..) => 115,
            Self::Voltage(..) => 116,
            Self::Current(..) => 117,
            Self::Frequency(..) => 118,
            Self::Percentage(..) => 120,
            Self::Altitude(..) => 121,
            Self::Concentration(..) => 125,
            Self::Power(..) => 128,
            Self::Distance(..) => 130,
            Self::Energy(..) => 131,
            Self::Direction(..) => 132,
            Self::UnixTime(..) => 133,
            Self::Gyrometer(..) => 134,
            Self::Colour(..) => 135,
            Self::Gps(..) => 136,
            Self::Switch(..) => 142,
            Self::Polyline(..) => 240,
        }
    }

    fn valid(&self) -> bool {
        match self {
            Self::RelativeHumidity(value) => *value <= 200,
            Self::Percentage(value) => *value <= 100,
            Self::Gps(xyz) => {
                (-900_000..=900_000).contains(&xyz[0])
                    && (-1_800_000..=1_800_000).contains(&xyz[1])
                    && (-8_388_608..=8_388_607).contains(&xyz[2])
            }
            _ => true,
        }
    }

    fn payload_len(&self) -> usize {
        let mut len = 0;
        self.write_payload(|bytes| len += bytes.len());
        len
    }

    pub(crate) fn encode_lpp(&self, channel: u8, output: &mut alloc::vec::Vec<u8>) {
        output.extend_from_slice(&[channel, self.lpp_type()]);
        self.write_payload(|bytes| output.extend_from_slice(bytes));
    }

    fn write_payload(&self, mut write: impl FnMut(&[u8])) {
        match self {
            Self::Presence(value) | Self::Switch(value) => write(&[u8::from(*value)]),
            Self::AnalogInput(value)
            | Self::AnalogOutput(value)
            | Self::Temperature(value)
            | Self::Altitude(value) => write(&value.to_be_bytes()),
            Self::Generic(value)
            | Self::Frequency(value)
            | Self::Distance(value)
            | Self::Energy(value)
            | Self::UnixTime(value) => write(&value.to_be_bytes()),
            Self::Luminosity(value)
            | Self::BarometricPressure(value)
            | Self::Voltage(value)
            | Self::Current(value)
            | Self::Concentration(value)
            | Self::Power(value)
            | Self::Direction(value) => write(&value.to_be_bytes()),
            Self::DigitalInput(value)
            | Self::DigitalOutput(value)
            | Self::RelativeHumidity(value)
            | Self::Percentage(value) => write(&[*value]),
            Self::Accelerometer(value) | Self::Gyrometer(value) => {
                for axis in value {
                    write(&axis.to_be_bytes());
                }
            }
            Self::Colour(value) => write(value),
            Self::Gps(value) => {
                for coordinate in value {
                    write(&coordinate.to_be_bytes()[1..]);
                }
            }
            Self::Polyline(value) => write(value.as_bytes()),
        }
    }
}

/// Channel 1 describes the node; channels 2+ identify external sources.
/// Different types may share a channel, e.g. voltage, current and power.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorReading {
    pub channel: u8,
    pub value: SensorValue,
}

/// Bounded snapshot, limited to 32 readings and 128 encoded bytes. This leaves
/// room for battery voltage and the timestamp in the telemetry reply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SensorReadings {
    readings: [Option<SensorReading>; MAX_READINGS],
    encoded_len: usize,
}

impl SensorReadings {
    /// Rejects invalid values, channel zero, duplicate (channel, type) pairs,
    /// and snapshots exceeding either capacity limit. Failure leaves it unchanged.
    pub fn push(&mut self, channel: u8, value: SensorValue) -> bool {
        let size = 2 + value.payload_len();
        if channel == 0
            || !value.valid()
            || self.encoded_len + size > MAX_ENCODED_BYTES
            || self
                .iter()
                .any(|r| r.channel == channel && r.value.lpp_type() == value.lpp_type())
        {
            return false;
        }
        let Some(slot) = self.readings.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        *slot = Some(SensorReading { channel, value });
        self.encoded_len += size;
        true
    }

    pub fn iter(&self) -> impl Iterator<Item = &SensorReading> {
        self.readings.iter().flatten()
    }

    pub(crate) fn encode_lpp(&self, output: &mut alloc::vec::Vec<u8>) {
        for reading in self.iter() {
            reading.value.encode_lpp(reading.channel, output);
        }
    }
}

/// One driver or a collection of drivers, independent of I²C or MCU peripherals.
/// Append successful readings only. Channel assignments belong to board code.
/// Sampling must be cancellation-safe: the repeater cancels a batch after two seconds.
pub trait Sensors {
    fn sample(&mut self, readings: &mut SensorReadings) -> impl Future<Output = ()>;
}

/// Compose built-in and external drivers without allocating or sharing buses.
impl<A: Sensors, B: Sensors> Sensors for (A, B) {
    async fn sample(&mut self, readings: &mut SensorReadings) {
        self.0.sample(readings).await;
        self.1.sample(readings).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn all_types_match_upstream_wire_codes_widths_and_units() {
        // Explicit byte fixtures exercise signed, unsigned, vector and variable payloads.
        let fixtures: &[(SensorValue, &[u8])] = &[
            (SensorValue::DigitalInput(42), &[0, 42]),
            (SensorValue::DigitalOutput(1), &[1, 1]),
            (SensorValue::AnalogInput(-123), &[2, 255, 133]),
            (SensorValue::AnalogOutput(123), &[3, 0, 123]),
            (SensorValue::Generic(0x12345678), &[100, 18, 52, 86, 120]),
            (SensorValue::Luminosity(1000), &[101, 3, 232]),
            (SensorValue::Presence(true), &[102, 1]),
            (SensorValue::Temperature(-55), &[103, 255, 201]),
            (SensorValue::RelativeHumidity(101), &[104, 101]),
            (
                SensorValue::Accelerometer([-1000, 0, 1000]),
                &[113, 252, 24, 0, 0, 3, 232],
            ),
            (SensorValue::BarometricPressure(10132), &[115, 39, 148]),
            (SensorValue::Voltage(420), &[116, 1, 164]),
            (SensorValue::Current(1000), &[117, 3, 232]),
            (SensorValue::Frequency(1_000_000), &[118, 0, 15, 66, 64]),
            (SensorValue::Percentage(99), &[120, 99]),
            (SensorValue::Altitude(-10), &[121, 255, 246]),
            (SensorValue::Concentration(400), &[125, 1, 144]),
            (SensorValue::Power(123), &[128, 0, 123]),
            (SensorValue::Distance(1000), &[130, 0, 0, 3, 232]),
            (SensorValue::Energy(1000), &[131, 0, 0, 3, 232]),
            (SensorValue::Direction(359), &[132, 1, 103]),
            (SensorValue::UnixTime(0x12345678), &[133, 18, 52, 86, 120]),
            (
                SensorValue::Gyrometer([-100, 0, 100]),
                &[134, 255, 156, 0, 0, 0, 100],
            ),
            (SensorValue::Colour([255, 128, 0]), &[135, 255, 128, 0]),
            (
                SensorValue::Gps([-10000, 10000, -100]),
                &[136, 255, 216, 240, 0, 39, 16, 255, 255, 156],
            ),
            (SensorValue::Switch(false), &[142, 0]),
            (
                SensorValue::Polyline(
                    Polyline::from_encoded(&[9, 0, 0, 0, 0, 0, 0, 0, 0x11]).unwrap(),
                ),
                &[240, 9, 0, 0, 0, 0, 0, 0, 0, 0x11],
            ),
        ];
        for &(value, expected) in fixtures {
            let mut readings = SensorReadings::default();
            assert!(readings.push(2, value));
            let mut bytes = Vec::new();
            readings.encode_lpp(&mut bytes);
            assert_eq!(bytes[0], 2);
            assert_eq!(&bytes[1..], expected, "{value:?}");
            assert_eq!(bytes.len(), readings.encoded_len);
        }
    }

    #[test]
    fn one_channel_can_report_multiple_types_but_not_duplicate_types() {
        let mut readings = SensorReadings::default();
        for value in [
            SensorValue::Voltage(420),
            SensorValue::Current(100),
            SensorValue::Power(1),
        ] {
            assert!(readings.push(2, value));
        }
        let before = readings;
        assert!(!readings.push(2, SensorValue::Voltage(500)));
        assert_eq!(readings, before);
        assert!(readings.push(3, SensorValue::Voltage(500)));
    }

    #[test]
    fn invalid_values_and_capacity_overflow_leave_snapshot_unchanged() {
        let mut readings = SensorReadings::default();
        for value in [
            SensorValue::RelativeHumidity(201),
            SensorValue::Percentage(101),
            SensorValue::Gps([900001, 0, 0]),
            SensorValue::Gps([0, 1800001, 0]),
            SensorValue::Gps([0, 0, 8388608]),
            SensorValue::Gps([0, 0, -8388609]),
        ] {
            assert!(!readings.push(2, value));
        }
        assert!(!readings.push(0, SensorValue::Temperature(0)));
        for channel in 1..=32 {
            assert!(readings.push(channel, SensorValue::Temperature(0)));
        }
        let before = readings;
        assert!(!readings.push(33, SensorValue::Temperature(0)));
        assert_eq!(readings, before);
        let mut readings = SensorReadings::default();
        for channel in 1..=11 {
            assert!(readings.push(channel, SensorValue::Gps([0; 3])));
        }
        let before = readings;
        assert!(!readings.push(12, SensorValue::Gps([0; 3])));
        assert_eq!(readings, before);
    }

    #[test]
    fn polyline_requires_a_complete_bounded_header() {
        assert!(Polyline::from_encoded(&[]).is_none());
        assert!(Polyline::from_encoded(&[7; 7]).is_none());
        assert!(Polyline::from_encoded(&[9; 8]).is_none());
        assert!(Polyline::from_encoded(&[33; 33]).is_none());
        assert_eq!(Polyline::from_encoded(&[8; 8]).unwrap().as_bytes(), &[8; 8]);
    }
}
