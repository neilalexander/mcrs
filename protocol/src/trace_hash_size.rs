use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceHashSize {
    One,
    Two,
    Four,
}

impl TraceHashSize {
    pub fn from_bits(bits: u8) -> Result<Self> {
        match bits & 0x03 {
            0 => Ok(Self::One),
            1 => Ok(Self::Two),
            2 => Ok(Self::Four),
            _ => Err(Error::InvalidTraceFlags(bits)),
        }
    }

    pub fn size(self) -> usize {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Four => 4,
        }
    }

    pub fn bits(self) -> u8 {
        match self {
            Self::One => 0,
            Self::Two => 1,
            Self::Four => 2,
        }
    }
}
