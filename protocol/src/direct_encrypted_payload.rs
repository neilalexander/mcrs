use alloc::vec::Vec;

use crate::{
    CIPHER_MAC_SIZE, Result,
    wire::{read_array, read_u8},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectEncryptedPayload {
    pub destination_hash: u8,
    pub source_hash: u8,
    pub mac: [u8; CIPHER_MAC_SIZE],
    pub ciphertext: Vec<u8>,
}

impl DirectEncryptedPayload {
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut offset = 0;
        Ok(Self {
            destination_hash: read_u8(input, &mut offset, "direct destination_hash")?,
            source_hash: read_u8(input, &mut offset, "direct source_hash")?,
            mac: read_array(input, &mut offset, "direct mac")?,
            ciphertext: input[offset..].to_vec(),
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.destination_hash);
        out.push(self.source_hash);
        out.extend_from_slice(&self.mac);
        out.extend_from_slice(&self.ciphertext);
    }
}
