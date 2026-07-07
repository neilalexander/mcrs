use alloc::vec::Vec;

use crate::{Result, wire::read_u16_le};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportCodes {
    pub primary: u16,
    pub secondary: u16,
}

impl TransportCodes {
    pub fn new(primary: u16) -> Self {
        Self {
            primary,
            secondary: 0,
        }
    }

    pub(crate) fn decode(input: &[u8]) -> Result<Self> {
        let mut offset = 0;

        Ok(Self {
            primary: read_u16_le(input, &mut offset, "transport primary")?,
            secondary: read_u16_le(input, &mut offset, "transport secondary")?,
        })
    }

    pub(crate) fn encode(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.primary.to_le_bytes());
        out.extend_from_slice(&self.secondary.to_le_bytes());
    }
}
