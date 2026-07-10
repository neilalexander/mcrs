pub mod cli;
pub mod config;
mod crypto;
pub(crate) mod discovery;
pub mod identity;

mod neighbours;
pub mod ota;
pub mod periodic;
mod protocol_log;
pub mod regions;
mod remote;

use alloc::{collections::VecDeque, string::String, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    fmt,
    future::{Future, poll_fn},
    pin::pin,
    sync::atomic::{AtomicI16, AtomicU8, AtomicU16, AtomicU32, Ordering},
    task::{Poll, Waker},
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, mutex::Mutex};
use embedded_hal_async::delay::DelayNs;
use mcrs_protocol::{Packet, PayloadKind, RoutePath, RouteType, SeenPacketCache};

const RECEIVE_BUFFER_LEN: usize = 255;
const FORWARD_BUFFER_LEN: usize = 255;
const INBOUND_QUEUE_CAPACITY: usize = 8;
const OUTBOUND_QUEUE_CAPACITY: usize = 16;
const SEEN_PACKET_TTL_MS: u64 = 15_000;
const FORWARD_DELAY_BASE_MS: u32 = 40;
const FORWARD_DELAY_JITTER_MS: u32 = 120;
const CAD_MAX_ATTEMPTS: usize = 6;
const CAD_BUSY_BACKOFF_BASE_MS: u32 = 10;
const CAD_BUSY_BACKOFF_AIRTIME_DIVISOR: u32 = 2;
const CAD_BUSY_BACKOFF_SYMBOLS: u32 = 4;
const BATTERY_LEVEL_UNKNOWN: u8 = u8::MAX;
const BATTERY_MILLIVOLTS_UNKNOWN: u16 = u16::MAX;

type AppMutex<T> = Mutex<CriticalSectionRawMutex, T>;

pub struct AppContext<S>
where
    S: crate::platform::storage::Storage,
{
    config: AppMutex<config::AppConfig>,
    storage: AppMutex<S>,
    memory: crate::memory::MemoryProfile,
    neighbours: AppMutex<neighbours::NeighbourTable>,
    remote_logins: AppMutex<remote::RemoteLoginTable>,
    inbound: Channel<CriticalSectionRawMutex, RxEvent, INBOUND_QUEUE_CAPACITY>,
    outbound: Channel<CriticalSectionRawMutex, QueuedTransmit, OUTBOUND_QUEUE_CAPACITY>,
    ota_requested: Cell<bool>,
    ota_waker: RefCell<Option<Waker>>,
    ota_generation: Cell<u32>,
    reboot_after_next_remote_reply: Cell<bool>,
    seen_packets: RefCell<SeenPacketCache>,
    pending_discover: RefCell<Option<PendingDiscover>>,
    started_at_ms: u64,
    packets_received: AtomicU32,
    packets_sent: AtomicU32,
    packet_errors: AtomicU32,
    battery_level_percent: AtomicU8,
    battery_millivolts: AtomicU16,
    last_rssi: AtomicI16,
    last_snr: AtomicI16,
}

