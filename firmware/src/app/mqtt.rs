//! Minimal MQTT 3.1.1 packet reporter, modelled on MeshCore-MQTT.

pub use mcrs_firmware::mqtt_transport as transport;

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
