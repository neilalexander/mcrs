#![no_std]

extern crate alloc;

pub mod board;
pub mod esp;
mod littlefs;
mod ota;
pub mod platform;

pub use mcrs_repeater::Repeater;
pub use platform::EspPlatform;
