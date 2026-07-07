use alloc::vec::Vec;

use crate::{
    CIPHER_MAC_SIZE, Result,
    wire::{read_array, read_u8},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupEncryptedPayload {
    pub channel_hash: u8,
    pub mac: [u8; CIPHER_MAC_SIZE],
    pub ciphertext: Vec<u8>,
}

impl GroupEncryptedPayload {
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut offset = 0;
        Ok(Self {
            channel_hash: read_u8(input, &mut offset, "group channel_hash")?,
            mac: read_array(input, &mut offset, "group mac")?,
            ciphertext: input[offset..].to_vec(),
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.channel_hash);
        out.extend_from_slice(&self.mac);
        out.extend_from_slice(&self.ciphertext);
    }
}
