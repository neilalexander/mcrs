//! The nRF52840 die temperature, exposed on the node's telemetry channel.
use embassy_nrf::temp::Temp;
use mcrs_repeater::platform::sensors::{CHANNEL_SELF, SensorReadings, SensorValue, Sensors};

pub struct NrfSensors<'d>(pub Temp<'d>);

impl Sensors for NrfSensors<'_> {
    async fn sample(&mut self, readings: &mut SensorReadings) {
        let quarters = self.0.read().await.to_bits();
        // Convert quarter degrees to tenths, rounding halves away from zero.
        let tenths = (quarters * 10 + if quarters < 0 { -2 } else { 2 }) / 4;
        if let Ok(decicelsius) = i16::try_from(tenths) {
            readings.push(CHANNEL_SELF, SensorValue::Temperature(decicelsius));
        }
    }
}
