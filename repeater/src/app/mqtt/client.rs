//! MQTT connections over an Embassy network stack, independent of board hardware.
use crate::{
    VERSION,
    app::AppContext,
    platform::{Platform, Storage},
};
use alloc::{format, string::String};
use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};
use defmt::{info, warn};
use rand_core::{CryptoRng, RngCore};

const MQTT_STATUS_INTERVAL_SECS: u64 = 15 * 60;

fn random_bytes<const N: usize>(rng: &mut impl RngCore) -> [u8; N] {
    let mut bytes = [0; N];
    rng.fill_bytes(&mut bytes);
    bytes
}

use core::cell::RefCell;
use embassy_net::tcp::{Error, TcpSocket};
use embedded_io_async::{ErrorType, Read, Write};

// Each poll borrows the socket only for that poll. Embassy TCP read/write have
// no per-future state and are cancellation safe, allowing TLS's split halves to
// drive independent reads and writes without holding a RefCell borrow over await.
#[derive(Clone, Copy)]
struct SharedSocket<'a, 's>(pub &'a RefCell<TcpSocket<'s>>);
impl ErrorType for SharedSocket<'_, '_> {
    type Error = Error;
}
impl Read for SharedSocket<'_, '_> {
    async fn read(&mut self, out: &mut [u8]) -> Result<usize, Error> {
        poll_fn(|cx| {
            let mut socket = self.0.borrow_mut();
            pin!(socket.read(out)).poll(cx)
        })
        .await
    }
}
impl Write for SharedSocket<'_, '_> {
    async fn write(&mut self, data: &[u8]) -> Result<usize, Error> {
        poll_fn(|cx| {
            let mut socket = self.0.borrow_mut();
            pin!(socket.write(data)).poll(cx)
        })
        .await
    }
    async fn flush(&mut self) -> Result<(), Error> {
        poll_fn(|cx| {
            let mut socket = self.0.borrow_mut();
            pin!(socket.flush()).poll(cx)
        })
        .await
    }
}

/// Run one configured broker, obtaining cryptographic entropy from the platform.
/// An empty broker host disables the connection.
pub async fn run<P: Platform, S: Storage, G: RngCore + CryptoRng>(
    stack: embassy_net::Stack<'_>,
    context: &AppContext<P, S>,
    index: usize,
    make_rng: impl Fn() -> G + Copy,
) -> ! {
    use crate::app::mqtt::ConnectionState;

    loop {
        let generation = context.mqtt.generation();
        let mqtt = context
            .with_config(|config| config.mqtt(index).cloned())
            .await;
        let Some(mqtt) = mqtt.filter(|mqtt| !mqtt.host.is_empty()) else {
            context.mqtt.set_state(index, ConnectionState::Disabled);
            wait_mqtt_retry_or_restart(context, index, generation, 30).await;
            continue;
        };
        context.mqtt.set_state(index, ConnectionState::Connecting);
        // Keep the three TLS-capable session futures out of the fixed 32 KiB
        // Embassy task arena. Disabled brokers allocate no session state.
        let result = alloc::boxed::Box::pin(mqtt_session(
            stack, context, index, generation, &mqtt, make_rng,
        ))
        .await;
        // Cancellation by an explicit restart is not a connection failure.
        if context.mqtt.generation() == generation
            && let Err(error) = result {
                context.mqtt.set_error(index, Some(error));
                warn!("MQTT {}: {}", index + 1, error.as_str());
            }
        context.mqtt.set_state(index, ConnectionState::Disconnected);
        wait_mqtt_retry_or_restart(context, index, generation, 10).await;
    }
}

