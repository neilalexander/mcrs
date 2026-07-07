#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteType {
    TransportFlood,
    Flood,
    Direct,
    TransportDirect,
}

impl RouteType {
    pub fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0x00 => Self::TransportFlood,
            0x01 => Self::Flood,
            0x02 => Self::Direct,
            _ => Self::TransportDirect,
        }
    }

    pub fn to_bits(self) -> u8 {
        match self {
            Self::TransportFlood => 0x00,
            Self::Flood => 0x01,
            Self::Direct => 0x02,
            Self::TransportDirect => 0x03,
        }
    }

    pub fn has_transport_codes(self) -> bool {
        matches!(self, Self::TransportFlood | Self::TransportDirect)
    }

    pub fn is_flood(self) -> bool {
        matches!(self, Self::Flood | Self::TransportFlood)
    }

    pub fn is_direct(self) -> bool {
        matches!(self, Self::Direct | Self::TransportDirect)
    }
}
