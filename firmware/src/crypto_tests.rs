//! Run the firmware crypto and MeshCore identity vectors on the host.
#![allow(dead_code)]

extern crate alloc;

#[path = "app/acl.rs"]
mod acl;

#[path = "app/crypto.rs"]
mod crypto;
#[path = "app/identity.rs"]
pub(crate) mod identity;
#[path = "app/remote.rs"]
mod remote;

// Response builders need a clock. Use a fixed time without linking ESP hardware.
pub(crate) mod platform {
    pub fn now_millis() -> u64 {
        1_000
    }

    pub fn now_seconds() -> u32 {
        1
    }
}
