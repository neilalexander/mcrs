//! Minimal MQTT 3.1.1 packet reporter, modelled on MeshCore-MQTT.

pub use crate::mqtt_transport as transport;

use alloc::{
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::fmt::Write as _;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MqttConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
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
            "topic.root" => self.topic_root.clone(),
            "iata" => self.iata.clone(),
            _ => return None,
        })
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
        event.snr
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
    config: &MqttConfig,
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
        let _ = write!(out, "{b:02X}");
    }
    out
}

// Fixed-size responses are sufficient for this clean-session, QoS 0 publisher.
// read_exact handles TCP fragmentation without consuming the next MQTT packet.
pub async fn read_connack(reader: &mut impl embedded_io_async::Read) -> Result<(), ()> {
    let mut packet = [0; 4];
    reader.read_exact(&mut packet).await.map_err(|_| ())?;
    (packet == [0x20, 0x02, 0x00, 0x00]).then_some(()).ok_or(())
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
            let event = PacketEvent::new(direction, &[0x12, 0xab, 0xff], -93, 4);
            assert_eq!(
                packet_json(&event, &public_key),
                format!(
                    "{{\"origin_id\":\"{key}\",\"type\":\"PACKET\",\"direction\":\"{label}\",\"raw\":\"12ABFF\",\"RSSI\":-93,\"SNR\":4}}"
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
        let connect = connect_packet(&config, "mcrs-cdcdcd-1", &status, &offline);
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
    fn rejects_failed_or_invalid_connack() {
        for bytes in [
            &[0x20, 2, 0, 5][..],
            &[0x20, 2, 1, 0],
            &[0x21, 2, 0, 0],
            &[0x20, 3, 0, 0],
            &[0x20, 2, 0],
            &[],
        ] {
            assert_eq!(
                run(read_connack(&mut Reader {
                    bytes,
                    chunk_size: 1
                })),
                Err(())
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