impl<S> AppContext<S>
where
    S: crate::platform::storage::Storage,
{
    pub fn new(
        config: config::AppConfig,
        storage: S,
        memory: crate::memory::MemoryProfile,
    ) -> Self {
        Self {
            config: Mutex::new(config),
            storage: Mutex::new(storage),
            memory,
            neighbours: Mutex::new(neighbours::NeighbourTable::new(memory.max_neighbours)),
            remote_logins: Mutex::new(remote::RemoteLoginTable::new()),
            inbound: Channel::new(),
            outbound: Channel::new(),
            ota_requested: Cell::new(false),
            ota_waker: RefCell::new(None),
            ota_generation: Cell::new(0),
            reboot_after_next_remote_reply: Cell::new(false),
            seen_packets: RefCell::new(SeenPacketCache::new_with_capacity(
                SEEN_PACKET_TTL_MS,
                memory.seen_packet_cache_len,
            )),
            pending_discover: RefCell::new(None),
            started_at_ms: crate::platform::now_millis(),
            packets_received: AtomicU32::new(0),
            packets_sent: AtomicU32::new(0),
            packet_errors: AtomicU32::new(0),
            battery_level_percent: AtomicU8::new(BATTERY_LEVEL_UNKNOWN),
            battery_millivolts: AtomicU16::new(BATTERY_MILLIVOLTS_UNKNOWN),
            last_rssi: AtomicI16::new(0),
            last_snr: AtomicI16::new(0),
        }
    }

    pub async fn with_config<R>(&self, read: impl FnOnce(&config::AppConfig) -> R) -> R {
        let config = self.config.lock().await;
        read(&config)
    }

    pub async fn with_identity<R>(&self, read: impl FnOnce(&identity::Identity) -> R) -> R {
        let config = self.config.lock().await;
        read(config.identity())
    }

    pub async fn public_key(&self) -> [u8; mcrs_protocol::PUB_KEY_SIZE] {
        self.with_identity(|identity| *identity.public_key()).await
    }

    pub async fn public_key_prefix<const N: usize>(&self) -> [u8; N] {
        let public_key = self.public_key().await;
        let mut prefix = [0; N];
        prefix.copy_from_slice(&public_key[..N]);
        prefix
    }

    pub async fn update_config<F>(&self, update: F) -> Result<(), ConfigUpdateError>
    where
        F: FnOnce(&mut config::AppConfig) -> Result<(), config::ConfigError>,
    {
        let mut config = self.config.lock().await;
        update(&mut config)?;
        let mut storage = self.storage.lock().await;
        Ok(config.save(&mut *storage)?)
    }

    pub async fn reset_config(&self) -> Result<(), crate::platform::storage::Error> {
        let mut storage = self.storage.lock().await;
        config::AppConfig::save_unprovisioned(&mut *storage)
    }

    pub async fn observe_neighbour_packet(
        &self,
        packet: &Packet,
        rssi: i16,
        snr: i16,
        now_ms: u64,
    ) {
        let node_hash = self.node_hash().await;
        self.neighbours
            .lock()
            .await
            .observe_packet(packet, rssi, snr, now_ms, &node_hash);
    }

    pub async fn observe_neighbour_public_key(
        &self,
        public_key: [u8; mcrs_protocol::PUB_KEY_SIZE],
        rssi: i16,
        snr: i16,
        now_ms: u64,
    ) {
        let node_hash = self.node_hash().await;
        self.neighbours
            .lock()
            .await
            .observe_public_key(public_key, rssi, snr, now_ms, &node_hash);
    }

    pub async fn encode_neighbours_binary_response(
        &self,
        request: &[u8],
        now_ms: u64,
        out: &mut Vec<u8>,
    ) -> bool {
        self.neighbours
            .lock()
            .await
            .encode_binary_response(request, now_ms, out)
    }

    pub async fn write_neighbours_summary(&self, output: &mut impl fmt::Write) {
        self.neighbours.lock().await.write_summary(output);
    }

    pub async fn authenticate_remote_login(
        &self,
        public_key: &[u8; mcrs_protocol::PUB_KEY_SIZE],
        shared_secret: &[u8; 32],
        privilege: remote::RemotePrivilege,
        last_timestamp: u32,
        now_ms: u64,
    ) {
        self.remote_logins.lock().await.authenticate(
            public_key,
            shared_secret,
            privilege,
            last_timestamp,
            now_ms,
        );
    }

    pub async fn remote_privilege_for(
        &self,
        public_key: &[u8; mcrs_protocol::PUB_KEY_SIZE],
        now_ms: u64,
    ) -> Option<remote::RemotePrivilege> {
        self.remote_logins
            .lock()
            .await
            .privilege_for(public_key, now_ms)
    }

    pub async fn remote_sessions_matching_source_hash(
        &self,
        source_hash: u8,
        now_ms: u64,
    ) -> [Option<remote::RemoteSession>; remote::MAX_REMOTE_SESSIONS] {
        self.remote_logins
            .lock()
            .await
            .sessions_matching_source_hash(source_hash, now_ms)
    }

    pub async fn accept_newer_remote_timestamp(
        &self,
        public_key: &[u8; mcrs_protocol::PUB_KEY_SIZE],
        privilege: remote::RemotePrivilege,
        timestamp: u32,
        now_ms: u64,
    ) -> bool {
        self.remote_logins
            .lock()
            .await
            .accept_newer_timestamp(public_key, privilege, timestamp, now_ms)
    }

    pub async fn node_hash(&self) -> [u8; identity::NODE_HASH_SIZE] {
        self.with_identity(|identity| *identity.node_hash()).await
    }

    pub fn status(&self) -> Status {
        Status {
            uptime_seconds: crate::platform::now_millis().saturating_sub(self.started_at_ms) / 1000,
            packets_received: self.packets_received.load(Ordering::Relaxed),
            packets_sent: self.packets_sent.load(Ordering::Relaxed),
            packet_errors: self.packet_errors.load(Ordering::Relaxed),
            outbound_queue_len: self.outbound.len().min(u16::MAX as usize) as u16,
            battery_level_percent: optional_battery_level(
                self.battery_level_percent.load(Ordering::Relaxed),
            )
            .or_else(crate::platform::battery_level_percent),
            battery_millivolts: optional_battery_millivolts(
                self.battery_millivolts.load(Ordering::Relaxed),
            )
            .or_else(crate::platform::battery_millivolts),
            last_rssi: self.last_rssi.load(Ordering::Relaxed),
            last_snr: self.last_snr.load(Ordering::Relaxed),
        }
    }

    pub async fn outbound_region_label(&self, packet: &[u8]) -> Option<String> {
        let packet = Packet::decode(packet).ok()?;
        self.packet_region_label(&packet).await
    }

    async fn packet_region_label(&self, packet: &Packet) -> Option<String> {
        match packet.route_type {
            RouteType::TransportFlood => {
                self.with_config(|config| {
                    config
                        .regions()
                        .match_flood_region(&packet)
                        .map(|region| String::from(region.display_name()))
                })
                .await
            }
            RouteType::Flood => Some(String::from("*")),
            _ => None,
        }
    }

    pub fn set_battery_millivolts(&self, battery_millivolts: Option<u16>) {
        self.battery_millivolts.store(
            battery_millivolts.unwrap_or(BATTERY_MILLIVOLTS_UNKNOWN),
            Ordering::Relaxed,
        );
    }

    pub fn set_battery_level_percent(&self, battery_level_percent: Option<u8>) {
        self.battery_level_percent.store(
            battery_level_percent
                .map(|level| level.min(100))
                .unwrap_or(BATTERY_LEVEL_UNKNOWN),
            Ordering::Relaxed,
        );
    }

    fn record_packet_received(&self, rssi: i16, snr: i16) {
        self.packets_received
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_add(1))
            })
            .ok();
        self.last_rssi.store(rssi, Ordering::Relaxed);
        self.last_snr.store(snr, Ordering::Relaxed);
    }

    fn record_packet_sent(&self) {
        self.packets_sent
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_add(1))
            })
            .ok();
    }

    fn record_packet_error(&self) {
        self.packet_errors
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                Some(count.saturating_add(1))
            })
            .ok();
    }

    fn enqueue_inbound(&self, payload: &[u8], rssi: i16, snr: i16) -> Result<(), InboundError> {
        if self.inbound.len() >= self.memory.inbound_queue_len {
            return Err(InboundError::QueueFull);
        }

        let mut packet = Vec::new();
        packet.extend_from_slice(payload);
        self.inbound
            .try_send(RxEvent {
                payload: packet,
                rssi,
                snr,
                received_at_ms: crate::platform::now_millis(),
            })
            .map_err(|_| InboundError::QueueFull)
    }

    async fn receive_inbound(&self) -> RxEvent {
        self.inbound.receive().await
    }

    pub fn enqueue_outbound(&self, packet: Vec<u8>) -> Result<(), OutboundError> {
        self.enqueue_outbound_after(packet, 0)
    }

    pub fn enqueue_outbound_after(
        &self,
        packet: Vec<u8>,
        delay_ms: u32,
    ) -> Result<(), OutboundError> {
        self.enqueue_outbound_after_internal(packet, delay_ms, false)
    }

    pub fn enqueue_outbound_reboot_after_tx(&self, packet: Vec<u8>) -> Result<(), OutboundError> {
        self.enqueue_outbound_after_internal(packet, 0, true)
    }

    fn enqueue_outbound_after_internal(
        &self,
        packet: Vec<u8>,
        delay_ms: u32,
        reboot_after_tx: bool,
    ) -> Result<(), OutboundError> {
        if self.outbound.len() >= self.memory.outbound_queue_len {
            return Err(OutboundError::QueueFull);
        }
        let queued = QueuedTransmit {
            eligible_at_ms: crate::platform::now_millis().saturating_add(delay_ms as u64),
            packet,
            reboot_after_tx,
        };
        self.outbound
            .try_send(queued)
            .map_err(|_| OutboundError::QueueFull)?;
        Ok(())
    }

    fn try_receive_outbound(&self) -> Option<QueuedTransmit> {
        self.outbound.try_receive().ok()
    }

    async fn receive_outbound(&self) -> QueuedTransmit {
        self.outbound.receive().await
    }

    pub fn request_reboot_after_next_remote_reply(&self) {
        self.reboot_after_next_remote_reply.set(true);
    }

    pub fn request_ota_start(&self) {
        self.ota_requested.set(true);
        self.wake_ota();
    }

    pub fn request_ota_stop(&self) {
        self.ota_requested.set(false);
        self.wake_ota();
    }

    pub fn ota_requested(&self) -> bool {
        self.ota_requested.get()
    }

    pub fn ota_generation(&self) -> u32 {
        self.ota_generation.get()
    }

    pub fn register_ota_waker(&self, waker: &Waker) {
        self.ota_waker.borrow_mut().replace(waker.clone());
    }

    fn wake_ota(&self) {
        self.ota_generation
            .set(self.ota_generation.get().wrapping_add(1));
        if let Some(waker) = self.ota_waker.borrow_mut().take() {
            waker.wake();
        }
    }

    fn take_reboot_after_next_remote_reply(&self) -> bool {
        let reboot = self.reboot_after_next_remote_reply.get();
        self.reboot_after_next_remote_reply.set(false);
        reboot
    }

    fn mark_seen(&self, signature: [u8; 8]) -> bool {
        self.seen_packets
            .borrow_mut()
            .check_and_insert(signature, crate::platform::now_millis())
    }

    pub fn start_discover_neighbours(&self, tag: u32, now_ms: u64) {
        *self.pending_discover.borrow_mut() = Some(PendingDiscover {
            tag,
            expires_at_ms: now_ms.saturating_add(discovery::DISCOVER_NEIGHBOURS_TTL_MS),
        });
    }

    fn accept_discover_response(&self, tag: u32, now_ms: u64) -> bool {
        let mut pending = self.pending_discover.borrow_mut();
        let Some(discover) = *pending else {
            return false;
        };
        if now_ms > discover.expires_at_ms {
            *pending = None;
            return false;
        }
        discover.tag == tag
    }
}

