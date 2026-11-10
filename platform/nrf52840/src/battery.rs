//! Battery sensing with board-specific scaling and an optional switched divider.
use embassy_nrf::{
    Peri,
    gpio::{Level, Output},
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
    pub enable_level: Level,
}

impl BatteryConfig {
    /// Matches MeshCore's RAK4631 calibration: 3 * 1.73 * 1.187 volts.
    pub const fn rak4631() -> Self {
        Self {
            gain: Gain::Gain1_6,
            full_scale_battery_uv: 6_160_530,
            settle_ms: 0,
            enable_level: Level::Low,
        }
    }

    /// Matches MeshCore's XIAO/Solar P1 3 V ADC range and 3:1 divider.
    pub const fn xiao() -> Self {
        Self {
            gain: Gain::Gain1_5,
            full_scale_battery_uv: 9_000_000,
            settle_ms: 50,
            enable_level: Level::Low,
        }
    }
}

/// Samples eight readings, with calibration and a discarded settling sample.
pub struct NrfBattery<'d> {
    adc: Saadc<'d, 1>,
    enable: Option<Output<'d>>,
    config: BatteryConfig,
}

impl<'d> NrfBattery<'d> {
    pub fn new(
        peripheral: Peri<'d, peripherals::SAADC>,
        irq: impl interrupt::typelevel::Binding<interrupt::typelevel::SAADC, saadc::InterruptHandler>
        + 'd,
        input: impl saadc::Input + 'd,
        mut enable: Option<Output<'d>>,
        config: BatteryConfig,
    ) -> Self {
        if let Some(pin) = enable.as_mut() {
            pin.set_level(inactive_level(config.enable_level));
        }
        let mut channel = ChannelConfig::single_ended(input);
        channel.reference = saadc::Reference::Internal;
        channel.gain = config.gain;
        // High impedance battery dividers need the longest acquisition time.
        channel.time = saadc::Time::_40US;
        let mut adc_config = saadc::Config::default();
        adc_config.resolution = saadc::Resolution::_12bit;
        Self {
            adc: Saadc::new(peripheral, irq, adc_config, [channel]),
            enable,
            config,
        }
    }
}

fn inactive_level(active: Level) -> Level {
    match active {
        Level::Low => Level::High,
        Level::High => Level::Low,
    }
}

// Restore the low-power state on completion, error, timeout or cancellation.
struct DividerGuard<'a, 'd> {
    pin: &'a mut Output<'d>,
    inactive: Level,
}

impl Drop for DividerGuard<'_, '_> {
    fn drop(&mut self) {
        self.pin.set_level(self.inactive);
    }
}

impl Battery for NrfBattery<'_> {
    async fn millivolts(&mut self) -> Option<u16> {
        let config = self.config;
        let _divider = self.enable.as_mut().map(|pin| {
            pin.set_level(config.enable_level);
            DividerGuard {
                pin,
                inactive: inactive_level(config.enable_level),
            }
        });
        Timer::after_millis(config.settle_ms).await;

        let sum = with_timeout(Duration::from_millis(250), async {
            self.adc.calibrate().await;
            let mut sample = [0];
            self.adc.sample(&mut sample).await;
            let mut sum = 0i32;
            for _ in 0..8 {
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
        let denominator = 4096u64 * 8 * 1000;
        let mv =
            (sum as u64 * u64::from(config.full_scale_battery_uv) + denominator / 2) / denominator;
        u16::try_from(mv).ok()
    }
}
