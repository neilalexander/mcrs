use mcrs_protocol::{
    AdvertNodeType, ControlMessage, DirectEncryptedPayload, Error, GroupEncryptedPayload, Packet,
    Payload, PayloadKind, RoutePath, RouteType, TraceHashSize,
};

const VERBOSE_PROTOCOL_LOGS: bool = false;

pub fn log_decode_failed(error: &Error) {
    crate::platform::log_fmt(format_args!("Protocol decode failed: {}", error));
}

pub fn log_packet(packet: &Packet) {
    crate::platform::log_fmt(format_args!(
        "Protocol packet: route={}, payload={}",
        route_type_name(packet.route_type),
        payload_kind_name(packet.payload.kind())
    ));

    if !VERBOSE_PROTOCOL_LOGS {
        return;
    }

    if let Some(codes) = packet.transport_codes {
        crate::platform::log_fmt(format_args!(
            "Protocol transport: primary=0x{:04x}, secondary=0x{:04x}",
            codes.primary, codes.secondary
        ));
    }

    log_path(&packet.path);
    log_payload(&packet.payload);
}

fn log_path(path: &RoutePath) {
    match path {
        RoutePath::Normal(path) => {
            crate::platform::log_fmt(format_args!(
                "Protocol path: NORMAL hops={}, hash size={} bytes",
                path.hop_count(),
                path.hash_size().size()
            ));
            if let Some(first_hash) = path.first_hash() {
                crate::platform::log_hex_line("Protocol path first hash:", first_hash, 8);
            }
        }
        RoutePath::Trace(path) => {
            crate::platform::log_fmt(format_args!(
                "Protocol path: TRACE consumed hops={}, SNR samples={}",
                path.consumed_hops,
                path.snr_samples.len()
            ));
            if !path.snr_samples.is_empty() {
                crate::platform::log_i8_line("Protocol TRACE SNR:", &path.snr_samples, 16);
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
            crate::platform::log_hex_line("Protocol ACK: hash=", &payload.ack_hash, 4);
        }
        Payload::Advert(payload) => {
            crate::platform::log_fmt(format_args!(
                "Protocol ADVERT: timestamp={}",
                payload.timestamp
            ));
            crate::platform::log_hex_line("Protocol ADVERT pubkey:", &payload.public_key, 8);
            if let Some(app_data) = &payload.app_data {
                crate::platform::log_fmt(format_args!(
                    "Protocol ADVERT app: node type={}",
                    advert_node_type_name(app_data.node_type)
                ));
                if let Some((latitude, longitude)) = app_data.location {
                    crate::platform::log_fmt(format_args!(
                        "Protocol ADVERT location: latitude={}, longitude={}",
                        latitude, longitude
                    ));
                }
                if let Some(feature) = app_data.feature1 {
                    crate::platform::log_fmt(format_args!(
                        "Protocol ADVERT feature1=0x{:04x}",
                        feature
                    ));
                }
                if let Some(feature) = app_data.feature2 {
                    crate::platform::log_fmt(format_args!(
                        "Protocol ADVERT feature2=0x{:04x}",
                        feature
                    ));
                }
                if let Some(name) = &app_data.name {
                    crate::platform::log_fmt(format_args!("Protocol ADVERT name={}", name));
                }
            }
        }
        Payload::GroupText(payload) => log_group_payload("GROUP_TEXT", payload),
        Payload::GroupData(payload) => log_group_payload("GROUP_DATA", payload),
        Payload::AnonymousRequest(payload) => {
            crate::platform::log_fmt(format_args!(
                "Protocol ANONYMOUS_REQUEST: dst=0x{:02x}, ciphertext={} bytes",
                payload.destination_hash,
                payload.ciphertext.len()
            ));
            crate::platform::log_hex_line(
                "Protocol ANONYMOUS_REQUEST sender pubkey:",
                &payload.sender_pubkey,
                8,
            );
            crate::platform::log_hex_line("Protocol ANONYMOUS_REQUEST MAC=", &payload.mac, 2);
        }
        Payload::Trace(payload) => {
            crate::platform::log_fmt(format_args!(
                "Protocol TRACE: tag=0x{:08x}, auth=0x{:08x}, hash size={} bytes, path hashes={}",
                payload.tag,
                payload.auth_code,
                trace_hash_size_bytes(payload.hash_size),
                payload.path_hashes.len() / trace_hash_size_bytes(payload.hash_size)
            ));
            crate::platform::log_hex_line("Protocol TRACE hashes:", &payload.path_hashes, 16);
        }
        Payload::Multipart(payload) => {
            crate::platform::log_fmt(format_args!(
                "Protocol MULTIPART: remaining={}, subtype={}, sub-payload={} bytes",
                payload.remaining,
                payload_kind_name(payload.sub_type),
                payload.sub_payload.len()
            ));
            crate::platform::log_hex_line(
                "Protocol MULTIPART sub-payload:",
                &payload.sub_payload,
                32,
            );
        }
        Payload::Control(payload) => {
            crate::platform::log_fmt(format_args!(
                "Protocol CONTROL: subtype=0x{:02x}, sub-data=0x{:02x}, zero-hop-only={}, data={} bytes",
                payload.sub_type(),
                payload.sub_data(),
                payload.zero_hop_only(),
                payload.data.len()
            ));
            match payload.message() {
                Ok(ControlMessage::DiscoverRequest {
                    prefix_only,
                    type_filter,
                    tag,
                    since,
                }) => {
                    crate::platform::log_fmt(format_args!(
                        "Protocol CONTROL DISCOVER_REQUEST: prefix-only={}, type-filter=0x{:02x}, tag=0x{:08x}",
                        prefix_only, type_filter, tag
                    ));
                    if let Some(since) = since {
                        crate::platform::log_fmt(format_args!(
                            "Protocol CONTROL DISCOVER_REQUEST since={}",
                            since
                        ));
                    }
                }
                Ok(ControlMessage::DiscoverResponse {
                    node_type,
                    snr_quarters,
                    tag,
                    pubkey,
                }) => {
                    crate::platform::log_fmt(format_args!(
                        "Protocol CONTROL DISCOVER_RESPONSE: node-type={}, SNR quarters={}, tag=0x{:08x}, pubkey={} bytes",
                        advert_node_type_name(node_type),
                        snr_quarters,
                        tag,
                        pubkey.len()
                    ));
                    crate::platform::log_hex_line(
                        "Protocol CONTROL DISCOVER_RESPONSE pubkey:",
                        &pubkey,
                        8,
                    );
                }
                Ok(ControlMessage::Unknown(_)) => {
                    crate::platform::log_hex_line(
                        "Protocol CONTROL UNKNOWN data:",
                        &payload.data,
                        32,
                    );
                }
                Err(error) => {
                    crate::platform::log_fmt(format_args!(
                        "Protocol CONTROL message decode failed: {}",
                        error
                    ));
                }
            }
        }
        Payload::Reserved { kind, bytes } => {
            crate::platform::log_fmt(format_args!(
                "Protocol RESERVED payload: kind=0x{:02x}, len={}",
                kind,
                bytes.len()
            ));
            crate::platform::log_hex_line("Protocol RESERVED payload bytes:", bytes, 32);
        }
        Payload::RawCustom(bytes) => {
            crate::platform::log_fmt(format_args!("Protocol RAW_CUSTOM: len={}", bytes.len()));
            crate::platform::log_hex_line("Protocol RAW_CUSTOM bytes:", bytes, 32);
        }
    }
}

fn log_direct_payload(name: &str, payload: &DirectEncryptedPayload) {
    crate::platform::log_fmt(format_args!(
        "Protocol {}: dst=0x{:02x}, src=0x{:02x}, ciphertext={} bytes",
        name,
        payload.destination_hash,
        payload.source_hash,
        payload.ciphertext.len()
    ));
    crate::platform::log_fmt(format_args!(
        "Protocol {} MAC= {:02x} {:02x}",
        name, payload.mac[0], payload.mac[1]
    ));
}

fn log_group_payload(name: &str, payload: &GroupEncryptedPayload) {
    crate::platform::log_fmt(format_args!(
        "Protocol {}: channel=0x{:02x}, ciphertext={} bytes",
        name,
        payload.channel_hash,
        payload.ciphertext.len()
    ));
    crate::platform::log_fmt(format_args!(
        "Protocol {} MAC= {:02x} {:02x}",
        name, payload.mac[0], payload.mac[1]
    ));
}

fn route_type_name(route_type: RouteType) -> &'static str {
    match route_type {
        RouteType::TransportFlood => "TRANSPORT_FLOOD",
        RouteType::Flood => "FLOOD",
        RouteType::Direct => "DIRECT",
        RouteType::TransportDirect => "TRANSPORT_DIRECT",
    }
}

