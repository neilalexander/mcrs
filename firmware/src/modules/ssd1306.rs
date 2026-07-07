use embedded_hal::{
    digital::OutputPin,
    i2c::{ErrorType as I2cErrorType, I2c},
};
use embedded_hal_async::delay::DelayNs;

const I2C_ADDRESS: u8 = 0x3c;
const WIDTH: u8 = 128;
const PAGES: u8 = 8;

pub struct Display<I2C, RESET, POWER> {
    i2c: I2C,
    reset: RESET,
    power: POWER,
    power_active_level: PowerActiveLevel,
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum PowerActiveLevel {
    High,
    Low,
}

impl<I2C, RESET, POWER> Display<I2C, RESET, POWER>
where
    I2C: I2c + I2cErrorType,
    RESET: OutputPin,
    POWER: OutputPin,
{
    pub fn new_with_power_active_level(
        i2c: I2C,
        reset: RESET,
        power: POWER,
        power_active_level: PowerActiveLevel,
    ) -> Self {
        Self {
            i2c,
            reset,
            power,
            power_active_level,
        }
    }

    pub async fn init<DLY>(&mut self, delay: &mut DLY) -> Result<(), ()>
    where
        DLY: DelayNs,
    {
        self.set_power_enabled(true)?;
        delay.delay_ms(50).await;
        self.reset.set_low().map_err(pin_error)?;
        delay.delay_ms(10).await;
        self.reset.set_high().map_err(pin_error)?;
        delay.delay_ms(50).await;

        self.write_commands(&[
            0xae, 0xd5, 0x80, 0xa8, 0x3f, 0xd3, 0x00, 0x40, 0x8d, 0x14, 0x20, 0x00, 0xa1, 0xc8,
            0xda, 0x12, 0x81, 0xcf, 0xd9, 0xf1, 0xdb, 0x40, 0xa4, 0xa6, 0x2e, 0xaf,
        ])?;
        self.clear()
    }

    pub fn write_status(
        &mut self,
        node_name: &str,
        version: &str,
        public_key_prefix: [u8; 3],
        packets_sent: u32,
        packets_received: u32,
        packet_errors: u32,
        battery_millivolts: Option<u16>,
        battery_level_percent: Option<u8>,
        message: Option<&str>,
    ) -> Result<(), ()> {
        self.write_text_line(0, node_name)?;
        self.write_text_line_fmt(
            1,
            format_args!(
                "Prefix: {:02x}{:02x}{:02x}",
                public_key_prefix[0], public_key_prefix[1], public_key_prefix[2]
            ),
        )?;
        self.write_text_line_fmt(2, format_args!("Firmware {}", version))?;
        self.write_text_line_fmt(
            4,
            format_args!(
                "RX:{} TX:{} E:{}",
                packets_received, packets_sent, packet_errors
            ),
        )?;
        match (battery_millivolts, battery_level_percent) {
            (Some(millivolts), Some(percent)) => self.write_text_line_fmt(
                5,
                format_args!(
                    "Battery: {}.{:03}V {}%",
                    millivolts / 1000,
                    millivolts % 1000,
                    percent
                ),
            )?,
            (Some(millivolts), None) => self.write_text_line_fmt(
                5,
                format_args!("Battery: {}.{:03}V", millivolts / 1000, millivolts % 1000),
            )?,
            (None, Some(percent)) => {
                self.write_text_line_fmt(5, format_args!("Battery: {}%", percent))?
            }
            (None, None) => self.write_text_line(5, "Battery not known")?,
        }
        self.write_text_line(7, message.unwrap_or("Hold PRG for advert"))
    }

    pub fn sleep(&mut self) -> Result<(), ()> {
        self.write_commands(&[0xae])
    }

    pub fn wake(&mut self) -> Result<(), ()> {
        self.write_commands(&[0xaf])
    }

    pub fn clear(&mut self) -> Result<(), ()> {
        self.set_window(0, WIDTH - 1, 0, PAGES - 1)?;

        for _ in 0..PAGES {
            for _ in 0..8 {
                self.write_data(&[0; 16])?;
            }
        }

        Ok(())
    }

    pub fn write_text_line(&mut self, page: u8, text: &str) -> Result<(), ()> {
        self.clear_text_line(page)?;
        self.write_text_at(page, 0, text)
    }

    pub fn write_text_line_fmt(
        &mut self,
        page: u8,
        args: core::fmt::Arguments<'_>,
    ) -> Result<(), ()> {
        let mut line = TextLine::new();
        core::fmt::write(&mut line, args).map_err(unit_error)?;
        self.write_text_line(page, line.as_str())
    }

    pub fn write_text_at(&mut self, page: u8, column: u8, text: &str) -> Result<(), ()> {
        self.set_window(column, WIDTH - 1, page, page)?;

        for character in text.bytes() {
            let glyph = font_5x7(character);
            self.write_data(&glyph)?;
            self.write_data(&[0x00])?;
        }

        Ok(())
    }

    fn set_window(
        &mut self,
        start_col: u8,
        end_col: u8,
        start_page: u8,
        end_page: u8,
    ) -> Result<(), ()> {
        self.write_commands(&[0x21, start_col, end_col, 0x22, start_page, end_page])
    }

    fn write_commands(&mut self, commands: &[u8]) -> Result<(), ()> {
        let mut buffer = [0u8; 32];
        buffer[0] = 0x00;

        for chunk in commands.chunks(buffer.len() - 1) {
            buffer[1..=chunk.len()].copy_from_slice(chunk);
            self.i2c
                .write(I2C_ADDRESS, &buffer[..chunk.len() + 1])
                .map_err(i2c_error)?;
        }

        Ok(())
    }

    fn write_data(&mut self, data: &[u8]) -> Result<(), ()> {
        let mut buffer = [0u8; 17];
        buffer[0] = 0x40;

        for chunk in data.chunks(buffer.len() - 1) {
            buffer[1..=chunk.len()].copy_from_slice(chunk);
            self.i2c
                .write(I2C_ADDRESS, &buffer[..chunk.len() + 1])
                .map_err(i2c_error)?;
        }

        Ok(())
    }

    fn clear_text_line(&mut self, page: u8) -> Result<(), ()> {
        self.set_window(0, WIDTH - 1, page, page)?;
        for _ in 0..8 {
            self.write_data(&[0; 16])?;
        }
        Ok(())
    }

    fn set_power_enabled(&mut self, enabled: bool) -> Result<(), ()> {
        match (enabled, self.power_active_level) {
            (true, PowerActiveLevel::High) | (false, PowerActiveLevel::Low) => {
                self.power.set_high().map_err(pin_error)
            }
            (true, PowerActiveLevel::Low) | (false, PowerActiveLevel::High) => {
                self.power.set_low().map_err(pin_error)
            }
        }
    }
}

fn pin_error<E>(_error: E) {}

fn i2c_error<E>(_error: E) {}

fn unit_error<E>(_error: E) {}

struct TextLine {
    bytes: [u8; 21],
    len: usize,
}

impl TextLine {
    const fn new() -> Self {
        Self {
            bytes: [0; 21],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl core::fmt::Write for TextLine {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let remaining = self.bytes.len().saturating_sub(self.len);
        let copy_len = remaining.min(text.len());
        self.bytes[self.len..self.len + copy_len].copy_from_slice(&text.as_bytes()[..copy_len]);
        self.len += copy_len;
        Ok(())
    }
}

fn font_5x7(character: u8) -> [u8; 5] {
    match character {
        b' ' => [0x00, 0x00, 0x00, 0x00, 0x00],
        b'!' => [0x00, 0x00, 0x5f, 0x00, 0x00],
        b'"' => [0x00, 0x07, 0x00, 0x07, 0x00],
        b'#' => [0x14, 0x7f, 0x14, 0x7f, 0x14],
        b'$' => [0x24, 0x2a, 0x7f, 0x2a, 0x12],
        b'%' => [0x23, 0x13, 0x08, 0x64, 0x62],
        b'&' => [0x36, 0x49, 0x55, 0x22, 0x50],
        b'\'' => [0x00, 0x05, 0x03, 0x00, 0x00],
        b'(' => [0x00, 0x1c, 0x22, 0x41, 0x00],
        b')' => [0x00, 0x41, 0x22, 0x1c, 0x00],
        b'*' => [0x14, 0x08, 0x3e, 0x08, 0x14],
        b'+' => [0x08, 0x08, 0x3e, 0x08, 0x08],
        b',' => [0x00, 0x50, 0x30, 0x00, 0x00],
        b'-' => [0x08, 0x08, 0x08, 0x08, 0x08],
        b'.' => [0x00, 0x60, 0x60, 0x00, 0x00],
        b'/' => [0x20, 0x10, 0x08, 0x04, 0x02],
        b'0' => [0x3e, 0x51, 0x49, 0x45, 0x3e],
        b'1' => [0x00, 0x42, 0x7f, 0x40, 0x00],
        b'2' => [0x42, 0x61, 0x51, 0x49, 0x46],
        b'3' => [0x21, 0x41, 0x45, 0x4b, 0x31],
        b'4' => [0x18, 0x14, 0x12, 0x7f, 0x10],
        b'5' => [0x27, 0x45, 0x45, 0x45, 0x39],
        b'6' => [0x3c, 0x4a, 0x49, 0x49, 0x30],
        b'7' => [0x01, 0x71, 0x09, 0x05, 0x03],
        b'8' => [0x36, 0x49, 0x49, 0x49, 0x36],
        b'9' => [0x06, 0x49, 0x49, 0x29, 0x1e],
        b':' => [0x00, 0x36, 0x36, 0x00, 0x00],
        b';' => [0x00, 0x56, 0x36, 0x00, 0x00],
        b'<' => [0x08, 0x14, 0x22, 0x41, 0x00],
        b'=' => [0x14, 0x14, 0x14, 0x14, 0x14],
        b'>' => [0x00, 0x41, 0x22, 0x14, 0x08],
        b'?' => [0x02, 0x01, 0x51, 0x09, 0x06],
        b'@' => [0x32, 0x49, 0x79, 0x41, 0x3e],
        b'A' => [0x7e, 0x11, 0x11, 0x11, 0x7e],
        b'B' => [0x7f, 0x49, 0x49, 0x49, 0x36],
        b'C' => [0x3e, 0x41, 0x41, 0x41, 0x22],
        b'D' => [0x7f, 0x41, 0x41, 0x22, 0x1c],
        b'E' => [0x7f, 0x49, 0x49, 0x49, 0x41],
        b'F' => [0x7f, 0x09, 0x09, 0x09, 0x01],
        b'G' => [0x3e, 0x41, 0x49, 0x49, 0x7a],
        b'H' => [0x7f, 0x08, 0x08, 0x08, 0x7f],
        b'I' => [0x00, 0x41, 0x7f, 0x41, 0x00],
        b'J' => [0x20, 0x40, 0x41, 0x3f, 0x01],
        b'K' => [0x7f, 0x08, 0x14, 0x22, 0x41],
        b'L' => [0x7f, 0x40, 0x40, 0x40, 0x40],
        b'M' => [0x7f, 0x02, 0x0c, 0x02, 0x7f],
        b'N' => [0x7f, 0x04, 0x08, 0x10, 0x7f],
        b'O' => [0x3e, 0x41, 0x41, 0x41, 0x3e],
        b'P' => [0x7f, 0x09, 0x09, 0x09, 0x06],
        b'Q' => [0x3e, 0x41, 0x51, 0x21, 0x5e],
        b'R' => [0x7f, 0x09, 0x19, 0x29, 0x46],
        b'S' => [0x46, 0x49, 0x49, 0x49, 0x31],
        b'T' => [0x01, 0x01, 0x7f, 0x01, 0x01],
        b'U' => [0x3f, 0x40, 0x40, 0x40, 0x3f],
        b'V' => [0x1f, 0x20, 0x40, 0x20, 0x1f],
        b'W' => [0x3f, 0x40, 0x38, 0x40, 0x3f],
        b'X' => [0x63, 0x14, 0x08, 0x14, 0x63],
        b'Y' => [0x07, 0x08, 0x70, 0x08, 0x07],
        b'Z' => [0x61, 0x51, 0x49, 0x45, 0x43],
        b'[' => [0x00, 0x7f, 0x41, 0x41, 0x00],
        b'\\' => [0x02, 0x04, 0x08, 0x10, 0x20],
        b']' => [0x00, 0x41, 0x41, 0x7f, 0x00],
        b'^' => [0x04, 0x02, 0x01, 0x02, 0x04],
        b'_' => [0x40, 0x40, 0x40, 0x40, 0x40],
        b'`' => [0x00, 0x01, 0x02, 0x04, 0x00],
        b'a' => [0x20, 0x54, 0x54, 0x54, 0x78],
        b'b' => [0x7f, 0x48, 0x44, 0x44, 0x38],
        b'c' => [0x38, 0x44, 0x44, 0x44, 0x20],
        b'd' => [0x38, 0x44, 0x44, 0x48, 0x7f],
        b'e' => [0x38, 0x54, 0x54, 0x54, 0x18],
        b'f' => [0x08, 0x7e, 0x09, 0x01, 0x02],
        b'g' => [0x0c, 0x52, 0x52, 0x52, 0x3e],
        b'h' => [0x7f, 0x08, 0x04, 0x04, 0x78],
        b'i' => [0x00, 0x44, 0x7d, 0x40, 0x00],
        b'j' => [0x20, 0x40, 0x44, 0x3d, 0x00],
        b'k' => [0x7f, 0x10, 0x28, 0x44, 0x00],
        b'l' => [0x00, 0x41, 0x7f, 0x40, 0x00],
        b'm' => [0x7c, 0x04, 0x18, 0x04, 0x78],
        b'n' => [0x7c, 0x08, 0x04, 0x04, 0x78],
        b'o' => [0x38, 0x44, 0x44, 0x44, 0x38],
        b'p' => [0x7c, 0x14, 0x14, 0x14, 0x08],
        b'q' => [0x08, 0x14, 0x14, 0x18, 0x7c],
        b'r' => [0x7c, 0x08, 0x04, 0x04, 0x08],
        b's' => [0x48, 0x54, 0x54, 0x54, 0x20],
        b't' => [0x04, 0x3f, 0x44, 0x40, 0x20],
        b'u' => [0x3c, 0x40, 0x40, 0x20, 0x7c],
        b'v' => [0x1c, 0x20, 0x40, 0x20, 0x1c],
        b'w' => [0x3c, 0x40, 0x30, 0x40, 0x3c],
        b'x' => [0x44, 0x28, 0x10, 0x28, 0x44],
        b'y' => [0x0c, 0x50, 0x50, 0x50, 0x3c],
        b'z' => [0x44, 0x64, 0x54, 0x4c, 0x44],
        b'{' => [0x00, 0x08, 0x36, 0x41, 0x00],
        b'|' => [0x00, 0x00, 0x7f, 0x00, 0x00],
        b'}' => [0x00, 0x41, 0x36, 0x08, 0x00],
        b'~' => [0x08, 0x04, 0x08, 0x10, 0x08],
        _ => [0x00, 0x00, 0x00, 0x00, 0x00],
    }
}
