//! Platform-independent repeater firmware. A board crate implements
//! [`Platform`] and runs a [`Repeater`] on it.
#![no_std]
extern crate alloc;

pub mod app;
pub mod memory;
pub mod modules;
pub mod platform;

pub mod radio_metrics;
mod repeater;

pub use platform::Platform;
pub use repeater::Repeater;

pub const VERSION: &str = env!("MESHCORE_FIRMWARE_VERSION");
