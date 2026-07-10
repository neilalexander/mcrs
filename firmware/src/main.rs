#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]

extern crate alloc;

#[cfg(not(any(
    feature = "board-heltec-v3",
    feature = "board-heltec-v4",
    feature = "board-heltec-wsl3"
)))]
compile_error!(
    "Select firmware board feature: board-heltec-v3, board-heltec-v4, or board-heltec-wsl3"
);

#[cfg(any(
    all(feature = "board-heltec-v3", feature = "board-heltec-v4"),
    all(feature = "board-heltec-v3", feature = "board-heltec-wsl3"),
    all(feature = "board-heltec-v4", feature = "board-heltec-wsl3")
))]
compile_error!("Select only one firmware board feature");

mod app;
mod board;
mod memory;
mod modules;
mod platform;
