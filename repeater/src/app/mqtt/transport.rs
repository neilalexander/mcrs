//! MQTT endpoints and bounded RFC 6455 binary-stream framing.
use alloc::{format, vec, vec::Vec};
use base64::{Engine, engine::general_purpose::STANDARD};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};
use embedded_io_async::{Read, Write};
use sha1::{Digest, Sha1};

#[derive(Debug, PartialEq, Eq)]
pub struct Endpoint<'a> {
    pub host: &'a str,
    pub port: u16,
    pub path: &'a str,
    pub websocket: bool,
    pub tls: bool,
}

impl<'a> Endpoint<'a> {
    pub fn parse(value: &'a str, legacy_port: u16) -> Result<Self, ()> {
        if value.is_empty()
            || !value.is_ascii()
            || value.bytes().any(|b| b <= 32 || b == 127)
            || value.contains(['#', '@', '\\'])
        {
            return Err(());
        }
        let (rest, websocket, tls, default_port) =
            if let Some((scheme, rest)) = value.split_once("://") {
                let (ws, tls, port) = match scheme {
                    "mqtt" => (false, false, 1883),
                    "mqtts" => (false, true, 8883),
                    "ws" | "http" => (true, false, 80),
                    "wss" | "https" => (true, true, 443),
                    _ => return Err(()),
                };
                (rest, ws, tls, port)
            } else {
                (value, false, false, legacy_port)
            };
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let authority = &rest[..end];
        let path = &rest[end..];
        if (!websocket && !path.is_empty()) || path.starts_with('?') {
            return Err(());
        }
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().map_err(|_| ())?),
            None => (authority, default_port),
        };
        // The network stack currently supports IPv4 only.
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
            || port == 0
        {
            return Err(());
        }
        Ok(Self {
            host,
            port,
            path: if path.is_empty() { "/mqtt" } else { path },
            websocket,
            tls,
        })
    }
}

pub async fn upgrade<S: Read + Write>(
    io: &mut S,
    endpoint: &Endpoint<'_>,
    nonce: [u8; 16],
) -> Result<(), ()> {
    let key = STANDARD.encode(nonce);
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: mqtt\r\n\r\n",
        endpoint.path, endpoint.host, endpoint.port, key
    );
    io.write_all(request.as_bytes()).await.map_err(|_| ())?;
    io.flush().await.map_err(|_| ())?;
    // Read exactly through the header terminator, retaining any coalesced WS data.
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        if header.len() == 4096 {
            return Err(());
        }
        let mut byte = [0];
        io.read_exact(&mut byte).await.map_err(|_| ())?;
        header.push(byte[0]);
    }
    validate_upgrade(&header, &key)
}

fn validate_upgrade(header: &[u8], key: &str) -> Result<(), ()> {
    let text = core::str::from_utf8(header).map_err(|_| ())?;
    let mut lines = text.split("\r\n");
    let status = lines.next().ok_or(())?;
    if !status.starts_with("HTTP/1.1 101 ") {
        return Err(());
    }
    let mut hash = Sha1::new();
    hash.update(key.as_bytes());
    hash.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let expected = STANDARD.encode(hash.finalize());
    let (mut upgrade, mut connection, mut accept, mut protocol) = (false, false, false, false);
    for line in lines.filter(|l| !l.is_empty()) {
        let (name, value) = line.split_once(':').ok_or(())?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("upgrade") {
            upgrade = value.eq_ignore_ascii_case("websocket");
        }
        if name.eq_ignore_ascii_case("connection") {
            connection |= value
                .split(',')
                .any(|v| v.trim().eq_ignore_ascii_case("upgrade"));
        }
        if name.eq_ignore_ascii_case("sec-websocket-accept") {
            if accept || value != expected {
                return Err(());
            }
            accept = true;
        }
        if name.eq_ignore_ascii_case("sec-websocket-protocol") {
            if protocol || value != "mqtt" {
                return Err(());
            }
            protocol = true;
        }
        if name.eq_ignore_ascii_case("sec-websocket-extensions") {
            return Err(());
        }
    }
    (upgrade && connection && accept && protocol)
        .then_some(())
        .ok_or(())
}

