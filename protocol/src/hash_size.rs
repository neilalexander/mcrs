use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashSize {
    One,
    Two,
    Three,
}

impl HashSize {
    pub fn new(size: usize) -> Result<Self> {
        match size {
            1 => Ok(Self::One),
            2 => Ok(Self::Two),
            3 => Ok(Self::Three),
            _ => Err(Error::InvalidHashSize(size)),
        }
    }

    pub fn from_code(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Self::One),
            1 => Ok(Self::Two),
            2 => Ok(Self::Three),
            _ => Err(Error::InvalidHashSizeCode(code)),
        }
    }

    pub fn size(self) -> usize {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
        }
    }

    pub fn code(self) -> u8 {
        match self {
            Self::One => 0,
            Self::Two => 1,
            Self::Three => 2,
        }
    }
}
