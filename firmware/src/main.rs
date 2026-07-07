#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]

extern crate alloc;

#[cfg(not(any(feature = "board-heltec-v3", feature = "board-heltec-v4")))]
compile_error!("Select firmware board feature: board-heltec-v3 or board-heltec-v4");

#[cfg(all(feature = "board-heltec-v3", feature = "board-heltec-v4"))]
compile_error!("Select only one firmware board feature");

mod app;
mod board;
mod memory;
mod modules;
mod platform;