pub type Pong = Signal<NoopRawMutex, Vec<u8>>;

pub struct Reader<'a, R> {
    inner: R,
    websocket: bool,
    remaining: u64,
    fragmented: bool,
    pong: &'a Pong,
}

impl<'a, R> Reader<'a, R> {
    pub fn new(inner: R, websocket: bool, pong: &'a Pong) -> Self {
        Self {
            inner,
            websocket,
            remaining: 0,
            fragmented: false,
            pong,
        }
    }
}

impl<R: Read> embedded_io_async::ErrorType for Reader<'_, R> {
    type Error = embedded_io_async::ErrorKind;
}

impl<R: Read> Read for Reader<'_, R> {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
        use embedded_io_async::ErrorKind::InvalidData;
        if out.is_empty() {
            return Ok(0);
        }
        if !self.websocket {
            return self.inner.read(out).await.map_err(|_| InvalidData);
        }
        while self.remaining == 0 {
            let mut head = [0; 2];
            self.inner
                .read_exact(&mut head)
                .await
                .map_err(|_| InvalidData)?;
            let fin = head[0] & 0x80 != 0;
            let opcode = head[0] & 15;
            if head[0] & 0x70 != 0 || head[1] & 0x80 != 0 {
                return Err(InvalidData);
            }
            let mut len = u64::from(head[1]);
            if len == 126 {
                let mut bytes = [0; 2];
                self.inner
                    .read_exact(&mut bytes)
                    .await
                    .map_err(|_| InvalidData)?;
                len = u64::from(u16::from_be_bytes(bytes));
                if len < 126 {
                    return Err(InvalidData);
                }
            } else if len == 127 {
                let mut bytes = [0; 8];
                self.inner
                    .read_exact(&mut bytes)
                    .await
                    .map_err(|_| InvalidData)?;
                len = u64::from_be_bytes(bytes);
                if len < 65536 || len >> 63 != 0 {
                    return Err(InvalidData);
                }
            }
            match opcode {
                0 | 2 => {
                    if (opcode == 0) != self.fragmented {
                        return Err(InvalidData);
                    }
                    self.fragmented = !fin;
                    self.remaining = len;
                }
                8 => return Err(InvalidData), // End the session; reconnect with a new clean session.
                9 | 10 => {
                    if !fin || len > 125 {
                        return Err(InvalidData);
                    }
                    let mut data = vec![0; len as usize];
                    self.inner
                        .read_exact(&mut data)
                        .await
                        .map_err(|_| InvalidData)?;
                    if opcode == 9 {
                        self.pong.signal(data);
                    }
                }
                _ => return Err(InvalidData),
            }
        }
        let len = (out.len() as u64).min(self.remaining) as usize;
        let n = self
            .inner
            .read(&mut out[..len])
            .await
            .map_err(|_| InvalidData)?;
        if n == 0 {
            return Err(InvalidData);
        }
        self.remaining -= n as u64;
        Ok(n)
    }
}

