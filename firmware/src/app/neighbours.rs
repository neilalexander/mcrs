extern crate alloc;

use alloc::vec::Vec;
use core::fmt::Write;
use mcrs_protocol::{AdvertNodeType, PUB_KEY_SIZE, Packet, Payload, RoutePath, node_hash};

const MAX_NEIGHBOUR_HASH_LEN: usize = 4;
const MAX_NEIGHBOUR_RESULTS_BYTES: usize = 130;

#[derive(Clone, Copy)]
pub struct Neighbour {
    public_key: [u8; PUB_KEY_SIZE],
    last_seen_ms: u64,
    last_rssi: i16,
    last_snr: i16,
    packet_count: u32,
}

pub struct NeighbourTable {
    entries: Vec<Neighbour>,
    capacity: usize,
}

impl NeighbourTable {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            capacity,
        }
    }

    pub fn observe_packet(
        &mut self,
        packet: &Packet,
        rssi: i16,
        snr: i16,
        now_ms: u64,
        local_node_hash: &[u8],
    ) {
        let Some(observation) = ImmediateNeighbour::from_packet(packet) else {
            return;
        };

        if observation.matches(local_node_hash) {
            return;
        }

        self.observe_public_key(observation.public_key, rssi, snr, now_ms, local_node_hash);
    }

    pub fn observe_public_key(
        &mut self,
        public_key: [u8; PUB_KEY_SIZE],
        rssi: i16,
        snr: i16,
        now_ms: u64,
        local_node_hash: &[u8],
    ) {
        if public_key_matches_hash(&public_key, local_node_hash) {
            return;
        }

        match self.find_index(&public_key) {
            Some(index) => {
                let neighbour = &mut self.entries[index];
                neighbour.last_seen_ms = now_ms;
                neighbour.last_rssi = rssi;
                neighbour.last_snr = snr;
                neighbour.packet_count = neighbour.packet_count.saturating_add(1);
            }
            None => {
                if self.capacity == 0 {
                    return;
                }

                let neighbour = Neighbour {
                    public_key,
                    last_seen_ms: now_ms,
                    last_rssi: rssi,
                    last_snr: snr,
                    packet_count: 1,
                };

                if self.entries.len() < self.capacity {
                    self.entries.push(neighbour);
                } else {
                    let index = self.oldest_index();
                    self.entries[index] = neighbour;
                }
                log_discovered(neighbour);
            }
        }
    }

    pub fn encode_binary_response(
        &self,
        request: &[u8],
        now_ms: u64,
        out: &mut alloc::vec::Vec<u8>,
    ) -> bool {
        if request.len() < 7 || request[0] != 0x06 || request[1] != 0 {
            return false;
        }

        let count = request[2] as usize;
        let offset = u16::from_le_bytes([request[3], request[4]]) as usize;
        let order_by = request[5];
        let public_key_prefix_len = (request[6] as usize).min(PUB_KEY_SIZE);

        let mut neighbours = self.neighbours();
        sort_neighbours(&mut neighbours, order_by);

        let total = neighbours.len().min(u16::MAX as usize) as u16;
        let mut results_count = 0u16;
        let mut results_bytes = 0usize;

        let response_start = out.len();
        out.extend_from_slice(&total.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());

        for neighbour in neighbours.iter().skip(offset).take(count) {
            let entry_len = public_key_prefix_len + 4 + 1;
            if results_bytes + entry_len > MAX_NEIGHBOUR_RESULTS_BYTES {
                break;
            }

            out.extend_from_slice(&neighbour.public_key[..public_key_prefix_len]);
            let heard_seconds_ago = now_ms.saturating_sub(neighbour.last_seen_ms) / 1000;
            out.extend_from_slice(&(heard_seconds_ago.min(u32::MAX as u64) as u32).to_le_bytes());
            out.push(neighbour.last_snr.clamp(i8::MIN as i16, i8::MAX as i16) as u8);
            results_count = results_count.saturating_add(1);
            results_bytes += entry_len;
        }

        let result_count_offset = response_start + 2;
        out[result_count_offset..result_count_offset + 2]
            .copy_from_slice(&results_count.to_le_bytes());

        true
    }

    pub fn write_summary(&self, output: &mut impl Write) {
        let count = self.entries.len();
        let _ = writeln!(output, "Neighbours: count={}", count);

        for neighbour in &self.entries {
            let _ = write!(output, "Neighbour: hash=",);
            append_hex(output, &neighbour.hash());
            let _ = writeln!(
                output,
                " rssi={} snr={} packets={}",
                neighbour.last_rssi, neighbour.last_snr, neighbour.packet_count
            );
        }
    }

    fn neighbours(&self) -> alloc::vec::Vec<Neighbour> {
        self.entries.clone()
    }

    fn find_index(&self, public_key: &[u8; PUB_KEY_SIZE]) -> Option<usize> {
        self.entries
            .iter()
            .position(|neighbour| &neighbour.public_key == public_key)
    }

    fn oldest_index(&self) -> usize {
        let mut oldest_index = 0;
        let mut oldest_seen = u64::MAX;

        for (index, neighbour) in self.entries.iter().enumerate() {
            if neighbour.last_seen_ms < oldest_seen {
                oldest_index = index;
                oldest_seen = neighbour.last_seen_ms;
            }
        }

        oldest_index
    }
}

