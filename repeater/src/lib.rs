//! Platform-independent firmware transport components.
#![no_std]
extern crate alloc;

pub mod app;
pub mod memory;
pub mod modules;
pub mod platform;

pub mod radio_metrics;

pub const VERSION: &str = env!("MESHCORE_FIRMWARE_VERSION");