pub async fn write<W: Write>(
    writer: &mut W,
    websocket: bool,
    opcode: u8,
    data: &[u8],
    mask: [u8; 4],
) -> Result<(), ()> {
    if websocket {
        let mut frame = Vec::with_capacity(data.len() + 14);
        frame.push(0x80 | opcode);
        match data.len() {
            0..=125 => frame.push(0x80 | data.len() as u8),
            126..=65535 => {
                frame.push(0xfe);
                frame.extend_from_slice(&(data.len() as u16).to_be_bytes());
            }
            _ => {
                frame.push(0xff);
                frame.extend_from_slice(&(data.len() as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(&mask);
        frame.extend(data.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        writer.write_all(&frame).await.map_err(|_| ())?;
    } else {
        writer.write_all(data).await.map_err(|_| ())?;
    }
    writer.flush().await.map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    fn run<T>(future: impl Future<Output = T>) -> T {
        let mut future = pin!(future);
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..100_000 {
            if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                return result;
            }
        }
        panic!("operation did not finish");
    }
    struct Io {
        input: Vec<u8>,
        offset: usize,
        output: Vec<u8>,
        chunk: usize,
    }
    impl Io {
        fn new(input: &[u8], chunk: usize) -> Self {
            Self {
                input: input.into(),
                offset: 0,
                output: Vec::new(),
                chunk,
            }
        }
    }
    impl embedded_io_async::ErrorType for Io {
        type Error = core::convert::Infallible;
    }
    impl Read for Io {
        async fn read(&mut self, out: &mut [u8]) -> Result<usize, Self::Error> {
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
            let n = out
                .len()
                .min(self.input.len() - self.offset)
                .min(self.chunk);
            out[..n].copy_from_slice(&self.input[self.offset..self.offset + n]);
            self.offset += n;
            Ok(n)
        }
    }
    impl Write for Io {
        async fn write(&mut self, data: &[u8]) -> Result<usize, Self::Error> {
            let n = data.len().min(self.chunk);
            self.output.extend_from_slice(&data[..n]);
            Ok(n)
        }
        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn endpoint_transports_ports_and_paths() {
        for (url, port, tls, websocket, path) in [
            ("broker.example", 2883, false, false, "/mqtt"),
            ("mqtt://broker.example", 1883, false, false, "/mqtt"),
            ("mqtts://broker.example", 8883, true, false, "/mqtt"),
            ("ws://broker.example", 80, false, true, "/mqtt"),
            ("http://broker.example", 80, false, true, "/mqtt"),
            ("wss://broker.example", 443, true, true, "/mqtt"),
            (
                "https://broker.example:8443/custom?token=abc",
                8443,
                true,
                true,
                "/custom?token=abc",
            ),
        ] {
            let endpoint = Endpoint::parse(url, 2883).unwrap();
            assert_eq!(
                endpoint,
                Endpoint {
                    host: "broker.example",
                    port,
                    tls,
                    websocket,
                    path
                }
            );
        }
        for url in [
            "",
            "wss://",
            "ftp://broker",
            "wss://user@broker",
            "ws://broker:0/",
            "ws://broker:65536/",
            "ws://broker/a#b",
            "ws://broker/\r\nX:yes",
            "ws://[::1]/",
            "mqtt://broker/path",
            "ws://broker?query",
            "broker bad",
        ] {
            assert!(Endpoint::parse(url, 1883).is_err(), "{url}");
        }
    }

    const RESPONSE: &str = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: WebSocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\nSec-WebSocket-Protocol: mqtt\r\n\r\n";
    #[test]
    fn upgrade_validates_accept_and_preserves_coalesced_data() {
        for chunk in [1, 7, 512] {
            let mut input = RESPONSE.as_bytes().to_vec();
            input.extend_from_slice(&[0x82, 4, 0x20, 2, 0, 0]);
            let mut io = Io::new(&input, chunk);
            let endpoint = Endpoint::parse("wss://broker.example:8443/path?q=1", 1883).unwrap();
            run(upgrade(&mut io, &endpoint, *b"the sample nonce")).unwrap();
            assert_eq!(io.offset, RESPONSE.len());
            let request = core::str::from_utf8(&io.output).unwrap();
            assert!(request.starts_with("GET /path?q=1 HTTP/1.1\r\nHost: broker.example:8443\r\n"));
            assert!(request.contains("Sec-WebSocket-Protocol: mqtt\r\n"));
        }
        for response in [
            RESPONSE.replace("101 Switching", "200 Switching"),
            RESPONSE.replace("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=", "wrong"),
            RESPONSE.replace("Protocol: mqtt", "Protocol: other"),
            RESPONSE.replace("Upgrade: WebSocket", "Upgrade: other"),
            RESPONSE.replace("keep-alive, Upgrade", "keep-alive"),
            RESPONSE.replace(
                "\r\n\r\n",
                "\r\nSec-WebSocket-Extensions: permessage-deflate\r\n\r\n",
            ),
        ] {
            assert!(validate_upgrade(response.as_bytes(), "dGhlIHNhbXBsZSBub25jZQ==").is_err());
        }
        let mut io = Io::new(&vec![b'x'; 4097], 1);
        assert!(
            run(upgrade(
                &mut io,
                &Endpoint::parse("ws://broker", 1883).unwrap(),
                [0; 16]
            ))
            .is_err()
        );
    }

    #[test]
    fn fragmented_binary_stream_and_interleaved_control_frames() {
        for chunk in [1, 2, 512] {
            let bytes = [
                0x02, 1, 0x20, 0x89, 2, b'h', b'i', 0x00, 1, 2, 0x8a, 0, 0x80, 2, 0, 0, 0x82, 4,
                0xd0, 0, 0xd0, 0,
            ];
            let pong = Pong::new();
            let mut reader = Reader::new(Io::new(&bytes, chunk), true, &pong);
            let mut connack = [0; 4];
            run(reader.read_exact(&mut connack)).unwrap();
            assert_eq!(connack, [0x20, 2, 0, 0]);
            assert_eq!(run(pong.wait()), b"hi");
            for _ in 0..2 {
                let mut response = [0; 2];
                run(reader.read_exact(&mut response)).unwrap();
                assert_eq!(response, [0xd0, 0]);
            }
            assert_eq!(reader.inner.offset, bytes.len());
        }
    }

    #[test]
    fn rejects_invalid_frames_and_eof() {
        for bytes in [
            &[0x81, 1, b'x'][..],
            &[0xc2, 0],
            &[0x82, 0x80],
            &[0x80, 0],
            &[0x09, 0],
            &[0x89, 126, 0, 126],
            &[0x82, 126, 0, 1, 0],
            &[0x82, 127, 0x80, 0, 0, 0, 0, 1, 0, 0],
            &[0x88, 0],
            &[0x82, 1],
            &[0x02, 0, 0x82, 1, 0],
            &[],
        ] {
            let pong = Pong::new();
            let mut reader = Reader::new(Io::new(bytes, 1), true, &pong);
            assert!(run(reader.read(&mut [0; 1])).is_err(), "{bytes:?}");
        }
    }

    #[test]
    fn masks_all_client_frames_and_supports_extended_lengths() {
        for len in [0, 2, 125, 126, 1024, 65536] {
            let data = vec![0x55; len];
            let mask = [1, 2, 3, 4];
            let mut io = Io::new(&[], 7);
            run(write(&mut io, true, 2, &data, mask)).unwrap();
            assert_eq!(io.output[0], 0x82);
            let offset = match len {
                0..=125 => {
                    assert_eq!(io.output[1], 0x80 | len as u8);
                    2
                }
                126..=65535 => {
                    assert_eq!(&io.output[1..4], &[0xfe, (len >> 8) as u8, len as u8]);
                    4
                }
                _ => {
                    assert_eq!(io.output[1], 0xff);
                    assert_eq!(&io.output[2..10], &(len as u64).to_be_bytes());
                    10
                }
            };
            assert_eq!(&io.output[offset..offset + 4], &mask);
            for (i, b) in io.output[offset + 4..].iter().enumerate() {
                assert_eq!(b ^ mask[i % 4], 0x55);
            }
            // Server binary frames are unmasked; read extended lengths incrementally.
            let mut server_frame = io.output[..offset].to_vec();
            server_frame[1] &= 0x7f;
            server_frame.extend_from_slice(&data);
            let pong = Pong::new();
            let mut reader = Reader::new(Io::new(&server_frame, 128), true, &pong);
            let mut actual = vec![0; len];
            run(reader.read_exact(&mut actual)).unwrap();
            assert_eq!(actual, data);
        }
        let mut io = Io::new(&[], 1);
        run(write(&mut io, false, 2, &[0xc0, 0], [0; 4])).unwrap();
        assert_eq!(io.output, [0xc0, 0]);
        run(write(&mut io, true, 10, b"ok", [1, 2, 3, 4])).unwrap();
        assert_eq!(
            &io.output[2..],
            &[0x8a, 0x82, 1, 2, 3, 4, b'o' ^ 1, b'k' ^ 2]
        );
    }
}