async fn mqtt_session<P: Platform, S: Storage, G: RngCore + CryptoRng>(
    stack: embassy_net::Stack<'_>,
    context: &AppContext<P, S>,
    index: usize,
    generation: u32,
    config: &crate::app::config::MqttConfig,
    make_rng: impl Fn() -> G + Copy,
) -> Result<(), crate::app::mqtt::ConnectionError> {
    use crate::app::mqtt::ConnectionError;
    use crate::app::mqtt::transport::{self, Endpoint};
    use embedded_tls::{Aes128GcmSha256, TlsConfig, TlsConnection, TlsContext, UnsecureProvider};

    let endpoint =
        Endpoint::parse(&config.host, config.port).map_err(|_| ConnectionError::Config)?;
    let mut rx = [0u8; 512];
    let mut tx = [0u8; 512];
    let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx, &mut tx);
    mqtt_io(context, index, generation, async {
        let addresses = stack
            .dns_query(endpoint.host, embassy_net::dns::DnsQueryType::A)
            .await
            .map_err(|_| ())?;
        socket
            .connect((addresses.first().copied().ok_or(())?, endpoint.port))
            .await
            .map_err(|_| ())
    })
    .await
    .map_err(|_| ConnectionError::Unreachable)?;

    if endpoint.tls {
        // Allocate TLS records only for secure connections, outside the task arena.
        // A full receive record accommodates servers that don't negotiate MFL.
        let mut read_record = alloc::vec::Vec::new();
        let mut write_record = alloc::vec::Vec::new();
        read_record
            .try_reserve_exact(16640)
            .map_err(|_| ConnectionError::Memory)?;
        write_record
            .try_reserve_exact(2048)
            .map_err(|_| ConnectionError::Memory)?;
        read_record.resize(16640, 0);
        write_record.resize(2048, 0);
        let shared = core::cell::RefCell::new(socket);
        let mut tls = TlsConnection::<_, Aes128GcmSha256>::new(
            SharedSocket(&shared),
            &mut read_record,
            &mut write_record,
        );
        let tls_config = TlsConfig::new()
            .enable_rsa_signatures()
            .with_server_name(endpoint.host);
        mqtt_io(context, index, generation, async {
            // Deliberately accept all certificates, including automatically rotated
            // certificates. This encrypts traffic but does not authenticate peers.
            tls.open(TlsContext::new(
                &tls_config,
                UnsecureProvider::new::<Aes128GcmSha256>(make_rng()),
            ))
            .await
            .map_err(|_| ())
        })
        .await
        .map_err(|_| ConnectionError::Tls)?;
        if endpoint.websocket {
            mqtt_io(
                context,
                index,
                generation,
                transport::upgrade(&mut tls, &endpoint, random_bytes(&mut make_rng())),
            )
            .await
            .map_err(|_| ConnectionError::WebSocket)?;
        }
        let (reader, writer) = tls.split();
        mqtt_connected(
            context,
            index,
            generation,
            config,
            endpoint.websocket,
            reader,
            writer,
            make_rng,
        )
        .await
    } else {
        if endpoint.websocket {
            mqtt_io(
                context,
                index,
                generation,
                transport::upgrade(&mut socket, &endpoint, random_bytes(&mut make_rng())),
            )
            .await
            .map_err(|_| ConnectionError::WebSocket)?;
        }
        let (reader, writer) = socket.split();
        mqtt_connected(
            context,
            index,
            generation,
            config,
            endpoint.websocket,
            reader,
            writer,
            make_rng,
        )
        .await
    }
}

async fn mqtt_status_json<P: Platform, S: Storage>(
    context: &AppContext<P, S>,
    public_key: &[u8; 32],
    online: bool,
) -> String {
    context
        .with_config(|config| {
            let status = context.status();
            crate::app::mqtt::status_json(
                public_key,
                online,
                config.node_name(),
                context.platform.model(),
                VERSION,
                context.platform.now_seconds(),
                crate::app::mqtt::StatusStats {
                    battery_mv: status.battery_millivolts,
                    uptime_ms: status.uptime_millis,
                },
            )
        })
        .await
}

async fn mqtt_connected<
    P: Platform,
    S: Storage,
    R: embedded_io_async::Read,
    W: embedded_io_async::Write,
    G: RngCore + CryptoRng,
