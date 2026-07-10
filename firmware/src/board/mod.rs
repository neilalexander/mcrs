mod heltec;
#[cfg(any(feature = "board-heltec-v3", feature = "board-heltec-wsl3"))]
mod heltec_v3;
#[cfg(feature = "board-heltec-v4")]
mod heltec_v4;

pub(crate) use heltec::{MEMORY_PROFILE, STORAGE_LAYOUT};
