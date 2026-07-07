use alloc::vec::Vec;

use crate::{
    Error, RESP_SERVER_LOGIN_OK, Result,
    wire::{read_array, read_u8, read_u32_le},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepeaterLoginResponsePlaintext {
    pub server_timestamp: u32,
    pub keep_alive_interval: u8,
    pub legacy_permissions: u8,
    pub acl_permissions: u8,
    pub nonce: [u8; 4],
    pub firmware_version_level: u8,
}

impl RepeaterLoginResponsePlaintext {
    pub const ENCODED_LEN: usize = 13;

    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut offset = 0;
        let server_timestamp = read_u32_le(input, &mut offset, "repeater login timestamp")?;
        let status = read_u8(input, &mut offset, "repeater login status")?;
        if status != RESP_SERVER_LOGIN_OK {
            return Err(Error::InvalidLength("repeater login response status"));
        }

        Ok(Self {
            server_timestamp,
            keep_alive_interval: read_u8(input, &mut offset, "repeater login keep_alive")?,
            legacy_permissions: read_u8(input, &mut offset, "repeater login legacy_permissions")?,
            acl_permissions: read_u8(input, &mut offset, "repeater login acl_permissions")?,
            nonce: read_array(input, &mut offset, "repeater login nonce")?,
            firmware_version_level: read_u8(input, &mut offset, "repeater login firmware")?,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::ENCODED_LEN);
        out.extend_from_slice(&self.server_timestamp.to_le_bytes());
        out.push(RESP_SERVER_LOGIN_OK);
        out.push(self.keep_alive_interval);
        out.push(self.legacy_permissions);
        out.push(self.acl_permissions);
        out.extend_from_slice(&self.nonce);
        out.push(self.firmware_version_level);
        out
    }
}
