//! Battery sensing with board-specific scaling and an optional divider-enable pin.
use embassy_nrf::{
    Peri,
    gpio::Output,
    interrupt, peripherals,
    saadc::{self, AnyInput, ChannelConfig, Gain, Saadc},
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
pub struct NrfBattery<'d, IRQ> {
    peripheral: Peri<'d, peripherals::SAADC>,
    input: AnyInput<'d>,
    irq: IRQ,
    enable: Option<Output<'d>>,
    config: BatteryConfig,
}

impl<'d, IRQ> NrfBattery<'d, IRQ>
where
    IRQ: interrupt::typelevel::Binding<interrupt::typelevel::SAADC, saadc::InterruptHandler>
        + Copy
        + 'd,
{
    pub fn new(
        peripheral: Peri<'d, peripherals::SAADC>,
        irq: IRQ,
        input: impl saadc::Input + 'd,
        enable: Option<Output<'d>>,
        config: BatteryConfig,
    ) -> Self {
        let mut battery = Self {
            peripheral,
            input: input.degrade_saadc(),
            irq,
            enable,
            config,
        };
        battery.shutdown();
        battery
    }

    pub fn shutdown(&mut self) {
        if let Some(enable) = &mut self.enable {
            enable.set_high();
        }
    }

    async fn sample(&mut self) -> Option<u16> {
        let config = self.config;
        let mut channel = ChannelConfig::single_ended(self.input.reborrow());
        channel.reference = saadc::Reference::Internal;
        channel.gain = config.gain;
        channel.time = saadc::Time::_40US;
        let mut adc_config = saadc::Config::default();
        adc_config.resolution = saadc::Resolution::_12bit;
        let mut adc = Saadc::new(self.peripheral.reborrow(), self.irq, adc_config, [channel]);
        Timer::after_millis(config.settle_ms).await;

        let sum = with_timeout(Duration::from_millis(250), async {
            adc.calibrate().await;
            let mut sample = [0];
            adc.sample(&mut sample).await;
            let mut sum = 0i32;
            for _ in 0..SAMPLES {
                adc.sample(&mut sample).await;
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

impl<'d, IRQ> Battery for NrfBattery<'d, IRQ>
where
    IRQ: interrupt::typelevel::Binding<interrupt::typelevel::SAADC, saadc::InterruptHandler>
        + Copy
        + 'd,
{
    async fn millivolts(&mut self) -> Option<u16> {
        if let Some(enable) = &mut self.enable {
            enable.set_low();
        }
        let reading = self.sample().await;
        self.shutdown();
        reading
    }
}
