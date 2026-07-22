use alloc::collections::VecDeque;

#[derive(Debug)]
pub struct SeenPacketCache {
    ttl_ticks: u64,
    max_entries: usize,
    entries: VecDeque<([u8; 8], u64)>,
}

impl SeenPacketCache {
    pub fn new(ttl_ticks: u64) -> Self {
        Self::new_with_capacity(ttl_ticks, usize::MAX)
    }

    pub fn new_with_capacity(ttl_ticks: u64, max_entries: usize) -> Self {
        Self {
            ttl_ticks,
            max_entries,
            entries: VecDeque::new(),
        }
    }

    pub fn check_and_insert(&mut self, signature: [u8; 8], now_ticks: u64) -> bool {
        if self.contains(signature, now_ticks) {
            return false;
        }

        self.touch(signature, now_ticks);
        true
    }

    pub fn contains(&mut self, signature: [u8; 8], now_ticks: u64) -> bool {
        self.prune(now_ticks);
        self.entries
            .iter()
            .any(|(seen_signature, _)| *seen_signature == signature)
    }

    pub fn touch(&mut self, signature: [u8; 8], now_ticks: u64) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|(seen_signature, _)| *seen_signature == signature)
        {
            self.entries.remove(index);
        }
        if self.max_entries == 0 {
            return;
        }
        while self.entries.len() >= self.max_entries {
            self.entries.pop_front();
        }
        self.entries.push_back((signature, now_ticks));
    }

    pub fn prune(&mut self, now_ticks: u64) {
        while let Some((_, inserted)) = self.entries.front().copied() {
            if now_ticks.saturating_sub(inserted) <= self.ttl_ticks {
                break;
            }
            self.entries.pop_front();
        }
    }
}
