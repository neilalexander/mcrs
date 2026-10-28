use crate::platform::storage::Storage;

pub mod storage;

pub trait Platform {
    //fn storage() -> S;

    fn reboot(&self) -> !;

    fn now_millis(&self) -> u64;

    fn now_seconds(&self) -> u32;

    fn set_wall_clock_if_forward(&self, unix_seconds: u32) -> bool;

    fn random_u32(&self) -> u32;

    fn battery_level_percent(&self) -> Option<u8> {
        None
    }

    fn battery_millivolts(&self) -> Option<u16> {
        None
    }
}
