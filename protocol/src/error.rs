use core::fmt;

use crate::PayloadKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    EmptyPacket,
    InvalidHeaderSentinel,
    UnsupportedPayloadVersion(u8),
    PacketTooLong {
        len: usize,
    },
    PayloadTooLong {
        len: usize,
    },
    PathTooLong {
        len: usize,
    },
    InvalidHashSizeCode(u8),
    InvalidHashSize(usize),
    InvalidPathLength,
    InvalidTracePathLength(u8),
    InvalidTraceFlags(u8),
    Truncated(&'static str),
    InvalidLength(&'static str),
    InvalidUtf8,
    MissingTransportCodes,
    UnexpectedTransportCodes,
    InvalidZeroHopControlRoute,
    PayloadKindMismatch {
        expected: PayloadKind,
        actual: PayloadKind,
    },
    PathKindMismatch,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPacket => write!(f, "packet is empty"),
            Self::InvalidHeaderSentinel => write!(f, "0xff is not a valid on-wire header"),
            Self::UnsupportedPayloadVersion(version) => {
                write!(f, "unsupported payload version {version}")
            }
            Self::PacketTooLong { len } => write!(f, "packet length {len} exceeds limit"),
            Self::PayloadTooLong { len } => write!(f, "payload length {len} exceeds limit"),
            Self::PathTooLong { len } => write!(f, "path length {len} exceeds limit"),
            Self::InvalidHashSizeCode(code) => write!(f, "invalid hash size code {code}"),
            Self::InvalidHashSize(size) => write!(f, "invalid hash size {size}"),
            Self::InvalidPathLength => write!(f, "invalid path length"),
            Self::InvalidTracePathLength(length) => write!(f, "invalid trace path length {length}"),
            Self::InvalidTraceFlags(flags) => write!(f, "invalid trace flags 0x{flags:02x}"),
            Self::Truncated(field) => write!(f, "truncated {field}"),
            Self::InvalidLength(field) => write!(f, "invalid length for {field}"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8"),
            Self::MissingTransportCodes => write!(f, "transport route is missing transport codes"),
            Self::UnexpectedTransportCodes => {
                write!(f, "non-transport route includes transport codes")
            }
            Self::InvalidZeroHopControlRoute => {
                write!(f, "zero-hop control packet is not direct zero-hop")
            }
            Self::PayloadKindMismatch { expected, actual } => {
                write!(
                    f,
                    "payload kind mismatch: expected {expected:?}, got {actual:?}"
                )
            }
            Self::PathKindMismatch => write!(f, "packet path kind does not match payload kind"),
        }
    }
}
