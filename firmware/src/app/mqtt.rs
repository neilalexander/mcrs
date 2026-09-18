//! Minimal MQTT 3.1.1 packet reporter, modelled on MeshCore-MQTT.

pub use crate::mqtt_transport as transport;

use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use core::fmt::Write as _;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthMode {
    #[default]
    Password,
    Device,
}

impl AuthMode {
    pub fn parse(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("password") {
            Some(Self::Password)
        } else if value.eq_ignore_ascii_case("device") {
            Some(Self::Device)
        } else {
            None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Device => "device",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MqttConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub auth: AuthMode,
    pub auth_audience: String,
    pub topic_root: String,
    pub iata: String,
}

impl MqttConfig {
    pub fn value(&self, key: &str) -> Option<String> {
        Some(match key {
            "host" => self.host.clone(),
            "port" => self.port.to_string(),
            "username" => self.username.clone(),
            "password" => self.password.clone(),
            "auth" => self.auth.as_str().into(),
            "auth.audience" | "audience" => self.auth_audience.clone(),
            "topic.root" => self.topic_root.clone(),
            "iata" => self.iata.clone(),
            _ => return None,
        })
    }

    /// Resolve credentials for one CONNECT. Tokens are never persisted and must
    /// be regenerated on reconnect. The broker uses a hex signature, not the
    /// base64url signature of a standard JWT.
    pub fn credentials(
        &self,
        public_key: &[u8; 32],
        now: u32,
        sign: impl FnOnce(&[u8]) -> [u8; 64],
    ) -> Result<(String, String), ()> {
        if self.auth == AuthMode::Password {
            return Ok((self.username.clone(), self.password.clone()));
        }
        // Before clock synchronization, the platform returns uptime. Accept a
        // retained or manually set wall clock too (2024-01-01 or later).
        if now < 1_704_067_200 {
            return Err(());
        }
        let expires = now.checked_add(3600).ok_or(())?;
        let endpoint = transport::Endpoint::parse(&self.host, self.port)?;
        let audience = if self.auth_audience.is_empty() {
            endpoint.host
        } else {
            &self.auth_audience
        };
        let key = hex(public_key);
        let mut escaped = String::new();
        for c in audience.chars() {
            match c {
                '"' => escaped.push_str("\\\""),
                '\\' => escaped.push_str("\\\\"),
                c if c < ' ' => {
                    let _ = write!(escaped, "\\u{:04x}", c as u32);
                }
                c => escaped.push(c),
            }
        }
        let payload = format!(
            "{{\"publicKey\":\"{key}\",\"aud\":\"{escaped}\",\"iat\":{now},\"exp\":{expires}}}"
        );
        let mut token = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"Ed25519","typ":"JWT"}"#),
            URL_SAFE_NO_PAD.encode(payload.as_bytes())
        );
        let signature = sign(token.as_bytes());
        token.push('.');
        token.push_str(&hex(&signature));
        Ok((format!("v1_{key}"), token))
    }
}

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

/// Coarse failure reasons, safe to expose without credentials or tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionError {
    Unreachable,
    Tls,
    Auth,
    Clock,
    WebSocket,
    Protocol,
    Server,
    Memory,
    Connection,
    Config,
}

impl ConnectionError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::Tls => "TLS error",
            Self::Auth => "auth error",
            Self::Clock => "clock not set",
            Self::WebSocket => "WebSocket error",
            Self::Protocol => "protocol error",
            Self::Server => "server unavailable",
            Self::Memory => "out of memory",
            Self::Connection => "connection lost or timed out",
            Self::Config => "invalid config",
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
    pub snr_quarters: i16,
}

impl PacketEvent {
    pub fn new(direction: Direction, payload: &[u8], rssi: i16, snr_quarters: i16) -> Self {
        Self {
            direction,
            payload: payload.into(),
            rssi,
            snr_quarters,
        }
    }
}

