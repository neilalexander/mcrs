use alloc::vec::Vec;

use crate::{Error, Path, PayloadKind, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPlaintext {
    pub path: Path,
    pub extra_type: Option<PayloadKind>,
    pub extra_payload: Vec<u8>,
}

impl PathPlaintext {
    pub fn decode(input: &[u8]) -> Result<Self> {
        let path_length = *input.first().ok_or(Error::Truncated("path plaintext"))?;
        let (path, used) = Path::decode_wire(path_length, &input[1..])?;
        let extra_offset = 1 + used;
        let extra_type_byte = *input
            .get(extra_offset)
            .ok_or(Error::Truncated("path plaintext extra_type"))?;
        let extra_type = if extra_type_byte == 0xff {
            None
        } else {
            Some(PayloadKind::from_nibble(extra_type_byte))
        };
        Ok(Self {
            path,
            extra_type,
            extra_payload: input[extra_offset + 1..].to_vec(),
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        out.push(self.path.encoded_length_byte()?);
        out.extend_from_slice(self.path.bytes());
        out.push(self.extra_type.map_or(0xff, PayloadKind::to_nibble));
        out.extend_from_slice(&self.extra_payload);
        Ok(out)
    }
}