#[derive(Debug)]
pub enum ConfigUpdateError {
    Invalid(config::ConfigError),
    Storage(crate::platform::storage::Error),
}

impl fmt::Display for ConfigUpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigUpdateError::Invalid(error) => write!(f, "{}", error),
            ConfigUpdateError::Storage(error) => write!(f, "Storage write failed: {:?}", error),
        }
    }
}

impl From<config::ConfigError> for ConfigUpdateError {
    fn from(error: config::ConfigError) -> Self {
        Self::Invalid(error)
    }
}

impl From<crate::platform::storage::Error> for ConfigUpdateError {
    fn from(error: crate::platform::storage::Error) -> Self {
        Self::Storage(error)
    }
}

fn optional_battery_level(level: u8) -> Option<u8> {
    (level != BATTERY_LEVEL_UNKNOWN).then_some(level)
}

fn optional_battery_millivolts(millivolts: u16) -> Option<u16> {
    (millivolts != BATTERY_MILLIVOLTS_UNKNOWN).then_some(millivolts)
}

#[derive(Debug)]
pub enum InboundError {
    QueueFull,
}

#[derive(Debug)]
pub enum OutboundError {
    QueueFull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub uptime_seconds: u64,
    pub packets_received: u32,
    pub packets_sent: u32,
    pub packet_errors: u32,
    pub outbound_queue_len: u16,
    pub battery_level_percent: Option<u8>,
    pub battery_millivolts: Option<u16>,
    pub last_rssi: i16,
    pub last_snr: i16,
}

