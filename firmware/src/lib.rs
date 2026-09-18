//! Platform-independent firmware transport components.
#![no_std]
extern crate alloc;

#[cfg(feature = "mqtt")]
#[path = "app/mqtt/transport.rs"]
pub mod mqtt_transport;

#[cfg(feature = "mqtt")]
#[path = "app/mqtt.rs"]
pub mod mqtt;

pub mod radio_metrics;

#[cfg(test)]
mod crypto_tests;
#[cfg(test)]
use crypto_tests::platform;
