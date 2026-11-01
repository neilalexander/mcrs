use {
    crate::{AdvertNodeType, ControlPayload},
    alloc::vec::Vec,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMessage {
    DiscoverRequest {
        prefix_only: bool,
        type_filter: u8,
        tag: u32,
        since: Option<u32>,
    },
    DiscoverResponse {
        node_type: AdvertNodeType,
        snr_quarters: i8,
        tag: u32,
        pubkey: Vec<u8>,
    },
    Unknown(ControlPayload),
}
