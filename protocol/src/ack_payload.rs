use alloc::vec::Vec;

use crate::{Error, Result, wire::read_array};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckPayload {
    pub ack_hash: [u8; 4],
}

impl AckPayload {
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength("ack payload"));
        }
        let mut offset = 0;
        Ok(Self {
            ack_hash: read_array(input, &mut offset, "ack payload")?,
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.ack_hash);
    }
}
