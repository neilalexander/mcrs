#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryProfile {
    pub heap_size: usize,
    pub inbound_queue_len: usize,
    pub outbound_queue_len: usize,
    pub max_neighbours: usize,
    pub seen_packet_cache_len: usize,
}

impl MemoryProfile {
    pub const fn new(
        heap_size: usize,
        inbound_queue_len: usize,
        outbound_queue_len: usize,
        max_neighbours: usize,
        seen_packet_cache_len: usize,
    ) -> Self {
        Self {
            heap_size,
            inbound_queue_len,
            outbound_queue_len,
            max_neighbours,
            seen_packet_cache_len,
        }
    }
}
