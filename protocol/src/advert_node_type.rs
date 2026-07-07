#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdvertNodeType {
    None,
    Chat,
    Repeater,
    Room,
    Sensor,
    Reserved(u8),
}

impl AdvertNodeType {
    pub fn from_nibble(nibble: u8) -> Self {
        match nibble & 0x0f {
            0 => Self::None,
            1 => Self::Chat,
            2 => Self::Repeater,
            3 => Self::Room,
            4 => Self::Sensor,
            other => Self::Reserved(other),
        }
    }

    pub fn to_nibble(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Chat => 1,
            Self::Repeater => 2,
            Self::Room => 3,
            Self::Sensor => 4,
            Self::Reserved(n) => n & 0x0f,
        }
    }
}
