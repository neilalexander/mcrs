use alloc::vec::Vec;

use crate::{
    Result,
    wire::{ensure_payload_len, read_u32_le},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeaterResponsePlaintext {
    pub reflected_tag: u32,
    pub responder_time: u32,
    pub body: Vec<u8>,
}

impl RepeaterResponsePlaintext {
    pub fn decode(input: &[u8]) -> Result<Self> {
        ensure_payload_len(input)?;
        let mut offset = 0;
        Ok(Self {
            reflected_tag: read_u32_le(input, &mut offset, "repeater response tag")?,
            responder_time: read_u32_le(input, &mut offset, "repeater response time")?,
            body: input[offset..].to_vec(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.body.len());
        out.extend_from_slice(&self.reflected_tag.to_le_bytes());
        out.extend_from_slice(&self.responder_time.to_le_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}
