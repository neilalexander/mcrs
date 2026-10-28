//! Platform-independent firmware transport components.
#![no_std]
extern crate alloc;

pub mod app;
pub mod memory;
pub mod modules;
pub mod platform;

pub mod radio_metrics;

#[cfg(test)]
mod crypto_tests;
#[cfg(test)]
use crypto_tests::platform;

pub const VERSION: &str = env!("MESHCORE_FIRMWARE_VERSION");
