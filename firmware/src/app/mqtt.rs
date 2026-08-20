//! Minimal MQTT 3.1.1 packet reporter, modelled on MeshCore-MQTT.

use alloc::{format, string::String, vec, vec::Vec};
use core::fmt::Write as _;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ConnectionState {
    Disabled = 0,
    Disconnected = 1,
    Connecting = 2,
    Connected = 3,
}

impl ConnectionState {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Disabled,
            2 => Self::Connecting,
            3 => Self::Connected,
            _ => Self::Disconnected,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Disconnected => "disconnected",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Direction {
    Rx,
    Tx,
}

pub struct PacketEvent {
    pub direction: Direction,
    pub payload: Vec<u8>,
    pub rssi: i16,
    pub snr: i16,
}

impl PacketEvent {
    pub fn new(direction: Direction, payload: &[u8], rssi: i16, snr: i16) -> Self {
        Self {
            direction,
            payload: payload.into(),
            rssi,
            snr,
        }
    }
}

pub fn packets_topic(config: &super::config::MqttConfig, public_key: &[u8; 32]) -> String {
    let key = hex(public_key);
    config
        .topic_root
        .replace("{IATA}", &config.iata)
        .replace("<IATA>", &config.iata)
        .replace("{PUBLIC_KEY}", &key)
        .replace("<PUBLIC_KEY>", &key)
}

pub fn status_topic(packets: &str) -> String {
    packets
        .strip_suffix("/packets")
        .map(|v| format!("{v}/status"))
        .unwrap_or_else(|| packets.into())
}

pub fn packet_json(event: &PacketEvent) -> String {
    let direction = match event.direction {
        Direction::Rx => "rx",
        Direction::Tx => "tx",
    };
    format!(
        "{{\"direction\":\"{direction}\",\"raw\":\"{}\",\"rssi\":{},\"snr\":{}}}",
        hex(&event.payload),
        event.rssi,
        event.snr
    )
}

pub fn connect_packet(
    config: &super::config::MqttConfig,
    client_id: &str,
    will_topic: &str,
    will_payload: &str,
) -> Vec<u8> {
    let mut body = Vec::new();
    field(&mut body, b"MQTT");
    body.push(4);
    let mut flags = 0x26; // clean session, retained QoS 0 last will
    if !config.username.is_empty() {
        flags |= 0x80;
    }
    if !config.password.is_empty() {
        flags |= 0x40;
    }
    body.push(flags);
    body.extend_from_slice(&60u16.to_be_bytes());
    field(&mut body, client_id.as_bytes());
    field(&mut body, will_topic.as_bytes());
    field(&mut body, will_payload.as_bytes());
    if !config.username.is_empty() {
        field(&mut body, config.username.as_bytes());
    }
    if !config.password.is_empty() {
        field(&mut body, config.password.as_bytes());
    }
    frame(0x10, &body)
}

pub fn publish_packet(topic: &str, payload: &str, retain: bool) -> Vec<u8> {
    let mut body = Vec::new();
    field(&mut body, topic.as_bytes());
    body.extend_from_slice(payload.as_bytes());
    frame(if retain { 0x31 } else { 0x30 }, &body)
}

fn frame(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    let mut remaining = body.len();
    loop {
        let mut byte = (remaining % 128) as u8;
        remaining /= 128;
        if remaining > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if remaining == 0 {
            break;
        }
    }
    out.extend_from_slice(body);
    out
}
fn field(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value);
}
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}
