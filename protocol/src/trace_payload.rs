use alloc::vec::Vec;

use crate::{
    Error, Result, TraceHashSize,
    wire::{ensure_payload_len, read_u8, read_u32_le},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracePayload {
    pub tag: u32,
    pub auth_code: u32,
    pub hash_size: TraceHashSize,
    pub path_hashes: Vec<u8>,
}

impl TracePayload {
    pub fn decode(input: &[u8]) -> Result<Self> {
        ensure_payload_len(input)?;
        let mut offset = 0;
        let tag = read_u32_le(input, &mut offset, "trace tag")?;
        let auth_code = read_u32_le(input, &mut offset, "trace auth_code")?;
        let flags = read_u8(input, &mut offset, "trace flags")?;
        if flags & 0xfc != 0 {
            return Err(Error::InvalidTraceFlags(flags));
        }
        let hash_size = TraceHashSize::from_bits(flags)?;
        let path_hashes = input[offset..].to_vec();
        if !path_hashes.len().is_multiple_of(hash_size.size()) {
            return Err(Error::InvalidLength("trace path_hashes"));
        }

        Ok(Self {
            tag,
            auth_code,
            hash_size,
            path_hashes,
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        if !self.path_hashes.len().is_multiple_of(self.hash_size.size()) {
            return Err(Error::InvalidLength("trace path_hashes"));
        }
        out.extend_from_slice(&self.tag.to_le_bytes());
        out.extend_from_slice(&self.auth_code.to_le_bytes());
        out.push(self.hash_size.bits());
        out.extend_from_slice(&self.path_hashes);
        Ok(())
    }
}
