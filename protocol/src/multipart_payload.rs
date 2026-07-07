use alloc::vec::Vec;

use crate::{Error, PayloadKind, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartPayload {
    pub remaining: u8,
    pub sub_type: PayloadKind,
    pub sub_payload: Vec<u8>,
}

impl MultipartPayload {
    pub fn decode(input: &[u8]) -> Result<Self> {
        let packed = *input.first().ok_or(Error::Truncated("multipart payload"))?;
        Ok(Self {
            remaining: packed >> 4,
            sub_type: PayloadKind::from_nibble(packed),
            sub_payload: input[1..].to_vec(),
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.push(((self.remaining & 0x0f) << 4) | self.sub_type.to_nibble());
        out.extend_from_slice(&self.sub_payload);
    }
}
