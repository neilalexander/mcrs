use crate::{Error, Result};
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq, defmt::Format)]
pub struct AckPayload {
    pub ack_hash: Vec<u8>,
}

impl AckPayload {
    pub fn decode(input: &[u8]) -> Result<Self> {
        if !(4..=6).contains(&input.len()) {
            return Err(Error::InvalidLength("ack payload"));
        }
        Ok(Self {
            ack_hash: input.to_vec(),
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        if !(4..=6).contains(&self.ack_hash.len()) {
            return Err(Error::InvalidLength("ack payload"));
        }
        out.extend_from_slice(&self.ack_hash);
        Ok(())
    }
}
