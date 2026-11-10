//! Battery sensing with board-specific scaling and an optional divider-enable pin.
use embassy_nrf::{
    Peri,
    gpio::Output,
    interrupt, peripherals,
    saadc::{self, ChannelConfig, Gain, Saadc},
};
use embassy_time::{Duration, Timer, with_timeout};
use mcrs_repeater::platform::Battery;

/// ADC range and battery-side calibration for a board's voltage divider.
#[derive(Clone, Copy)]
pub struct BatteryConfig {
    pub gain: Gain,
    /// Battery voltage corresponding to ADC full scale, including calibration.
    pub full_scale_battery_uv: u32,
    pub settle_ms: u64,
}

impl BatteryConfig {
    /// Matches MeshCore's RAK4631 calibration: 3 * 1.73 * 1.187 volts.
    pub const fn rak4631() -> Self {
        Self {
            gain: Gain::Gain1_6,
            full_scale_battery_uv: 6_160_530,
            settle_ms: 0,
        }
    }

    /// Matches MeshCore's XIAO/Solar P1 3 V ADC range and 3:1 divider.
    pub const fn xiao() -> Self {
        Self {
            gain: Gain::Gain1_5,
            full_scale_battery_uv: 9_000_000,
            settle_ms: 50,
        }
    }
}

const SAMPLES: u64 = 8;

/// Samples eight readings, with calibration and a discarded settling sample.
pub struct NrfBattery<'d> {
    adc: Saadc<'d, 1>,
    // Retain the pin in its board-configured state for the lifetime of sensing.
    _enable: Option<Output<'d>>,
    config: BatteryConfig,
}

impl<'d> NrfBattery<'d> {
    pub fn new(
        peripheral: Peri<'d, peripherals::SAADC>,
        irq: impl interrupt::typelevel::Binding<interrupt::typelevel::SAADC, saadc::InterruptHandler>
        + 'd,
        input: impl saadc::Input + 'd,
        enable: Option<Output<'d>>,
        config: BatteryConfig,
    ) -> Self {
        let mut channel = ChannelConfig::single_ended(input);
        channel.reference = saadc::Reference::Internal;
        channel.gain = config.gain;
        // High impedance battery dividers need the longest acquisition time.
        channel.time = saadc::Time::_40US;
        let mut adc_config = saadc::Config::default();
        adc_config.resolution = saadc::Resolution::_12bit;
        Self {
            adc: Saadc::new(peripheral, irq, adc_config, [channel]),
            _enable: enable,
            config,
        }
    }
}

impl Battery for NrfBattery<'_> {
    async fn millivolts(&mut self) -> Option<u16> {
        let config = self.config;
        Timer::after_millis(config.settle_ms).await;

        let sum = with_timeout(Duration::from_millis(250), async {
            self.adc.calibrate().await;
            let mut sample = [0];
            self.adc.sample(&mut sample).await;
            let mut sum = 0i32;
            for _ in 0..SAMPLES {
                self.adc.sample(&mut sample).await;
                if !(0..4095).contains(&sample[0]) {
                    return None;
                }
                sum += i32::from(sample[0]);
            }
            Some(sum)
        })
        .await
        .ok()??;

        if sum <= 0 || config.full_scale_battery_uv == 0 {
            return None;
        }
        // Average eight 12-bit samples and convert microvolts to millivolts.
        let denominator = 4096 * SAMPLES * 1000;
        let mv =
            (sum as u64 * u64::from(config.full_scale_battery_uv) + denominator / 2) / denominator;
        u16::try_from(mv).ok()
    }
}