#[derive(Clone, Copy)]
struct PendingDiscover {
    tag: u32,
    expires_at_ms: u64,
}

struct QueuedTransmit {
    eligible_at_ms: u64,
    packet: Vec<u8>,
    reboot_after_tx: bool,
}

struct OutboundSchedule {
    pending: VecDeque<QueuedTransmit>,
}

impl OutboundSchedule {
    fn new() -> Self {
        Self {
            pending: VecDeque::new(),
        }
    }

    fn push(&mut self, queued: QueuedTransmit) {
        let index = self
            .pending
            .iter()
            .position(|pending| queued.eligible_at_ms < pending.eligible_at_ms)
            .unwrap_or(self.pending.len());
        self.pending.insert(index, queued);
    }

    fn pop_eligible(&mut self, now_ms: u64) -> Option<QueuedTransmit> {
        match self.pending.front() {
            Some(queued) if queued.eligible_at_ms <= now_ms => self.pending.pop_front(),
            _ => None,
        }
    }

    fn has_eligible(&self, now_ms: u64) -> bool {
        self.pending
            .front()
            .is_some_and(|queued| queued.eligible_at_ms <= now_ms)
    }

    fn next_eligible_ms(&self) -> Option<u64> {
        self.pending.front().map(|queued| queued.eligible_at_ms)
    }
}

