use defmt::{debug, info};
use mcrs_protocol::{
    ControlMessage, DirectEncryptedPayload, GroupEncryptedPayload, Packet, Payload, RoutePath,
};

const VERBOSE_PROTOCOL_LOGS: bool = false;

pub fn log_packet(packet: &Packet, region: Option<&str>) {
    match region {
        Some(region) => info!(
            "Protocol packet: route={}, payload={}, region={}",
            packet.route_type,
            packet.payload.kind(),
            region
        ),
        None => info!(
            "Protocol packet: route={}, payload={}",
            packet.route_type,
            packet.payload.kind()
        ),
    }

    if !VERBOSE_PROTOCOL_LOGS {
        return;
    }

    if let Some(codes) = packet.transport_codes {
        debug!(
            "Protocol transport: primary=0x{:04x}, secondary=0x{:04x}",
            codes.primary, codes.secondary
        );
    }

    log_path(&packet.path);
    log_payload(&packet.payload);
}

fn log_path(path: &RoutePath) {
    match path {
        RoutePath::Normal(path) => {
            debug!(
                "Protocol path: NORMAL hops={}, hash size={} bytes",
                path.hop_count(),
                path.hash_size().size()
            );
            if let Some(first_hash) = path.first_hash() {
                debug!("Protocol path first hash: {:x}", first_hash);
            }
        }
        RoutePath::Trace(path) => {
            debug!(
                "Protocol path: TRACE consumed hops={}, SNR samples={}",
                path.consumed_hops,
                path.snr_samples.len()
            );
            if !path.snr_samples.is_empty() {
                debug!("Protocol TRACE SNR: {:?}", path.snr_samples);
            }
        }
    }
}

fn log_payload(payload: &Payload) {
    match payload {
        Payload::Request(payload) => log_direct_payload("REQUEST", payload),
        Payload::Response(payload) => log_direct_payload("RESPONSE", payload),
        Payload::TextMessage(payload) => log_direct_payload("TEXT_MESSAGE", payload),
        Payload::Path(payload) => log_direct_payload("PATH", payload),
        Payload::Ack(payload) => {
            debug!("Protocol ACK: hash={:x}", payload.ack_hash);
        }
        Payload::Advert(payload) => {
            debug!("Protocol ADVERT: timestamp={}", payload.timestamp);
            debug!("Protocol ADVERT pubkey: {:x}", payload.public_key);
            if let Some(app_data) = &payload.app_data {
                debug!("Protocol ADVERT app: node type={}", app_data.node_type);
                if let Some((latitude, longitude)) = app_data.location {
                    debug!(
                        "Protocol ADVERT location: latitude={}, longitude={}",
                        latitude, longitude
                    );
                }
                if let Some(feature) = app_data.feature1 {
                    debug!("Protocol ADVERT feature1=0x{:04x}", feature);
                }
                if let Some(feature) = app_data.feature2 {
                    debug!("Protocol ADVERT feature2=0x{:04x}", feature);
                }
                if let Some(name) = &app_data.name {
                    debug!("Protocol ADVERT name={}", name);
                }
            }
        }
        Payload::GroupText(payload) => log_group_payload("GROUP_TEXT", payload),
        Payload::GroupData(payload) => log_group_payload("GROUP_DATA", payload),
        Payload::AnonymousRequest(payload) => {
            debug!(
                "Protocol ANONYMOUS_REQUEST: dst=0x{:02x}, ciphertext={} bytes",
                payload.destination_hash,
                payload.ciphertext.len()
            );
            debug!(
                "Protocol ANONYMOUS_REQUEST sender pubkey: {:x}",
                payload.sender_pubkey,
            );
            debug!("Protocol ANONYMOUS_REQUEST MAC= {:x}", payload.mac);
        }
        Payload::Trace(payload) => {
            debug!(
                "Protocol TRACE: tag=0x{:08x}, auth=0x{:08x}, hash size={} bytes, path hashes={}",
                payload.tag,
                payload.auth_code,
                payload.hash_size.size(),
                payload.path_hashes.len() / payload.hash_size.size()
            );
            debug!("Protocol TRACE hashes: {:x}", payload.path_hashes);
        }
        Payload::Multipart(payload) => {
            debug!(
                "Protocol MULTIPART: remaining={}, subtype={}, sub-payload={} bytes",
                payload.remaining,
                payload.sub_type,
                payload.sub_payload.len()
            );
            debug!("Protocol MULTIPART sub-payload: {:x}", payload.sub_payload);
        }
        Payload::Control(payload) => {
            debug!(
                "Protocol CONTROL: subtype=0x{:02x}, sub-data=0x{:02x}, zero-hop-only={}, data={} bytes",
                payload.sub_type(),
                payload.sub_data(),
                payload.zero_hop_only(),
                payload.data.len()
            );
            match payload.message() {
                Ok(ControlMessage::DiscoverRequest {
                    prefix_only,
                    type_filter,
                    tag,
                    since,
                }) => {
                    debug!(
                        "Protocol CONTROL DISCOVER_REQUEST: prefix-only={}, type-filter=0x{:02x}, tag=0x{:08x}",
                        prefix_only, type_filter, tag
                    );
                    if let Some(since) = since {
                        debug!("Protocol CONTROL DISCOVER_REQUEST since={}", since);
                    }
                }
                Ok(ControlMessage::DiscoverResponse {
                    node_type,
                    snr_quarters,
                    tag,
                    pubkey,
                }) => {
                    debug!(
                        "Protocol CONTROL DISCOVER_RESPONSE: node-type={}, SNR quarters={}, tag=0x{:08x}, pubkey={} bytes",
                        node_type,
                        snr_quarters,
                        tag,
                        pubkey.len()
                    );
                    debug!("Protocol CONTROL DISCOVER_RESPONSE pubkey: {:x}", &pubkey,);
                }
                Ok(ControlMessage::Unknown(_)) => {
                    debug!("Protocol CONTROL UNKNOWN data: {:x}", &payload.data);
                }
                Err(error) => {
                    debug!("Protocol CONTROL message decode failed: {}", error);
                }
            }
        }
        Payload::Reserved { kind, bytes } => {
            debug!(
                "Protocol RESERVED payload: kind=0x{:02x}, len={}",
                kind,
                bytes.len()
            );
            debug!("Protocol RESERVED payload bytes: {:x}", bytes);
        }
        Payload::RawCustom(bytes) => {
            debug!("Protocol RAW_CUSTOM: len={}", bytes.len());
            debug!("Protocol RAW_CUSTOM bytes: {:x}", bytes);
        }
    }
}

fn log_direct_payload(name: &str, payload: &DirectEncryptedPayload) {
    debug!(
        "Protocol {}: dst=0x{:02x}, src=0x{:02x}, ciphertext={} bytes",
        name,
        payload.destination_hash,
        payload.source_hash,
        payload.ciphertext.len()
    );
    debug!(
        "Protocol {} MAC= {:02x} {:02x}",
        name, payload.mac[0], payload.mac[1]
    );
}

fn log_group_payload(name: &str, payload: &GroupEncryptedPayload) {
    debug!(
        "Protocol {}: channel=0x{:02x}, ciphertext={} bytes",
        name,
        payload.channel_hash,
        payload.ciphertext.len()
    );
    debug!(
        "Protocol {} MAC= {:02x} {:02x}",
        name, payload.mac[0], payload.mac[1]
    );
}