fn append_hex(output: &mut impl Write, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    for byte in bytes {
        let _ = output.write_char(HEX[(byte >> 4) as usize] as char);
        let _ = output.write_char(HEX[(byte & 0x0f) as usize] as char);
    }
}

impl Neighbour {
    fn hash(&self) -> [u8; MAX_NEIGHBOUR_HASH_LEN] {
        node_hash::<MAX_NEIGHBOUR_HASH_LEN>(&self.public_key)
    }
}

#[derive(Clone, Copy)]
struct ImmediateNeighbour {
    public_key: [u8; PUB_KEY_SIZE],
}

impl ImmediateNeighbour {
    fn from_packet(packet: &Packet) -> Option<Self> {
        let RoutePath::Normal(path) = &packet.path else {
            return None;
        };

        if !(packet.route_type.is_flood() || packet.route_type.is_direct()) || path.hop_count() != 0
        {
            return None;
        }

        let Payload::Advert(advert) = &packet.payload else {
            return None;
        };
        let Some(app_data) = &advert.app_data else {
            return None;
        };
        if app_data.node_type != AdvertNodeType::Repeater {
            return None;
        }
        if !advert.verify_signature() {
            return None;
        }

        Some(Self {
            public_key: advert.public_key,
        })
    }

    fn matches(&self, hash: &[u8]) -> bool {
        public_key_matches_hash(&self.public_key, hash)
    }
}

fn public_key_matches_hash(public_key: &[u8; PUB_KEY_SIZE], hash: &[u8]) -> bool {
    let own_hash = node_hash::<MAX_NEIGHBOUR_HASH_LEN>(public_key);
    hash.len() >= own_hash.len() && hash[..own_hash.len()] == own_hash
}

fn sort_neighbours(neighbours: &mut [Neighbour], order_by: u8) {
    for i in 0..neighbours.len() {
        let mut selected = i;
        for j in i + 1..neighbours.len() {
            if neighbour_precedes(neighbours[j], neighbours[selected], order_by) {
                selected = j;
            }
        }
        neighbours.swap(i, selected);
    }
}

fn neighbour_precedes(left: Neighbour, right: Neighbour, order_by: u8) -> bool {
    match order_by {
        1 => left.last_seen_ms < right.last_seen_ms,
        2 => left.last_snr > right.last_snr,
        3 => left.last_snr < right.last_snr,
        _ => left.last_seen_ms > right.last_seen_ms,
    }
}

fn log_discovered(neighbour: Neighbour) {
    crate::platform::log_fmt(format_args!(
        "Neighbour discovered: RSSI={} SNR={}",
        neighbour.last_rssi, neighbour.last_snr
    ));
    crate::platform::log_hex_line("Neighbour hash:", &neighbour.hash(), MAX_NEIGHBOUR_HASH_LEN);
    crate::platform::log_hex_line("Neighbour pubkey:", &neighbour.public_key, 8);
}