struct RxEvent {
    payload: Vec<u8>,
    rssi: i16,
    snr: i16,
    received_at_ms: u64,
}

enum RadioWait {
    Packet(Result<crate::modules::ReceivedPacket, ()>),
    NewOutbound(QueuedTransmit),
    OutboundDue,
}

pub async fn radio_loop<R, S, D>(radio: &mut R, context: &AppContext<S>, delay: &mut D) -> !
where
    R: crate::modules::Receiver,
    S: crate::platform::storage::Storage,
    D: DelayNs,
{
    let mut receive_buffer = [0u8; RECEIVE_BUFFER_LEN];
    let mut outbound = OutboundSchedule::new();

    loop {
        drain_outbound_channel(context, &mut outbound);
        drain_eligible_outbound(radio, context, delay, &mut outbound).await;

        match wait_for_read_or_eligible_outbound(
            radio,
            context,
            &mut receive_buffer,
            delay,
            &outbound,
        )
        .await
        {
            RadioWait::OutboundDue => continue,
            RadioWait::NewOutbound(queued) => {
                outbound.push(queued);
                continue;
            }
            RadioWait::Packet(Ok(packet)) => {
                context.record_packet_received(packet.rssi, packet.snr);
                if packet.len > receive_buffer.len() {
                    context.record_packet_error();
                    crate::platform::log_fmt(format_args!(
                        "Radio RX: packet length {} exceeds buffer {}",
                        packet.len,
                        receive_buffer.len()
                    ));
                    continue;
                }
                let payload = &receive_buffer[..packet.len];
                crate::platform::log_radio_packet_received(
                    packet.len,
                    packet.rssi,
                    packet.snr,
                    payload,
                );

                if context
                    .enqueue_inbound(payload, packet.rssi, packet.snr)
                    .is_err()
                {
                    context.record_packet_error();
                    crate::platform::log_fmt(format_args!("Radio RX: inbound queue full"));
                }
            }
            RadioWait::Packet(Err(())) => {
                context.record_packet_error();
                crate::platform::log_radio_receive_failed();
                delay.delay_ms(1000).await;
            }
        }
    }
}

pub async fn handler_loop<S>(context: &AppContext<S>) -> !
where
    S: crate::platform::storage::Storage,
{
    loop {
        let event = context.receive_inbound().await;
        handle_rx_event(context, event).await;
    }
}

async fn handle_rx_event<S>(context: &AppContext<S>, event: RxEvent)
where
    S: crate::platform::storage::Storage,
{
    match mcrs_protocol::Packet::decode(&event.payload) {
        Ok(protocol_packet) => {
            let region = context.packet_region_label(&protocol_packet).await;
            protocol_log::log_packet(&protocol_packet, region.as_deref());
            let node_hash = context.node_hash().await;
            context
                .observe_neighbour_packet(
                    &protocol_packet,
                    event.rssi,
                    event.snr,
                    event.received_at_ms,
                )
                .await;
            if let Some(response) = cli::handle_remote_packet(&protocol_packet, context).await {
                crate::platform::log_fmt(format_args!(
                    "Remote response: queueing {} bytes",
                    response.len()
                ));
                let result = if context.take_reboot_after_next_remote_reply() {
                    context.enqueue_outbound_reboot_after_tx(response)
                } else {
                    context.enqueue_outbound(response)
                };
                if result.is_err() {
                    crate::platform::log_fmt(format_args!("Remote response: outbound queue full"));
                }
            }
            if let Some(response) = discovery::handle_control_packet(
                &protocol_packet,
                context,
                event.rssi,
                event.snr,
                event.received_at_ms,
            )
            .await
            {
                crate::platform::log_fmt(format_args!(
                    "Discovery response: queueing {} bytes",
                    response.len()
                ));
                if context.enqueue_outbound(response).is_err() {
                    crate::platform::log_fmt(format_args!(
                        "Discovery response: outbound queue full"
                    ));
                }
            }
            apply_repeater_rules(context, protocol_packet, event.snr, &node_hash).await;
        }
        Err(error) => {
            context.record_packet_error();
            protocol_log::log_decode_failed(&error);
        }
    }
}

fn drain_outbound_channel<S>(context: &AppContext<S>, outbound: &mut OutboundSchedule)
where
    S: crate::platform::storage::Storage,
{
    while let Some(queued) = context.try_receive_outbound() {
        outbound.push(queued);
    }
}