>(
    context: &AppContext<P, S>,
    index: usize,
    generation: u32,
    config: &crate::app::config::MqttConfig,
    websocket: bool,
    reader: R,
    mut writer: W,
    make_rng: impl Fn() -> G + Copy,
) -> Result<(), crate::app::mqtt::ConnectionError> {
    use crate::app::mqtt::{self, ConnectionError, transport};
    use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};

    let (public_key, credentials) = context
        .with_identity(|identity| {
            let public_key = *identity.public_key();
            let credentials =
                config.credentials(&public_key, context.platform.now_seconds(), |message| {
                    identity.sign(message)
                });
            (public_key, credentials)
        })
        .await;
    let (username, password) = credentials.map_err(|_| {
        if context.platform.now_seconds() < 1_704_067_200 {
            ConnectionError::Clock
        } else {
            ConnectionError::Config
        }
    })?;
    let topic = mqtt::packets_topic(config, &public_key);
    let status = mqtt::status_topic(&topic);
    let client_id = format!(
        "mcrs-{:02x}{:02x}{:02x}-{}",
        public_key[0],
        public_key[1],
        public_key[2],
        index + 1
    );
    let pong = transport::Pong::new();
    let ack = Signal::<NoopRawMutex, ()>::new();
    let mut reader = transport::Reader::new(reader, websocket, &pong);
    let mut receive = pin!(async {
        mqtt::read_connack(&mut reader).await?;
        ack.signal(());
        while mqtt::read_pingresp(&mut reader).await.is_ok() {
            ack.signal(());
        }
        Err::<(), _>(ConnectionError::Connection)
    });
    let mut send = pin!(async {
        let mut rng = make_rng();
        mqtt_io(context, index, generation, async {
            // The broker retains these bytes until the connection is lost.
            let offline = mqtt_status_json(context, &public_key, false).await;
            transport::write(
                &mut writer,
                websocket,
                2,
                &mqtt::connect_packet((&username, &password), &client_id, &status, &offline),
                random_bytes(&mut rng),
            )
            .await?;
            mqtt_wait_ack(&mut writer, websocket, &ack, &pong, &mut rng).await
        })
        .await?;
        context
            .mqtt
            .set_state(index, mqtt::ConnectionState::Connected);
        info!("MQTT {}: connected", index + 1);
        let mut next_status = embassy_time::Instant::now(); // Publish immediately after connecting.
        let mut keepalive = next_status + embassy_time::Duration::from_secs(30);
        loop {
            let mut event = pin!(context.mqtt.receive_packet(index));
            let mut idle = pin!(embassy_time::Timer::at(keepalive.min(next_status)));
            let mut control = pin!(pong.wait());
            let (packet, control) = poll_fn(|cx| {
                context.mqtt.register_waker(index, cx.waker());
                if context.mqtt.generation() != generation {
                    return Poll::Ready((None, None));
                }
                // Deadlines take priority so busy connections still refresh status.
                if idle.as_mut().poll(cx).is_ready() {
                    return Poll::Ready((None, None));
                }
                if let Poll::Ready(data) = control.as_mut().poll(cx) {
                    return Poll::Ready((None, Some(data)));
                }
                if let Poll::Ready(event) = event.as_mut().poll(cx) {
                    return Poll::Ready((Some(event), None));
                }
                Poll::Pending
            })
            .await;
            mqtt_io(context, index, generation, async {
                if let Some(data) = control {
                    // WebSocket control frames do not count as MQTT keepalive traffic.
                    return transport::write(
                        &mut writer,
                        websocket,
                        10,
                        &data,
                        random_bytes(&mut rng),
                    )
                    .await;
                }
                if let Some(event) = packet {
                    let payload = context
                        .with_config(|config| {
                            mqtt::packet_json(&event, &public_key, config.node_name())
                        })
                        .await;
                    transport::write(
                        &mut writer,
                        websocket,
                        2,
                        &mqtt::publish_packet(&topic, &payload, false),
                        random_bytes(&mut rng),
                    )
                    .await?;
                } else if embassy_time::Instant::now() >= next_status {
                    let online = mqtt_status_json(context, &public_key, true).await;
                    transport::write(
                        &mut writer,
                        websocket,
                        2,
                        &mqtt::publish_packet(&status, &online, true),
                        random_bytes(&mut rng),
                    )
                    .await?;
                    next_status = embassy_time::Instant::now()
                        + embassy_time::Duration::from_secs(MQTT_STATUS_INTERVAL_SECS);
                } else {
                    ack.reset();
                    transport::write(
                        &mut writer,
                        websocket,
                        2,
                        &[0xc0, 0],
                        random_bytes(&mut rng),
                    )
                    .await?;
                    mqtt_wait_ack(&mut writer, websocket, &ack, &pong, &mut rng).await?;
                }
                keepalive = embassy_time::Instant::now() + embassy_time::Duration::from_secs(30);
                Ok::<(), ()>(())
            })
            .await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), ()>(())
    });
    poll_fn(|cx| {
        if let Poll::Ready(result) = receive.as_mut().poll(cx) {
            return Poll::Ready(result);
        }
        if send.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(ConnectionError::Connection));
        }
        Poll::Pending
    })
    .await
}

