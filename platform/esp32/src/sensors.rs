//! ESP32-S3 die temperature. esp-hal 1.1 does not expose the S3 sensor.
//! Register sequence and nominal conversion follow Espressif ESP-IDF v5.4:
//! components/hal/esp32s3/include/hal/temperature_sensor_ll.h and
//! components/soc/esp32s3/temperature_sensor_periph.c.
//! These are die readings without per-chip eFuse temperature calibration.
use esp_hal::peripherals::SENS;
use mcrs_repeater::platform::sensors::{CHANNEL_SELF, SensorReadings, SensorValue, Sensors};

pub struct EspSensors {
    _peripheral: SENS<'static>,
}

unsafe extern "C" {
    fn rom_i2c_writeReg_Mask(block: u8, host: u8, register: u8, msb: u8, lsb: u8, value: u8);
}

fn regs() -> &'static esp32s3::sens::RegisterBlock {
    // Only temperature fields are accessed here. The ADC uses other SENS fields.
    unsafe { &*esp32s3::SENS::PTR }
}

impl EspSensors {
    pub fn new(peripheral: SENS<'static>) -> Self {
        Self {
            _peripheral: peripheral,
        }
    }
}

struct PoweredSensor<'a> {
    _peripheral: &'a mut SENS<'static>,
}
impl<'a> PoweredSensor<'a> {
    fn new(peripheral: &'a mut SENS<'static>) -> Self {
        critical_section::with(|_| {
            regs()
                .sar_peri_clk_gate_conf()
                .modify(|_, w| w.tsens_clk_en().set_bit());
            regs()
                .sar_peri_reset_conf()
                .modify(|_, w| w.sar_tsens_reset().set_bit());
            regs()
                .sar_peri_reset_conf()
                .modify(|_, w| w.sar_tsens_reset().clear_bit());
            regs()
                .sar_tsens_ctrl()
                .modify(|_, w| unsafe { w.sar_tsens_clk_div().bits(6) });
        });
        Self {
            _peripheral: peripheral,
        }
    }

    fn set_range(&self, dac: u8) {
        critical_section::with(|_| {
            // Internal analog configuration bus: not the external GPIO I²C bus.
            // ESP-IDF defines TSENS_DAC as block 0x69, host 1, reg 6, bits 3:0.
            unsafe { rom_i2c_writeReg_Mask(0x69, 1, 6, 3, 0, dac) };
            regs()
                .sar_tsens_ctrl2()
                .modify(|_, w| unsafe { w.sar_tsens_xpd_force().bits(1) });
            regs().sar_tsens_ctrl().modify(|_, w| {
                w.sar_tsens_power_up_force()
                    .set_bit()
                    .sar_tsens_power_up()
                    .set_bit()
            });
        });
    }

    async fn read_raw(&self) -> Option<u8> {
        embassy_time::Timer::after_micros(250).await;
        regs()
            .sar_tsens_ctrl()
            .modify(|_, w| w.sar_tsens_dump_out().set_bit());
        let mut raw = None;
        // Bounded polling: a failed peripheral cannot stall the repeater.
        for _ in 0..20 {
            let value = regs().sar_tsens_ctrl().read();
            if value.sar_tsens_ready().bit_is_set() {
                raw = Some(value.sar_tsens_out().bits());
                break;
            }
            embassy_time::Timer::after_micros(50).await;
        }
        regs()
            .sar_tsens_ctrl()
            .modify(|_, w| w.sar_tsens_dump_out().clear_bit());
        raw
    }
}

impl Drop for PoweredSensor<'_> {
    fn drop(&mut self) {
        critical_section::with(|_| {
            regs().sar_tsens_ctrl().modify(|_, w| {
                w.sar_tsens_dump_out()
                    .clear_bit()
                    .sar_tsens_power_up()
                    .clear_bit()
                    .sar_tsens_power_up_force()
                    .clear_bit()
            });
            regs()
                .sar_tsens_ctrl2()
                .modify(|_, w| unsafe { w.sar_tsens_xpd_force().bits(0) });
            regs()
                .sar_peri_clk_gate_conf()
                .modify(|_, w| w.tsens_clk_en().clear_bit());
        });
    }
}

impl Sensors for EspSensors {
    async fn sample(&mut self, readings: &mut SensorReadings) {
        let power = PoweredSensor::new(&mut self._peripheral);
        // Try the most accurate range first, then cover the full -40..125 C range.
        for (dac, offset, minimum, maximum) in [
            (15, 0, -100, 800),
            (11, 1, -300, 500),
            (10, 2, -400, 200),
            (7, -1, 200, 1000),
            (5, -2, 500, 1250),
        ] {
            power.set_range(dac);
            let Some(raw) = power.read_raw().await else {
                return;
            };
            let numerator = i32::from(raw) * 4386 - offset * 278800 - 205200;
            let tenths = (numerator + if numerator < 0 { -500 } else { 500 }) / 1000;
            if (minimum..=maximum).contains(&tenths) {
                readings.push(CHANNEL_SELF, SensorValue::Temperature(tenths as i16));
                return;
            }
        }
    }
}