async fn drain_eligible_outbound<R, S, D>(
    radio: &mut R,
    context: &AppContext<S>,
    delay: &mut D,
    outbound: &mut OutboundSchedule,
) where
    R: crate::modules::Receiver,
    S: crate::platform::storage::Storage,
    D: DelayNs,
{
    while let Some(queued) = outbound.pop_eligible(crate::platform::now_millis()) {
        mark_outbound_seen(context, &queued.packet);
        if let Some(region) = context.outbound_region_label(&queued.packet).await {
            crate::platform::log_fmt(format_args!(
                "Outbound packet: transmitting {} bytes region={}",
                queued.packet.len(),
                region
            ));
        } else {
            crate::platform::log_fmt(format_args!(
                "Outbound packet: transmitting {} bytes",
                queued.packet.len()
            ));
        }
        let radio_config = context.with_config(|config| config.radio()).await;
        if transmit_when_clear(radio, &queued.packet, radio_config, delay)
            .await
            .is_err()
        {
            context.record_packet_error();
            crate::platform::log_fmt(format_args!("Outbound packet: transmit failed"));
            return;
        }
        context.record_packet_sent();
        if queued.reboot_after_tx {
            crate::platform::reboot();
        }
    }
}

fn mark_outbound_seen<S>(context: &AppContext<S>, packet: &[u8])
where
    S: crate::platform::storage::Storage,
{
    let Ok(packet) = Packet::decode(packet) else {
        return;
    };
    if !packet.route_type.is_flood() {
        return;
    }
    let Ok(signature) = packet.dedup_signature() else {
        return;
    };
    context.mark_seen(signature);
}

async fn wait_for_read_or_eligible_outbound<R, S, D>(
    radio: &mut R,
    context: &AppContext<S>,
    receive_buffer: &mut [u8],
    delay: &mut D,
    outbound: &OutboundSchedule,
) -> RadioWait
where
    R: crate::modules::Receiver,
    S: crate::platform::storage::Storage,
    D: DelayNs,
{
    if outbound.has_eligible(crate::platform::now_millis()) {
        return RadioWait::OutboundDue;
    }

    let mut receive = pin!(radio.wait_for_read(receive_buffer));
    let mut new_outbound = pin!(context.receive_outbound());

    let Some(eligible_at_ms) = outbound.next_eligible_ms() else {
        return poll_fn(|cx| {
            if let Poll::Ready(queued) = new_outbound.as_mut().poll(cx) {
                return Poll::Ready(RadioWait::NewOutbound(queued));
            }

            match receive.as_mut().poll(cx) {
                Poll::Ready(result) => Poll::Ready(RadioWait::Packet(result)),
                Poll::Pending => Poll::Pending,
            }
        })
        .await;
    };

    let wait_ms = eligible_at_ms
        .saturating_sub(crate::platform::now_millis())
        .min(u32::MAX as u64) as u32;
    let mut timer = pin!(delay.delay_ms(wait_ms));
    poll_fn(|cx| {
        if let Poll::Ready(queued) = new_outbound.as_mut().poll(cx) {
            return Poll::Ready(RadioWait::NewOutbound(queued));
        }

        match receive.as_mut().poll(cx) {
            Poll::Ready(result) => Poll::Ready(RadioWait::Packet(result)),
            Poll::Pending => {
                if timer.as_mut().poll(cx).is_ready() {
                    Poll::Ready(RadioWait::OutboundDue)
                } else {
                    Poll::Pending
                }
            }
        }
    })
    .await
}

async fn apply_repeater_rules<S>(
    context: &AppContext<S>,
    packet: Packet,
    snr_quarters: i16,
    node_hash: &[u8],
) where
    S: crate::platform::storage::Storage,
{
    match prepare_forward(context, packet, snr_quarters, node_hash).await {
        ForwardDecision::Forward { packet, signature } => {
            queue_repeater_packet(context, packet, signature, node_hash, "forward").await;
        }
        ForwardDecision::Capture { packet, signature } => {
            queue_repeater_packet(context, packet, signature, node_hash, "capture").await;
        }
        ForwardDecision::Drop(reason) => {
            crate::platform::log_fmt(format_args!("Protocol repeater: drop: {}", reason));
        }
        ForwardDecision::DoNotForward => {}
    }
}

