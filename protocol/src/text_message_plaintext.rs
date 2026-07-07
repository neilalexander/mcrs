use alloc::vec::Vec;

use crate::{
    Result, TextType,
    wire::{read_u8, read_u32_le},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextMessagePlaintext {
    pub timestamp: u32,
    pub text_type: TextType,
    pub attempt: u8,
    pub message: Vec<u8>,
}

impl TextMessagePlaintext {
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut offset = 0;
        let timestamp = read_u32_le(input, &mut offset, "text timestamp")?;
        let packed = read_u8(input, &mut offset, "text type_attempt")?;
        Ok(Self {
            timestamp,
            text_type: TextType::from_bits(packed >> 2),
            attempt: packed & 0x03,
            message: input[offset..].to_vec(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + self.message.len());
        out.extend_from_slice(&self.timestamp.to_le_bytes());
        out.push((self.text_type.to_bits() << 2) | (self.attempt & 0x03));
        out.extend_from_slice(&self.message);
        out
    }
}
