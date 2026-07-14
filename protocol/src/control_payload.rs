use alloc::{vec, vec::Vec};

use crate::{
    AdvertNodeType, ControlMessage, Error, Result,
    wire::{ensure_payload_len, read_u8, read_u32_le},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPayload {
    pub flags: u8,
    pub data: Vec<u8>,
}

impl ControlPayload {
    pub fn new(sub_type: u8, sub_data: u8, data: Vec<u8>) -> Self {
        Self {
            flags: ((sub_type & 0x0f) << 4) | (sub_data & 0x0f),
            data,
        }
    }

    pub fn discover_request(
        type_filter: u8,
        tag: u32,
        since: Option<u32>,
        prefix_only: bool,
    ) -> Self {
        let mut data = vec![type_filter];
        data.extend_from_slice(&tag.to_le_bytes());
        if let Some(since) = since {
            data.extend_from_slice(&since.to_le_bytes());
        }
        Self::new(0x08, u8::from(prefix_only), data)
    }

    pub fn discover_response(
        node_type: AdvertNodeType,
        snr_quarters: i8,
        tag: u32,
        pubkey: Vec<u8>,
    ) -> Self {
        let mut data = vec![snr_quarters as u8];
        data.extend_from_slice(&tag.to_le_bytes());
        data.extend_from_slice(&pubkey);
        Self::new(0x09, node_type.to_nibble(), data)
    }

    pub fn decode(input: &[u8]) -> Result<Self> {
        ensure_payload_len(input)?;
        let flags = *input.first().ok_or(Error::Truncated("control payload"))?;
        Ok(Self {
            flags,
            data: input[1..].to_vec(),
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.flags);
        out.extend_from_slice(&self.data);
    }

    pub fn sub_type(&self) -> u8 {
        self.flags >> 4
    }

    pub fn sub_data(&self) -> u8 {
        self.flags & 0x0f
    }

    pub fn zero_hop_only(&self) -> bool {
        self.flags & 0x80 != 0
    }

    pub fn message(&self) -> Result<ControlMessage> {
        match self.sub_type() {
            0x08 => {
                if self.data.len() != 5 && self.data.len() != 9 {
                    return Err(Error::InvalidLength("discover request"));
                }
                let mut offset = 0;
                let type_filter = read_u8(&self.data, &mut offset, "discover type_filter")?;
                let tag = read_u32_le(&self.data, &mut offset, "discover tag")?;
                let since = if self.data.len() == 9 {
                    Some(read_u32_le(&self.data, &mut offset, "discover since")?)
                } else {
                    None
                };
                Ok(ControlMessage::DiscoverRequest {
                    prefix_only: self.sub_data() & 0x01 != 0,
                    type_filter,
                    tag,
                    since,
                })
            }
            0x09 => {
                if self.data.len() != 13 && self.data.len() != 37 {
                    return Err(Error::InvalidLength("discover response"));
                }
                let mut offset = 0;
                let snr_quarters = read_u8(&self.data, &mut offset, "discover snr")? as i8;
                let tag = read_u32_le(&self.data, &mut offset, "discover tag")?;
                Ok(ControlMessage::DiscoverResponse {
                    node_type: AdvertNodeType::from_nibble(self.sub_data()),
                    snr_quarters,
                    tag,
                    pubkey: self.data[offset..].to_vec(),
                })
            }
            _ => Ok(ControlMessage::Unknown(self.clone())),
        }
    }
}
