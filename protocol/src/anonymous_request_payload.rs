use alloc::vec::Vec;

use crate::{
    CIPHER_MAC_SIZE, PUB_KEY_SIZE, Result,
    wire::{ensure_payload_len, read_array, read_u8},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnonymousRequestPayload {
    pub destination_hash: u8,
    pub sender_pubkey: [u8; PUB_KEY_SIZE],
    pub mac: [u8; CIPHER_MAC_SIZE],
    pub ciphertext: Vec<u8>,
}

impl AnonymousRequestPayload {
    pub fn decode(input: &[u8]) -> Result<Self> {
        ensure_payload_len(input)?;
        let mut offset = 0;
        Ok(Self {
            destination_hash: read_u8(input, &mut offset, "anonymous destination_hash")?,
            sender_pubkey: read_array(input, &mut offset, "anonymous sender_pubkey")?,
            mac: read_array(input, &mut offset, "anonymous mac")?,
            ciphertext: input[offset..].to_vec(),
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.destination_hash);
        out.extend_from_slice(&self.sender_pubkey);
        out.extend_from_slice(&self.mac);
        out.extend_from_slice(&self.ciphertext);
    }
}
