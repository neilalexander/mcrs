#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PayloadKind {
    Request,
    Response,
    TextMessage,
    Ack,
    Advert,
    GroupText,
    GroupData,
    AnonymousRequest,
    Path,
    Trace,
    Multipart,
    Control,
    Reserved(u8),
    RawCustom,
}

impl PayloadKind {
    pub fn from_nibble(nibble: u8) -> Self {
        match nibble & 0x0f {
            0x00 => Self::Request,
            0x01 => Self::Response,
            0x02 => Self::TextMessage,
            0x03 => Self::Ack,
            0x04 => Self::Advert,
            0x05 => Self::GroupText,
            0x06 => Self::GroupData,
            0x07 => Self::AnonymousRequest,
            0x08 => Self::Path,
            0x09 => Self::Trace,
            0x0a => Self::Multipart,
            0x0b => Self::Control,
            0x0f => Self::RawCustom,
            other => Self::Reserved(other),
        }
    }

    pub fn to_nibble(self) -> u8 {
        match self {
            Self::Request => 0x00,
            Self::Response => 0x01,
            Self::TextMessage => 0x02,
            Self::Ack => 0x03,
            Self::Advert => 0x04,
            Self::GroupText => 0x05,
            Self::GroupData => 0x06,
            Self::AnonymousRequest => 0x07,
            Self::Path => 0x08,
            Self::Trace => 0x09,
            Self::Multipart => 0x0a,
            Self::Control => 0x0b,
            Self::Reserved(n) => n & 0x0f,
            Self::RawCustom => 0x0f,
        }
    }

    pub fn is_direct_encrypted(self) -> bool {
        matches!(
            self,
            Self::Request | Self::Response | Self::TextMessage | Self::Path
        )
    }
}
