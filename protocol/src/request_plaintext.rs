use alloc::vec::Vec;

use crate::{
    Result,
    wire::{ensure_payload_len, read_u32_le},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestPlaintext {
    pub timestamp: u32,
    pub request_data: Vec<u8>,
}

impl RequestPlaintext {
    pub fn decode(input: &[u8]) -> Result<Self> {
        ensure_payload_len(input)?;
        let mut offset = 0;
        Ok(Self {
            timestamp: read_u32_le(input, &mut offset, "request timestamp")?,
            request_data: input[offset..].to_vec(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.request_data.len());
        out.extend_from_slice(&self.timestamp.to_le_bytes());
        out.extend_from_slice(&self.request_data);
        out
    }
}