// Continue servicing WebSocket pings even while waiting for MQTT acknowledgements.
async fn mqtt_wait_ack<W: embedded_io_async::Write, G: RngCore>(
    writer: &mut W,
    websocket: bool,
    ack: &embassy_sync::signal::Signal<embassy_sync::blocking_mutex::raw::NoopRawMutex, ()>,
    pong: &crate::app::mqtt::transport::Pong,
    mut rng: &mut G,
) -> Result<(), ()> {
    loop {
        let mut response = pin!(ack.wait());
        let mut control = pin!(pong.wait());
        let data = poll_fn(|cx| {
            if response.as_mut().poll(cx).is_ready() {
                return Poll::Ready(None);
            }
            if let Poll::Ready(data) = control.as_mut().poll(cx) {
                return Poll::Ready(Some(data));
            }
            Poll::Pending
        })
        .await;
        let Some(data) = data else {
            return Ok(());
        };
        crate::app::mqtt::transport::write(writer, websocket, 10, &data, random_bytes(&mut rng))
            .await?;
    }
}

// Dropping a cancelled or timed-out operation may leave a partial MQTT frame.
// Every error from this helper must therefore discard the connection.
async fn mqtt_io<P: Platform, S: Storage, T, E>(
    context: &AppContext<P, S>,
    index: usize,
    generation: u32,
    operation: impl Future<Output = Result<T, E>>,
) -> Result<T, ()> {
    let mut operation = pin!(operation);
    let mut timeout = pin!(embassy_time::Timer::after_secs(10));
    poll_fn(|cx| {
        context.mqtt.register_waker(index, cx.waker());
        if context.mqtt.generation() != generation {
            return Poll::Ready(Err(()));
        }
        if timeout.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(()));
        }
        operation
            .as_mut()
            .poll(cx)
            .map(|result| result.map_err(|_| ()))
    })
    .await
}

async fn wait_mqtt_retry_or_restart<P: Platform, S: Storage>(
    context: &AppContext<P, S>,
    index: usize,
    generation: u32,
    seconds: u64,
) {
    let mut timer = pin!(embassy_time::Timer::after_secs(seconds));
    poll_fn(|cx| {
        if context.mqtt.generation() != generation {
            return Poll::Ready(());
        }
        context.mqtt.register_waker(index, cx.waker());
        if timer.as_mut().poll(cx).is_ready() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await
}

/// Update broker state when the platform network connection is lost.
pub async fn disconnected<P: Platform, S: Storage>(context: &AppContext<P, S>) {
    for index in 0..crate::app::config::MQTT_SERVER_COUNT {
        use crate::app::mqtt::{ConnectionError, ConnectionState};
        let enabled = context
            .with_config(|config| config.mqtt(index).is_some_and(|mqtt| !mqtt.host.is_empty()))
            .await;
        if enabled {
            context
                .mqtt
                .set_error(index, Some(ConnectionError::Unreachable));
            context.mqtt.set_state(index, ConnectionState::Disconnected);
        } else {
            context.mqtt.set_state(index, ConnectionState::Disabled);
        }
    }
}