async fn queue_repeater_packet<S>(
    context: &AppContext<S>,
    packet: Packet,
    signature: [u8; 8],
    node_hash: &[u8],
    action: &'static str,
) where
    S: crate::platform::storage::Storage,
{
    let delay_ms = forwarding_delay_ms(signature, node_hash);
    let Ok(encoded) = packet.encode() else {
        crate::platform::log_fmt(format_args!("Protocol repeater: {} encode failed", action));
        return;
    };
    if encoded.len() > FORWARD_BUFFER_LEN {
        crate::platform::log_fmt(format_args!(
            "Protocol repeater: encoded {} too long: {} bytes",
            action,
            encoded.len()
        ));
        return;
    }

    let encoded_len = encoded.len();
    if context.enqueue_outbound_after(encoded, delay_ms).is_err() {
        crate::platform::log_fmt(format_args!("Protocol repeater: outbound queue full"));
        return;
    }

    crate::platform::log_fmt(format_args!(
        "Protocol repeater: queued {} {} bytes, eligible in {} ms",
        action, encoded_len, delay_ms
    ));
}

async fn prepare_forward(
    context: &AppContext<impl crate::platform::storage::Storage>,
    packet: Packet,
    snr_quarters: i16,
    node_hash: &[u8],
) -> ForwardDecision {
    if matches!(
        packet.payload.kind(),
        PayloadKind::Reserved(_) | PayloadKind::RawCustom
    ) {
        return ForwardDecision::Drop("unsupported payload kind");
    }

    let signature = match packet.dedup_signature() {
        Ok(signature) => signature,
        Err(_) => return ForwardDecision::Drop("dedup signature failed"),
    };

    match packet.payload.kind() {
        PayloadKind::Trace => {
            prepare_trace_forward(context, packet, signature, snr_quarters, node_hash)
        }
        _ if packet.route_type.is_flood() => {
            prepare_flood_forward(context, packet, signature, node_hash).await
        }
        _ if packet.route_type.is_direct() => {
            prepare_direct_forward(context, packet, signature, node_hash).await
        }
        _ => ForwardDecision::Drop("unknown route"),
    }
}

async fn prepare_flood_forward(
    context: &AppContext<impl crate::platform::storage::Storage>,
    mut packet: Packet,
    signature: [u8; 8],
    node_hash: &[u8],
) -> ForwardDecision {
    if !matches!(
        packet.route_type,
        RouteType::Flood | RouteType::TransportFlood
    ) {
        return ForwardDecision::Drop("not flood routed");
    }

    let captured = if packet.route_type == RouteType::Flood {
        match context
            .with_config(|config| {
                if !config.region_capture() {
                    return Ok(false);
                }
                if packet
                    .normal_path()
                    .is_none_or(|path| path.hop_count() != 0)
                {
                    return Ok(false);
                }
                config.regions().apply_default_scope(&mut packet)
            })
            .await
        {
            Ok(captured) => captured,
            Err(_) => return ForwardDecision::Drop("default region scope failed"),
        }
    } else {
        false
    };

    let Some((unscoped, max_unscoped_hops, max_advert_hops)) = context
        .with_config(|config| {
            config.regions().match_flood_region(&packet).map(|region| {
                (
                    region.is_wildcard(),
                    config.flood_max_unscoped_hops(),
                    config.flood_max_advert_hops(),
                )
            })
        })
        .await
    else {
        return ForwardDecision::Drop("unknown or denied flood region");
    };

    let Some(path) = packet.normal_path() else {
        return ForwardDecision::Drop("flood packet without normal path");
    };
    let hop_count = path.hop_count();
    if path.contains_hash(node_hash) {
        return ForwardDecision::Drop("loop detected");
    }
    if packet.payload.kind() == PayloadKind::Advert && hop_count >= max_advert_hops as usize {
        return ForwardDecision::DoNotForward;
    }
    if unscoped && hop_count >= max_unscoped_hops as usize {
        return ForwardDecision::DoNotForward;
    }

    if !context.mark_seen(signature) {
        return ForwardDecision::Drop("duplicate");
    }

    if packet.append_flood_hop(node_hash).is_err() {
        return ForwardDecision::DoNotForward;
    }

    if captured {
        ForwardDecision::Capture { packet, signature }
    } else {
        ForwardDecision::Forward { packet, signature }
    }
}

