extern crate alloc;

use alloc::{string::String, vec::Vec};
use mcrs_protocol::{
    AdvertAppData, AdvertNodeType, AdvertPayload, ControlMessage, ControlPayload, HashSize,
    MAX_ADVERT_DATA_SIZE, Packet, Path, Payload, RoutePath, RouteType,
};

use super::{AppContext, config::AppConfig};

pub const DISCOVER_NEIGHBOURS_TTL_MS: u64 = 60_000;

pub fn zero_hop_advert(config: &AppConfig) -> Option<Vec<u8>> {
    advert(config, RouteType::Direct)
}

pub fn flood_advert(config: &AppConfig) -> Option<Vec<u8>> {
    let mut packet = advert_packet(config, RouteType::Flood)?;
    let default_region = config
        .regions()
        .default_region()
        .map(|region| String::from(region.display_name()));
    match config.regions().apply_default_scope(&mut packet) {
        Ok(true) => match default_region {
            Some(region) => crate::platform::log_fmt(format_args!(
                "Advert: using default region scope region={}",
                region
            )),
            None => crate::platform::log_fmt(format_args!("Advert: using default region scope")),
        },
        Ok(false) => {}
        Err(error) => {
            crate::platform::log_fmt(format_args!(
                "Advert: default region scope failed: {}",
                error
            ));
            return None;
        }
    }
    packet.encode().ok()
}

fn advert(config: &AppConfig, route_type: RouteType) -> Option<Vec<u8>> {
    advert_packet(config, route_type)?.encode().ok()
}

fn advert_packet(config: &AppConfig, route_type: RouteType) -> Option<Packet> {
    crate::platform::log_fmt(format_args!(
        "Advert: sending {} name={}",
        route_type_name(route_type),
        config.node_name()
    ));

    let location = config.location();
    let app_data = AdvertAppData {
        node_type: AdvertNodeType::Repeater,
        location,
        feature1: None,
        feature2: None,
        name: advert_name(config.node_name(), location.is_some()),
    };
    let app_data_bytes = app_data.encode().ok()?;
    let timestamp = crate::platform::now_seconds();
    let public_key = *config.identity().public_key();

    let mut signed_message = Vec::new();
    signed_message.extend_from_slice(&public_key);
    signed_message.extend_from_slice(&timestamp.to_le_bytes());
    signed_message.extend_from_slice(&app_data_bytes);

    Some(Packet {
        route_type,
        transport_codes: None,
        path: RoutePath::Normal(advert_path(config)?),
        payload: Payload::Advert(AdvertPayload {
            public_key,
            timestamp,
            signature: super::identity::sign_with_seed(config.identity_seed(), &signed_message),
            app_data: Some(app_data),
        }),
    })
}

fn advert_path(config: &AppConfig) -> Option<Path> {
    Path::new(
        HashSize::from_code(config.path_hash_mode()).ok()?,
        Vec::new(),
    )
    .ok()
}

fn advert_name(name: &str, has_location: bool) -> Option<String> {
    let fixed_len = 1 + if has_location { 8 } else { 0 };
    let max_name_len = MAX_ADVERT_DATA_SIZE.saturating_sub(fixed_len);
    if max_name_len == 0 {
        return None;
    }

    let mut out = String::new();
    for character in name.chars() {
        if out.len() + character.len_utf8() > max_name_len {
            break;
        }
        out.push(character);
    }

    (!out.is_empty()).then_some(out)
}

fn route_type_name(route_type: RouteType) -> &'static str {
    match route_type {
        RouteType::TransportFlood => "TRANSPORT_FLOOD",
        RouteType::Flood => "FLOOD",
        RouteType::Direct => "DIRECT",
        RouteType::TransportDirect => "TRANSPORT_DIRECT",
    }
}

pub fn discover_neighbours_request(tag: u32) -> Option<Vec<u8>> {
    let type_filter = 1 << AdvertNodeType::Repeater.to_nibble();
    Packet {
        route_type: RouteType::Direct,
        transport_codes: None,
        path: RoutePath::Normal(Path::empty()),
        payload: Payload::Control(ControlPayload::discover_request(
            type_filter,
            tag,
            Some(0),
            false,
        )),
    }
    .encode()
    .ok()
}

pub async fn handle_control_packet<S>(
    packet: &Packet,
    context: &AppContext<S>,
    rssi: i16,
    snr_quarters: i16,
    now_ms: u64,
) -> Option<Vec<u8>>
where
    S: crate::platform::storage::Storage,
{
    if !is_zero_hop(packet) {
        return None;
    }

    let Payload::Control(payload) = &packet.payload else {
        return None;
    };

    match payload.message().ok()? {
        ControlMessage::DiscoverRequest {
            prefix_only,
            type_filter,
            tag,
            since,
        } => {
            handle_discover_request(context, snr_quarters, prefix_only, type_filter, tag, since)
                .await
        }
        ControlMessage::DiscoverResponse {
            node_type,
            tag,
            pubkey,
            ..
        } => {
            handle_discover_response(context, rssi, snr_quarters, now_ms, node_type, tag, &pubkey)
                .await;
            None
        }
        ControlMessage::Unknown(_) => None,
    }
}

async fn handle_discover_request<S>(
    context: &AppContext<S>,
    snr_quarters: i16,
    prefix_only: bool,
    type_filter: u8,
    tag: u32,
    since: Option<u32>,
) -> Option<Vec<u8>>
where
    S: crate::platform::storage::Storage,
{
    if type_filter & (1 << AdvertNodeType::Repeater.to_nibble()) == 0 {
        return None;
    }
    if since.is_some_and(|since| since > crate::platform::now_seconds()) {
        return None;
    }

    let public_key = context.public_key().await;
    let prefix_len = if prefix_only { 8 } else { public_key.len() };
    let payload = ControlPayload::discover_response(
        AdvertNodeType::Repeater,
        snr_quarters.clamp(i8::MIN as i16, i8::MAX as i16) as i8,
        tag,
        public_key[..prefix_len].to_vec(),
    );

    crate::platform::log_fmt(format_args!("Discover neighbours: response queued"));
    Packet {
        route_type: RouteType::Direct,
        transport_codes: None,
        path: RoutePath::Normal(Path::empty()),
        payload: Payload::Control(payload),
    }
    .encode()
    .ok()
}

async fn handle_discover_response<S>(
    context: &AppContext<S>,
    rssi: i16,
    snr_quarters: i16,
    now_ms: u64,
    node_type: AdvertNodeType,
    tag: u32,
    pubkey: &[u8],
) where
    S: crate::platform::storage::Storage,
{
    if node_type != AdvertNodeType::Repeater || pubkey.len() != mcrs_protocol::PUB_KEY_SIZE {
        return;
    }
    if !context.accept_discover_response(tag, now_ms) {
        return;
    }

    let mut public_key = [0; mcrs_protocol::PUB_KEY_SIZE];
    public_key.copy_from_slice(pubkey);
    context
        .observe_neighbour_public_key(public_key, rssi, snr_quarters, now_ms)
        .await;
}

fn is_zero_hop(packet: &Packet) -> bool {
    let RoutePath::Normal(path) = &packet.path else {
        return false;
    };
    packet.route_type.is_direct() && path.hop_count() == 0
}
