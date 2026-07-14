use alloc::vec::Vec;

use crate::{
    CIPHER_BLOCK_SIZE, CIPHER_MAC_SIZE, Result,
    wire::{ensure_payload_len, read_array, read_u8},
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
        ensure_payload_len(input)?;
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

    /// Whether ciphertext can be safely processed by the AES block decryptor.
    ///
    /// A valid MAC alone is not sufficient: AES decryption consumes complete
    /// 16-byte blocks and an empty ciphertext is not a valid encrypted payload.
    pub fn has_complete_ciphertext_blocks(&self) -> bool {
        !self.ciphertext.is_empty() && self.ciphertext.len().is_multiple_of(CIPHER_BLOCK_SIZE)
    }
}