async fn prepare_direct_forward(
    context: &AppContext<impl crate::platform::storage::Storage>,
    mut packet: Packet,
    signature: [u8; 8],
    node_hash: &[u8],
) -> ForwardDecision {
    if !matches!(
        packet.route_type,
        RouteType::Direct | RouteType::TransportDirect
    ) {
        return ForwardDecision::Drop("not direct routed");
    }

    let Some(path) = packet.normal_path() else {
        return ForwardDecision::Drop("direct packet without normal path");
    };
    if path.hop_count() == 0 {
        return ForwardDecision::DoNotForward;
    }

    match packet.consume_direct_hop(node_hash) {
        Ok(true) => {
            if !context.mark_seen(signature) {
                return ForwardDecision::Drop("duplicate");
            }
            ForwardDecision::Forward { packet, signature }
        }
        Ok(false) => ForwardDecision::DoNotForward,
        Err(_) => ForwardDecision::Drop("direct path mutation failed"),
    }
}

fn prepare_trace_forward(
    context: &AppContext<impl crate::platform::storage::Storage>,
    mut packet: Packet,
    signature: [u8; 8],
    snr_quarters: i16,
    node_hash: &[u8],
) -> ForwardDecision {
    if packet.route_type != RouteType::Direct {
        return ForwardDecision::Drop("trace is not direct routed");
    }
    if !matches!(&packet.path, RoutePath::Trace(_)) {
        return ForwardDecision::Drop("trace packet without trace path");
    }
    match packet.trace_is_complete() {
        Ok(true) => return ForwardDecision::DoNotForward,
        Ok(false) => {}
        Err(_) => return ForwardDecision::Drop("trace completion check failed"),
    }
    match packet.trace_next_hop_matches(node_hash) {
        Ok(true) => {}
        Ok(false) => return ForwardDecision::DoNotForward,
        Err(_) => return ForwardDecision::Drop("trace next-hop check failed"),
    }

    if !context.mark_seen(signature) {
        return ForwardDecision::Drop("duplicate");
    }

    if packet
        .append_trace_snr(snr_quarters.clamp(i8::MIN as i16, i8::MAX as i16) as i8)
        .is_err()
    {
        return ForwardDecision::DoNotForward;
    }

    ForwardDecision::Forward { packet, signature }
}

fn forwarding_delay_ms(signature: [u8; 8], node_hash: &[u8]) -> u32 {
    FORWARD_DELAY_BASE_MS + (forwarding_jitter_seed(signature, node_hash) % FORWARD_DELAY_JITTER_MS)
}

fn forwarding_jitter_seed(signature: [u8; 8], node_hash: &[u8]) -> u32 {
    let mut seed = 0x811c_9dc5u32;
    for byte in signature.iter().chain(node_hash.iter()) {
        seed ^= u32::from(*byte);
        seed = seed.wrapping_mul(0x0100_0193);
    }
    seed
}

async fn transmit_when_clear<R, D>(
    radio: &mut R,
    payload: &[u8],
    radio_config: config::RadioConfig,
    delay: &mut D,
) -> Result<(), ()>
where
    R: crate::modules::Receiver,
    D: DelayNs,
{
    for attempt in 0..CAD_MAX_ATTEMPTS {
        match radio.channel_is_busy().await {
            Ok(false) => {
                return radio.transmit(payload).await;
            }
            Ok(true) => {
                let delay_ms = cad_busy_backoff_ms(payload, radio_config, attempt);
                delay.delay_ms(delay_ms).await;
            }
            Err(()) => {
                crate::platform::log_fmt(format_args!("CAD: failed"));
                return Err(());
            }
        }
    }

    crate::platform::log_fmt(format_args!("CAD: channel stayed busy, dropping transmit"));
    Err(())
}

fn cad_busy_backoff_ms(payload: &[u8], radio_config: config::RadioConfig, attempt: usize) -> u32 {
    let airtime_ms = radio_config.packet_airtime_ms(payload.len());
    let cad_time_ms = radio_config
        .symbol_time_ms()
        .saturating_mul(CAD_BUSY_BACKOFF_SYMBOLS);
    let minimum_ms = CAD_BUSY_BACKOFF_BASE_MS.max(cad_time_ms);
    let lower_ms = minimum_ms.max(airtime_ms / CAD_BUSY_BACKOFF_AIRTIME_DIVISOR);
    let jitter_window_ms = airtime_ms.saturating_sub(lower_ms).max(minimum_ms);
    let seed = payload
        .get(attempt % payload.len().max(1))
        .copied()
        .unwrap_or(0) as u32;
    lower_ms
        .saturating_add(attempt as u32 * minimum_ms)
        .saturating_add(seed % jitter_window_ms)
}

enum ForwardDecision {
    Forward { packet: Packet, signature: [u8; 8] },
    Capture { packet: Packet, signature: [u8; 8] },
    DoNotForward,
    Drop(&'static str),
}