fn payload_kind_name(kind: PayloadKind) -> &'static str {
    match kind {
        PayloadKind::Request => "REQUEST",
        PayloadKind::Response => "RESPONSE",
        PayloadKind::TextMessage => "TEXT_MESSAGE",
        PayloadKind::Ack => "ACK",
        PayloadKind::Advert => "ADVERT",
        PayloadKind::GroupText => "GROUP_TEXT",
        PayloadKind::GroupData => "GROUP_DATA",
        PayloadKind::AnonymousRequest => "ANONYMOUS_REQUEST",
        PayloadKind::Path => "PATH",
        PayloadKind::Trace => "TRACE",
        PayloadKind::Multipart => "MULTIPART",
        PayloadKind::Control => "CONTROL",
        PayloadKind::Reserved(_) => "RESERVED",
        PayloadKind::RawCustom => "RAW_CUSTOM",
    }
}

fn advert_node_type_name(node_type: AdvertNodeType) -> &'static str {
    match node_type {
        AdvertNodeType::None => "NONE",
        AdvertNodeType::Chat => "CHAT",
        AdvertNodeType::Repeater => "REPEATER",
        AdvertNodeType::Room => "ROOM",
        AdvertNodeType::Sensor => "SENSOR",
        AdvertNodeType::Reserved(_) => "RESERVED",
    }
}

fn trace_hash_size_bytes(hash_size: TraceHashSize) -> usize {
    match hash_size {
        TraceHashSize::One => 1,
        TraceHashSize::Two => 2,
        TraceHashSize::Four => 4,
    }
}