pub fn packets_topic(config: &MqttConfig, public_key: &[u8; 32]) -> String {
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

pub fn packet_json(event: &PacketEvent, public_key: &[u8; 32]) -> String {
    let direction = match event.direction {
        Direction::Rx => "rx",
        Direction::Tx => "tx",
    };
    format!(
        "{{\"origin_id\":\"{}\",\"type\":\"PACKET\",\"direction\":\"{direction}\",\"raw\":\"{}\",\"RSSI\":{},\"SNR\":{}}}",
        hex(public_key),
        hex(&event.payload),
        event.rssi,
        crate::radio_metrics::SnrDb(event.snr_quarters)
    )
}

pub fn status_json(public_key: &[u8; 32], online: bool) -> String {
    let status = if online { "online" } else { "offline" };
    format!(
        "{{\"status\":\"{status}\",\"origin_id\":\"{}\"}}",
        hex(public_key)
    )
}

pub fn connect_packet(
    credentials: (&str, &str),
    client_id: &str,
    will_topic: &str,
    will_payload: &str,
) -> Vec<u8> {
    let (username, password) = credentials;
    let mut body = Vec::new();
    field(&mut body, b"MQTT");
    body.push(4);
    let mut flags = 0x26; // clean session, retained QoS 0 last will
    if !username.is_empty() {
        flags |= 0x80;
    }
    if !password.is_empty() {
        flags |= 0x40;
    }
    body.push(flags);
    body.extend_from_slice(&60u16.to_be_bytes());
    field(&mut body, client_id.as_bytes());
    field(&mut body, will_topic.as_bytes());
    field(&mut body, will_payload.as_bytes());
    if !username.is_empty() {
        field(&mut body, username.as_bytes());
    }
    if !password.is_empty() {
        field(&mut body, password.as_bytes());
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
        let _ = write!(out, "{b:02X}");
    }
    out
}

// Fixed-size responses are sufficient for this clean-session, QoS 0 publisher.
// read_exact handles TCP fragmentation without consuming the next MQTT packet.
pub async fn read_connack(
    reader: &mut impl embedded_io_async::Read,
) -> Result<(), ConnectionError> {
    let mut packet = [0; 4];
    reader
        .read_exact(&mut packet)
        .await
        .map_err(|_| ConnectionError::Connection)?;
    if packet[..3] != [0x20, 0x02, 0x00] {
        return Err(ConnectionError::Protocol);
    }
    match packet[3] {
        0 => Ok(()),
        4 | 5 => Err(ConnectionError::Auth),
        3 => Err(ConnectionError::Server),
        _ => Err(ConnectionError::Protocol),
    }
}

pub async fn read_pingresp(reader: &mut impl embedded_io_async::Read) -> Result<(), ()> {
    let mut packet = [0; 2];
    reader.read_exact(&mut packet).await.map_err(|_| ())?;
    (packet == [0xd0, 0x00]).then_some(()).ok_or(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    #[test]
    fn device_credentials_match_meshcore_format_and_verify_for_both_key_formats() {
        use crate::crypto_tests::identity::{Identity, PrivateKey};
        use ed25519_dalek::{Signature, VerifyingKey};
        let expanded = PrivateKey::from_hex("28ad39fefd7fa3e200a9c626eef599e61a2d055c48a8288a4e7e4c4bca3928789c7d6db3506d65dbac7c052aaee4857425210c9bc54030c826e54055983452a5").unwrap();
        let mut previous = None;
        for private in [PrivateKey::Seed([7; 32]), expanded] {
            let identity = Identity::from_private_key(private);
            let config = MqttConfig {
                host: "wss://mqtt.meshrank.net:443/mqtt".into(),
                auth: AuthMode::Device,
                username: "ignored".into(),
                password: "ignored".into(),
                ..Default::default()
            };
            let credentials = config
                .credentials(identity.public_key(), 1_800_000_000, |m| identity.sign(m))
                .unwrap();
            let (username, token) = &credentials;
            assert_eq!(
                username,
                "v1_EA4A6C63E29C520ABEF5507B132EC5F9954776AEBEBE7B92421EEA691446D22C"
            );
            let parts: Vec<_> = token.split('.').collect();
            assert_eq!(parts.len(), 3);
            assert_eq!(parts[0], "eyJhbGciOiJFZDI1NTE5IiwidHlwIjoiSldUIn0");
            assert!(!parts[1].contains('='));
            assert_eq!(URL_SAFE_NO_PAD.decode(parts[1]).unwrap(), br#"{"publicKey":"EA4A6C63E29C520ABEF5507B132EC5F9954776AEBEBE7B92421EEA691446D22C","aud":"mqtt.meshrank.net","iat":1800000000,"exp":1800003600}"#);
            assert_eq!(parts[2].len(), 128);
            let mut sig = [0; 64];
            for (i, byte) in sig.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&parts[2][i * 2..i * 2 + 2], 16).unwrap();
            }
            VerifyingKey::from_bytes(identity.public_key())
                .unwrap()
                .verify_strict(
                    token.rsplit_once('.').unwrap().0.as_bytes(),
                    &Signature::from_bytes(&sig),
                )
                .unwrap();
            let packet = connect_packet((username, token), "client", "status", "offline");
            // Skip MQTT Remaining Length, then inspect CONNECT flags and fields.
            let mut offset = 1;
            while packet[offset] & 0x80 != 0 {
                offset += 1;
            }
            offset += 1;
            assert_eq!(packet[offset + 7], 0xe6);
            offset += 10;
            for expected in ["client", "status", "offline", username, token] {
                let len = u16::from_be_bytes([packet[offset], packet[offset + 1]]) as usize;
                offset += 2;
                assert_eq!(&packet[offset..offset + len], expected.as_bytes());
                offset += len;
            }
            assert_eq!(offset, packet.len());
            assert_ne!(
                config
                    .credentials(identity.public_key(), 1_800_000_001, |m| identity.sign(m))
                    .unwrap(),
                credentials
            );
            if let Some(previous) = previous {
                assert_eq!(credentials, previous);
            }
            previous = Some(credentials);
        }
    }

    #[test]
    fn auth_defaults_clock_guard_and_audience_override() {
        let mut config = MqttConfig {
            username: "user".into(),
            password: "pass".into(),
            ..Default::default()
        };
        assert_eq!(config.value("auth").as_deref(), Some("password"));
        assert_eq!(
            config.credentials(&[0; 32], 0, |_| panic!("password auth must not sign")),
            Ok(("user".into(), "pass".into()))
        );
        assert_eq!(AuthMode::parse("DEVICE"), Some(AuthMode::Device));
        assert_eq!(AuthMode::parse("invalid"), None);
        config.auth = AuthMode::Device;
        config.host = "mqtt://broker.example:1883".into();
        for time in [0, 3600, u32::MAX] {
            assert!(
                config
                    .credentials(&[0; 32], time, |_| panic!("invalid clock must not sign"))
                    .is_err()
            );
        }
        config.auth_audience = "custom\"\\\n".into();
        let (_, token) = config
            .credentials(&[0; 32], 1_800_000_000, |_| [0; 64])
            .unwrap();
        let payload = String::from_utf8(
            URL_SAFE_NO_PAD
                .decode(token.split('.').nth(1).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert!(payload.contains(r#""aud":"custom\"\\\u000a""#));
        for host in [
            "broker.example",
            "mqtts://broker.example",
            "https://broker.example/path",
        ] {
            config.host = host.into();
            config.port = 1883;
            config.auth_audience.clear();
            let (_, token) = config
                .credentials(&[0; 32], 1_800_000_000, |_| [0; 64])
                .unwrap();
            let payload = String::from_utf8(
                URL_SAFE_NO_PAD
                    .decode(token.split('.').nth(1).unwrap())
                    .unwrap(),
            )
            .unwrap();
            assert!(payload.contains(r#""aud":"broker.example""#));
        }
    }

    struct Reader<'a> {
        bytes: &'a [u8],
        chunk_size: usize,
    }

    impl embedded_io_async::ErrorType for Reader<'_> {
        type Error = core::convert::Infallible;
    }

    impl embedded_io_async::Read for Reader<'_> {
        async fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
            // Simulate a scheduler boundary between TCP fragments.
            let mut pending = true;
            core::future::poll_fn(|cx| {
                if pending {
                    pending = false;
                    cx.waker().wake_by_ref();
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
            let len = out.len().min(self.bytes.len()).min(self.chunk_size);
            out[..len].copy_from_slice(&self.bytes[..len]);
            self.bytes = &self.bytes[len..];
            Ok(len)
        }
    }

    fn run<T>(future: impl Future<Output = T>) -> T {
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..10000 {
            if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                return result;
            }
        }
        panic!("response reader did not finish");
    }

    // The broker authorizes publications by the full observer key in both the
    // topic and JSON origin_id. MQTT client IDs are not observer identities.
    // https://github.com/michaelhart/meshcore-mqtt-broker#topics
    #[test]
    fn reports_use_the_full_uppercase_observer_key() {
        let public_key = [0xab; 32];
        let key = "AB".repeat(32);
        let config = MqttConfig {
            topic_root: "meshcore/{IATA}/{PUBLIC_KEY}/packets".into(),
            iata: "LHR".into(),
            ..Default::default()
        };
        assert_eq!(
            packets_topic(&config, &public_key),
            format!("meshcore/LHR/{key}/packets")
        );
        let legacy_template = MqttConfig {
            topic_root: "meshcore/<IATA>/<PUBLIC_KEY>/packets".into(),
            ..config.clone()
        };
        assert_eq!(
            packets_topic(&legacy_template, &public_key),
            packets_topic(&config, &public_key)
        );
        for (direction, label) in [(Direction::Rx, "rx"), (Direction::Tx, "tx")] {
            let event = PacketEvent::new(direction, &[0x12, 0xab, 0xff], -93, 16);
            assert_eq!(
                packet_json(&event, &public_key),
                format!(
                    "{{\"origin_id\":\"{key}\",\"type\":\"PACKET\",\"direction\":\"{label}\",\"raw\":\"12ABFF\",\"RSSI\":-93,\"SNR\":4.00}}"
                )
            );
        }
        assert_eq!(
            status_json(&public_key, true),
            format!("{{\"status\":\"online\",\"origin_id\":\"{key}\"}}")
        );
        assert_eq!(
            status_json(&public_key, false),
            format!("{{\"status\":\"offline\",\"origin_id\":\"{key}\"}}")
        );
    }

    #[test]
    fn mqtt_snr_is_decimal_db_not_the_internal_quarter_db_value() {
        for (quarters, db) in [(33, "8.25"), (-33, "-8.25"), (-1, "-0.25"), (0, "0.00")] {
            let event = PacketEvent::new(Direction::Rx, &[0], -93, quarters);
            let json = packet_json(&event, &[0; 32]);
            assert!(
                json.ends_with(&format!("\"RSSI\":-93,\"SNR\":{db}}}")),
                "{json}"
            );
        }
    }

    fn publish_contents(packet: &[u8]) -> (&str, &str) {
        assert_eq!(packet[0] & 0xfe, 0x30);
        let mut offset = 1;
        let mut length = 0usize;
        let mut multiplier = 1;
        loop {
            let byte = packet[offset];
            offset += 1;
            length += usize::from(byte & 0x7f) * multiplier;
            if byte & 0x80 == 0 {
                break;
            }
            multiplier *= 128;
            assert!(multiplier <= 128 * 128 * 128);
        }
        assert_eq!(packet.len() - offset, length);
        let topic_len = usize::from(u16::from_be_bytes([packet[offset], packet[offset + 1]]));
        offset += 2;
        let topic = core::str::from_utf8(&packet[offset..offset + topic_len]).unwrap();
        let json = core::str::from_utf8(&packet[offset + topic_len..]).unwrap();
        (topic, json)
    }

    #[test]
    fn packet_and_status_publish_wire_payloads_match_broker_identity() {
        let key = [0xcd; 32];
        let config = MqttConfig {
            topic_root: "meshcore/LHR/{PUBLIC_KEY}/packets".into(),
            ..Default::default()
        };
        let topic = packets_topic(&config, &key);
        // Exercise multi-byte Remaining Length and a packet larger than TCP's
        // 512-byte send buffer, using the same encoder as the firmware loop.
        let event = PacketEvent::new(Direction::Rx, &[0xab; 255], -90, 7);
        let json = packet_json(&event, &key);
        let publish = publish_packet(&topic, &json, false);
        assert!(publish.len() > 512);
        assert_eq!(publish_contents(&publish), (topic.as_str(), json.as_str()));
        let identity = format!("\"origin_id\":\"{}\"", "CD".repeat(32));
        assert!(json.contains(&identity));
        let status = status_topic(&topic);
        for online in [false, true] {
            let json = status_json(&key, online);
            let publish = publish_packet(&status, &json, true);
            assert_eq!(publish[0], 0x31);
            assert_eq!(publish_contents(&publish), (status.as_str(), json.as_str()));
            assert!(json.contains(&identity));
        }
        let offline = status_json(&key, false);
        let connect = connect_packet(
            (&config.username, &config.password),
            "mcrs-cdcdcd-1",
            &status,
            &offline,
        );
        assert!(
            connect
                .windows(offline.len())
                .any(|bytes| bytes == offline.as_bytes())
        );
        assert!(
            connect
                .windows(status.len())
                .any(|bytes| bytes == status.as_bytes())
        );
    }

    #[test]
    fn fragmented_and_coalesced_responses_preserve_packet_boundaries() {
        for chunk_size in [1, 2, 8] {
            let mut reader = Reader {
                bytes: &[0x20, 2, 0, 0, 0xd0, 0, 0xd0, 0],
                chunk_size,
            };
            assert_eq!(run(read_connack(&mut reader)), Ok(()));
            assert_eq!(run(read_pingresp(&mut reader)), Ok(()));
            assert_eq!(run(read_pingresp(&mut reader)), Ok(()));
            assert!(reader.bytes.is_empty());
        }
    }

    #[test]
    fn drains_more_than_a_receive_buffer_of_ping_responses() {
        let bytes = [0xd0, 0].repeat(1024);
        let mut reader = Reader {
            bytes: &bytes,
            chunk_size: 512,
        };
        for _ in 0..1024 {
            assert_eq!(run(read_pingresp(&mut reader)), Ok(()));
        }
        assert!(reader.bytes.is_empty());
    }

    #[test]
    fn connack_distinguishes_auth_server_protocol_and_transport_failures() {
        for (code, expected) in [
            (0, Ok(())),
            (1, Err(ConnectionError::Protocol)),
            (2, Err(ConnectionError::Protocol)),
            (3, Err(ConnectionError::Server)),
            (4, Err(ConnectionError::Auth)),
            (5, Err(ConnectionError::Auth)),
            (6, Err(ConnectionError::Protocol)),
        ] {
            assert_eq!(
                run(read_connack(&mut Reader {
                    bytes: &[0x20, 2, 0, code],
                    chunk_size: 1,
                })),
                expected
            );
        }
        assert_eq!(
            run(read_connack(&mut Reader {
                bytes: &[0x20, 2],
                chunk_size: 1,
            })),
            Err(ConnectionError::Connection)
        );
    }

    #[test]
    fn rejects_failed_or_invalid_connack() {
        for bytes in [
            &[0x20, 2, 0, 5][..],
            &[0x20, 2, 1, 0],
            &[0x21, 2, 0, 0],
            &[0x20, 3, 0, 0],
            &[0x20, 2, 0],
            &[],
        ] {
            assert!(
                run(read_connack(&mut Reader {
                    bytes,
                    chunk_size: 1
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn rejects_malformed_unexpected_or_truncated_responses() {
        for bytes in [&[0xd1, 0][..], &[0xd0, 1], &[0x20, 2], &[0xd0], &[]] {
            assert_eq!(
                run(read_pingresp(&mut Reader {
                    bytes,
                    chunk_size: 1
                })),
                Err(())
            );
        }
    }
}
