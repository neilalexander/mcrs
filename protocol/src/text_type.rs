#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextType {
    Plain,
    CliData,
    SignedPlain,
    Reserved(u8),
}

impl TextType {
    pub fn from_bits(bits: u8) -> Self {
        match bits & 0x3f {
            0 => Self::Plain,
            1 => Self::CliData,
            2 => Self::SignedPlain,
            other => Self::Reserved(other),
        }
    }

    pub fn to_bits(self) -> u8 {
        match self {
            Self::Plain => 0,
            Self::CliData => 1,
            Self::SignedPlain => 2,
            Self::Reserved(n) => n & 0x3f,
        }
    }
}
