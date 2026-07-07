use crate::{Error, PayloadKind, Result, RouteType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub payload_version: u8,
    pub payload_kind: PayloadKind,
    pub route_type: RouteType,
}

impl Header {
    pub fn decode(byte: u8) -> Result<Self> {
        if byte == 0xff {
            return Err(Error::InvalidHeaderSentinel);
        }

        let payload_version = (byte >> 6) + 1;
        if payload_version != 1 {
            return Err(Error::UnsupportedPayloadVersion(payload_version));
        }

        Ok(Self {
            payload_version,
            payload_kind: PayloadKind::from_nibble((byte >> 2) & 0x0f),
            route_type: RouteType::from_bits(byte),
        })
    }

    pub fn encode(self) -> Result<u8> {
        if self.payload_version != 1 {
            return Err(Error::UnsupportedPayloadVersion(self.payload_version));
        }

        Ok(((self.payload_version - 1) << 6)
            | (self.payload_kind.to_nibble() << 2)
            | self.route_type.to_bits())
    }
}
