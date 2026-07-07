use alloc::vec::Vec;

use crate::{
    Error, Path, Result,
    wire::{read_u8, read_u32_le},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeaterSubRequestPlaintext {
    pub timestamp: u32,
    pub req_type: u8,
    pub reply_path: Path,
}

impl RepeaterSubRequestPlaintext {
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut offset = 0;
        let timestamp = read_u32_le(input, &mut offset, "repeater sub-request timestamp")?;
        let req_type = read_u8(input, &mut offset, "repeater sub-request type")?;
        let path_length = read_u8(input, &mut offset, "repeater sub-request reply_path_length")?;
        let (reply_path, used) = Path::decode_wire(path_length, &input[offset..])?;
        offset += used;
        if offset != input.len() {
            return Err(Error::InvalidLength("repeater sub-request reply_path"));
        }
        Ok(Self {
            timestamp,
            req_type,
            reply_path,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.timestamp.to_le_bytes());
        out.push(self.req_type);
        out.push(self.reply_path.encoded_length_byte()?);
        out.extend_from_slice(self.reply_path.bytes());
        Ok(out)
    }
}
