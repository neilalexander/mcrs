//! Remote admin proxy.
//!
//! A repeater can hold an admin session to another node and run CLI commands
//! on its behalf. The operator reaches this repeater over serial or telnet;
//! the proxy logs in with this repeater's own identity, learns the forward
//! path from the login response, and sends commands as encrypted CLI text.
//!
//! This is the client half of the remote management protocol that the rest of
//! this crate already implements as a responder.

use super::{AppContext, crypto};
use crate::platform::{Platform, Storage};
use alloc::{format, string::String, vec::Vec};
use core::fmt::Write;
use defmt::{info, warn};
use embassy_futures::select::{Either, select};
use embedded_hal_async::delay::DelayNs;
use mcrs_protocol::{
    HashSize, MAX_TEXT_LEN, PERM_ACL_ADMIN, PUB_KEY_SIZE, Packet, Path, PathPlaintext, Payload,
    PayloadKind, RepeaterLoginResponsePlaintext, TextMessagePlaintext, TextType,
};

/// How many targets can hold a session at once.
pub const MAX_PROXY_TARGETS: usize = 2;

const LOGIN_TIMEOUT_MS: u64 = 15_000;
const COMMAND_TIMEOUT_MS: u64 = 10_000;
const SESSION_TTL_MS: u64 = 30 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyState {
    LoginSent,
    Ready,
}

#[derive(Clone)]
pub struct ProxyTarget {
    pub public_key: [u8; PUB_KEY_SIZE],
    pub shared_secret: [u8; 32],
    /// Forward route to the target, learned from a flooded login.
    pub path: Option<Path>,
    pub hash_size: HashSize,
    pub last_timestamp: u32,
    pub seq: u8,
    pub state: ProxyState,
    pub permissions: u8,
    pub last_seen_ms: u64,
}

pub struct ProxyTable {
    entries: [Option<ProxyTarget>; MAX_PROXY_TARGETS],
}

impl ProxyTable {
    pub(crate) const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_PROXY_TARGETS],
        }
    }

    pub fn get(&self, public_key: &[u8; PUB_KEY_SIZE]) -> Option<&ProxyTarget> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| &entry.public_key == public_key)
    }

    pub fn get_mut(&mut self, public_key: &[u8; PUB_KEY_SIZE]) -> Option<&mut ProxyTarget> {
        self.entries
            .iter_mut()
            .flatten()
            .find(|entry| &entry.public_key == public_key)
    }

    pub fn targets(&self) -> impl Iterator<Item = &ProxyTarget> {
        self.entries.iter().flatten()
    }

    pub fn remove(&mut self, public_key: &[u8; PUB_KEY_SIZE]) -> bool {
        for slot in &mut self.entries {
            if slot
                .as_ref()
                .is_some_and(|entry| &entry.public_key == public_key)
            {
                *slot = None;
                return true;
            }
        }
        false
    }

    /// Adds or replaces a session, evicting the least recently used target.
    pub fn upsert(&mut self, target: ProxyTarget) {
        if let Some(slot) = self
            .entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.public_key == target.public_key)
        {
            *slot = target;
            return;
        }

        let index = self
            .entries
            .iter()
            .position(Option::is_none)
            .unwrap_or_else(|| {
                self.entries
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| entry.as_ref().map(|entry| (index, entry)))
                    .min_by_key(|(_, entry)| entry.last_seen_ms)
                    .map_or(0, |(index, _)| index)
            });
        self.entries[index] = Some(target);
    }

    fn prune(&mut self, now_ms: u64) {
        for slot in &mut self.entries {
            if slot
                .as_ref()
                .is_some_and(|entry| now_ms.saturating_sub(entry.last_seen_ms) > SESSION_TTL_MS)
            {
                *slot = None;
            }
        }
    }
}

#[derive(Clone)]
pub enum ProxyEvent {
    Login {
        target: [u8; PUB_KEY_SIZE],
        permissions: u8,
    },
    Text {
        target: [u8; PUB_KEY_SIZE],
        seq: Option<u8>,
        message: Vec<u8>,
    },
}

/// Handles the `remote` command family from a local CLI.
pub async fn handle_command<P: Platform, S: Storage>(
    command: &str,
    context: &AppContext<P, S>,
) -> String {
    let Some((first, rest)) =
        split_first_word(command.strip_prefix("remote").unwrap_or(command).trim())
    else {
        return usage();
    };

    match first {
        "list" => list(context).await,
        "login" => {
            let Some((target, password)) = split_first_word(rest) else {
                return String::from("Error - expected: remote login <target> [password]");
            };
            let password = password.trim();
            login(
                target,
                if password.is_empty() {
                    None
                } else {
                    Some(password)
                },
                context,
            )
            .await
        }
        "logout" => logout(rest.trim(), context).await,
        _ => {
            let command = rest.trim();
            if command.is_empty() {
                return String::from("Error - expected: remote <target> <command>");
            }
            run(first, command, context).await
        }
    }
}

/// Consumes an inbound packet that belongs to an outbound proxy session.
///
/// Returns true when the packet was addressed to this node as a proxy client,
/// so the caller can skip repeater forwarding for it.
pub async fn handle_inbound<P: Platform, S: Storage>(
    packet: &Packet,
    context: &AppContext<P, S>,
) -> bool {
    let payload = match &packet.payload {
        Payload::Path(payload) | Payload::Response(payload) | Payload::TextMessage(payload) => {
            payload
        }
        _ => return false,
    };

    let own_key = context.public_key().await;
    let now_ms = context.platform.now_millis();
    let mut table = context.proxy.lock().await;
    table.prune(now_ms);

    for entry in table.entries.iter_mut().flatten() {
        if entry.public_key[0] != payload.source_hash {
            continue;
        }
        let Some(plaintext) = crypto::decrypt_direct_payload(&entry.shared_secret, payload) else {
            continue;
        };
        entry.last_seen_ms = now_ms;

        match &packet.payload {
            Payload::Path(_) => {
                let Ok(path) = PathPlaintext::decode(&plaintext) else {
                    return true;
                };
                entry.hash_size = path.path.hash_size();
                entry.path = Some(path.path.clone());

                match path.extra_type {
                    Some(PayloadKind::Response) => {
                        if let Ok(login) =
                            RepeaterLoginResponsePlaintext::decode(&path.extra_payload)
                        {
                            entry.permissions = login.acl_permissions;
                            entry.state = ProxyState::Ready;
                            context.proxy_events.signal(ProxyEvent::Login {
                                target: entry.public_key,
                                permissions: login.acl_permissions,
                            });
                        }
                    }
                    Some(PayloadKind::TextMessage) => {
                        signal_text(context, entry.public_key, &path.extra_payload);
                    }
                    _ => {}
                }

                // A PATH carried by a flood has no usable return route for the
                // target yet. Send the reverse path back directly, as the
                // companion firmware does, so replies can stop flooding.
                if packet.route_type.is_flood()
                    && let Some(outer) = packet.normal_path()
                {
                    let reciprocal = PathPlaintext {
                        path: outer.clone(),
                        extra_type: None,
                        // A random tail keeps the packet hash unique, as the
                        // companion firmware does for an empty extra payload.
                        extra_payload: context.platform.random_u32().to_le_bytes().to_vec(),
                    };
                    if let Some(encoded) = crypto::encode_direct_path(
                        &entry.shared_secret,
                        &entry.public_key,
                        &own_key,
                        &reciprocal,
                        &path.path,
                    ) && context.enqueue_outbound(encoded).is_err()
                    {
                        warn!("Proxy: reciprocal path queue full");
                    }
                }
                return true;
            }
            Payload::Response(_) => {
                let Ok(login) = RepeaterLoginResponsePlaintext::decode(&plaintext) else {
                    return true;
                };
                entry.permissions = login.acl_permissions;
                entry.state = ProxyState::Ready;
                context.proxy_events.signal(ProxyEvent::Login {
                    target: entry.public_key,
                    permissions: login.acl_permissions,
                });
                return true;
            }
            Payload::TextMessage(_) => {
                signal_text(context, entry.public_key, &plaintext);
                return true;
            }
            _ => {}
        }
    }

    false
}

fn signal_text<P: Platform, S: Storage>(
    context: &AppContext<P, S>,
    target: [u8; PUB_KEY_SIZE],
    plaintext: &[u8],
) {
    let len = plaintext
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |index| index + 1);
    let Ok(text) = TextMessagePlaintext::decode(&plaintext[..len]) else {
        return;
    };
    if text.text_type != TextType::CliData {
        return;
    }

    let message = String::from_utf8_lossy(&text.message).into_owned();
    let seq = super::cli::split_cli_correlation_prefix(&message)
        .0
        .and_then(|prefix| u8::from_str_radix(&prefix[..2], 16).ok());
    context.proxy_events.signal(ProxyEvent::Text {
        target,
        seq,
        message: text.message,
    });
}

async fn login<P: Platform, S: Storage>(
    spec: &str,
    password: Option<&str>,
    context: &AppContext<P, S>,
) -> String {
    let _busy = context.proxy_command.lock().await;

    let Some(target) = resolve_target(spec, context).await else {
        return format!("Error - unknown target '{spec}', use a public key or 4+ hex characters");
    };
    let own_key = context.public_key().await;
    if target == own_key {
        return String::from("Error - target is this node");
    }
    let Some(shared_secret) = context
        .with_identity(|identity| identity.shared_secret_with_ed25519_public(&target))
        .await
    else {
        return String::from("Error - target public key is not a valid curve point");
    };

    let now_ms = context.platform.now_millis();
    let previous = {
        let mut table = context.proxy.lock().await;
        table.prune(now_ms);
        table.get(&target).cloned()
    };
    let timestamp = next_timestamp(context.platform.now_seconds(), previous.as_ref());
    let mut plaintext = timestamp.to_le_bytes().to_vec();
    if let Some(password) = password {
        plaintext.extend_from_slice(password.as_bytes());
    }

    let Some(encoded) =
        crypto::encode_anonymous_request(&own_key, &target, &shared_secret, &plaintext)
    else {
        return String::from("Error - login request encoding failed");
    };
    let Some(encoded) = scope_outbound(encoded, context).await else {
        return String::from("Error - login request encoding failed");
    };

    // Drop any late event from a previous session before this request can
    // produce one of its own.
    context.proxy_events.reset();

    // Register the session before transmitting so a fast response cannot
    // arrive before there is a table entry to match it against.
    {
        let mut table = context.proxy.lock().await;
        table.upsert(ProxyTarget {
            public_key: target,
            shared_secret,
            path: previous.as_ref().and_then(|entry| entry.path.clone()),
            hash_size: previous
                .as_ref()
                .map_or(HashSize::One, |entry| entry.hash_size),
            last_timestamp: timestamp,
            seq: previous.as_ref().map_or(0, |entry| entry.seq),
            state: ProxyState::LoginSent,
            permissions: 0,
            last_seen_ms: now_ms,
        });
    }

    if context.enqueue_outbound(encoded).is_err() {
        context.proxy.lock().await.remove(&target);
        return String::from("Error - outbound queue full");
    }

    let expected = target;
    let Some(event) = wait_for_event(
        context,
        move |event| matches!(event, ProxyEvent::Login { target, .. } if *target == expected),
        LOGIN_TIMEOUT_MS,
    )
    .await
    else {
        // A failed re-login must not throw away a session that still works.
        let mut table = context.proxy.lock().await;
        match previous.filter(|entry| entry.state == ProxyState::Ready) {
            Some(previous) => table.upsert(previous),
            None => {
                table.remove(&target);
            }
        }
        warn!("Proxy: login timed out");
        return format!(
            "Error - no login response from {} within {}s (key, ACL or clock)",
            hex_prefix(&target),
            LOGIN_TIMEOUT_MS / 1000
        );
    };

    let ProxyEvent::Login { permissions, .. } = event else {
        unreachable!()
    };
    let ready = {
        let mut table = context.proxy.lock().await;
        table.get_mut(&target).map(|entry| {
            entry.state = ProxyState::Ready;
            entry.permissions = permissions;
            entry.last_seen_ms = context.platform.now_millis();
            (entry.path.clone(), entry.hash_size)
        })
    };
    let Some((path, _)) = ready else {
        return String::from("Error - session disappeared");
    };

    let role = if is_admin(permissions) {
        "admin"
    } else {
        "guest, target will reject commands"
    };
    let hops = path.as_ref().map_or(0, Path::hop_count);
    info!("Proxy: logged in to target");
    format!(
        "OK - logged in to {} ({role}, {hops} hop{})",
        hex_prefix(&target),
        if hops == 1 { "" } else { "s" }
    )
}

async fn run<P: Platform, S: Storage>(
    spec: &str,
    command: &str,
    context: &AppContext<P, S>,
) -> String {
    let _busy = context.proxy_command.lock().await;

    let Some(target) = resolve_target(spec, context).await else {
        return format!("Error - unknown target '{spec}'");
    };
    let now_ms = context.platform.now_millis();
    let Some(entry) = ({
        let mut table = context.proxy.lock().await;
        table.prune(now_ms);
        table.get(&target).cloned()
    }) else {
        return String::from("Error - no session, run remote login first");
    };
    if entry.state != ProxyState::Ready {
        return String::from("Error - login is not complete");
    }

    let message = format!("{:02x}|{command}", entry.seq);
    if message.len() > MAX_TEXT_LEN {
        return format!(
            "Error - command is {} bytes, the target accepts at most {}",
            message.len(),
            MAX_TEXT_LEN
        );
    }

    let timestamp = next_timestamp(context.platform.now_seconds(), Some(&entry));
    let seq = entry.seq;
    let Ok(plaintext) = (TextMessagePlaintext {
        timestamp,
        text_type: TextType::CliData,
        attempt: 0,
        message: message.into_bytes(),
    })
    .encode() else {
        return String::from("Error - command encoding failed");
    };

    let own_key = context.public_key().await;
    let Some(encoded) = crypto::encode_direct_text(
        &entry.shared_secret,
        &target,
        &own_key,
        &plaintext,
        entry.path.as_ref(),
        entry.hash_size,
        crypto::PayloadKindForEncoding::TextMessage,
    ) else {
        return String::from("Error - command encoding failed");
    };
    let Some(encoded) = scope_outbound(encoded, context).await else {
        return String::from("Error - command encoding failed");
    };

    context.proxy_events.reset();

    {
        let mut table = context.proxy.lock().await;
        if let Some(stored) = table.get_mut(&target) {
            stored.last_timestamp = timestamp;
            stored.seq = seq.wrapping_add(1);
            stored.last_seen_ms = now_ms;
        }
    }

    if context.enqueue_outbound(encoded).is_err() {
        return String::from("Error - outbound queue full");
    }

    let expected = target;
    let Some(event) = wait_for_event(
        context,
        move |event| match event {
            ProxyEvent::Text {
                target, seq: reply, ..
            } => *target == expected && reply.is_none_or(|reply| reply == seq),
            _ => false,
        },
        COMMAND_TIMEOUT_MS,
    )
    .await
    else {
        return format!(
            "Error - no reply from {} within {}s",
            hex_prefix(&target),
            COMMAND_TIMEOUT_MS / 1000
        );
    };

    let ProxyEvent::Text { message, .. } = event else {
        unreachable!()
    };
    let text = String::from_utf8_lossy(&message).into_owned();
    let text = String::from(strip_correlation_prefix(&text).trim());
    if text.is_empty() {
        String::from("OK")
    } else {
        text
    }
}

async fn logout<P: Platform, S: Storage>(spec: &str, context: &AppContext<P, S>) -> String {
    if spec.is_empty() {
        return String::from("Error - expected: remote logout <target>");
    }
    let _busy = context.proxy_command.lock().await;
    let Some(target) = resolve_target(spec, context).await else {
        return format!("Error - unknown target '{spec}'");
    };
    if context.proxy.lock().await.remove(&target) {
        format!("OK - removed session for {}", hex_prefix(&target))
    } else {
        format!("Error - no session for {}", hex_prefix(&target))
    }
}

async fn list<P: Platform, S: Storage>(context: &AppContext<P, S>) -> String {
    let now_ms = context.platform.now_millis();
    let table = context.proxy.lock().await;
    let mut output = String::new();
    let count = table.targets().count();
    if count == 0 {
        return String::from("No remote sessions.");
    }

    let _ = writeln!(output, "Remote sessions: {count}");
    for entry in table.targets() {
        let role = if is_admin(entry.permissions) {
            "admin"
        } else {
            "guest"
        };
        let route = match &entry.path {
            Some(path) => format!("hops={}", path.hop_count()),
            None => String::from("route=flood"),
        };
        let _ = writeln!(
            output,
            "{} {role} {route} state={} idle={}s",
            hex_prefix(&entry.public_key),
            match entry.state {
                ProxyState::LoginSent => "login",
                ProxyState::Ready => "ready",
            },
            now_ms.saturating_sub(entry.last_seen_ms) / 1000
        );
    }
    output
}

async fn resolve_target<P: Platform, S: Storage>(
    spec: &str,
    context: &AppContext<P, S>,
) -> Option<[u8; PUB_KEY_SIZE]> {
    let bytes = parse_hex(spec)?;
    if bytes.len() == PUB_KEY_SIZE {
        let mut key = [0; PUB_KEY_SIZE];
        key.copy_from_slice(&bytes);
        return Some(key);
    }
    if bytes.len() < 2 {
        return None;
    }

    let from_session = {
        let table = context.proxy.lock().await;
        table
            .targets()
            .find(|entry| entry.public_key.starts_with(&bytes))
            .map(|entry| entry.public_key)
    };
    if from_session.is_some() {
        return from_session;
    }

    context
        .neighbours
        .lock()
        .await
        .public_key_matching_prefix(&bytes)
}

async fn scope_outbound<P: Platform, S: Storage>(
    encoded: Vec<u8>,
    context: &AppContext<P, S>,
) -> Option<Vec<u8>> {
    let mut packet = Packet::decode(&encoded).ok()?;
    if packet.route_type.is_flood()
        && let Err(error) = context
            .with_config(|config| config.regions().apply_default_scope(&mut packet))
            .await
    {
        warn!("Proxy: default region scope failed: {}", error);
        return None;
    }
    packet.encode().ok()
}

async fn wait_for_event<P: Platform, S: Storage>(
    context: &AppContext<P, S>,
    accept: impl Fn(&ProxyEvent) -> bool,
    timeout_ms: u64,
) -> Option<ProxyEvent> {
    let deadline = context.platform.now_millis().saturating_add(timeout_ms);
    let mut delay = context.platform.delay();
    loop {
        let remaining = deadline.saturating_sub(context.platform.now_millis());
        if remaining == 0 {
            return None;
        }
        let wait_ms = remaining.min(u32::MAX as u64) as u32;
        match select(context.proxy_events.wait(), delay.delay_ms(wait_ms)).await {
            Either::First(event) => {
                if accept(&event) {
                    return Some(event);
                }
            }
            Either::Second(()) => return None,
        }
    }
}

fn next_timestamp(now: u32, previous: Option<&ProxyTarget>) -> u32 {
    let previous = previous.map_or(0, |entry| entry.last_timestamp);
    now.max(previous.saturating_add(1)).max(1)
}

fn is_admin(permissions: u8) -> bool {
    permissions & PERM_ACL_ADMIN == PERM_ACL_ADMIN
}

fn strip_correlation_prefix(text: &str) -> &str {
    super::cli::split_cli_correlation_prefix(text)
        .1
        .trim_start()
}

fn hex_prefix(public_key: &[u8; PUB_KEY_SIZE]) -> String {
    let mut output = String::new();
    for byte in &public_key[..4] {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn parse_hex(input: &str) -> Option<Vec<u8>> {
    if input.is_empty() || !input.len().is_multiple_of(2) || !input.is_ascii() {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() / 2);
    for index in (0..input.len()).step_by(2) {
        out.push(u8::from_str_radix(&input[index..index + 2], 16).ok()?);
    }
    Some(out)
}

fn split_first_word(input: &str) -> Option<(&str, &str)> {
    let input = input.trim_start();
    if input.is_empty() {
        return None;
    }
    match input.split_once(char::is_whitespace) {
        Some((first, rest)) => Some((first, rest.trim_start())),
        None => Some((input, "")),
    }
}

fn usage() -> String {
    String::from(
        "Usage:\nremote login <target> [password]\nremote <target> <command>\nremote list\nremote logout <target>",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::neighbours::NeighbourTable;
    use alloc::vec;

    fn target(
        public_key: [u8; PUB_KEY_SIZE],
        last_timestamp: u32,
        last_seen_ms: u64,
    ) -> ProxyTarget {
        ProxyTarget {
            public_key,
            shared_secret: [0; 32],
            path: None,
            hash_size: HashSize::One,
            last_timestamp,
            seq: 0,
            state: ProxyState::Ready,
            permissions: PERM_ACL_ADMIN,
            last_seen_ms,
        }
    }

    #[test]
    fn table_caps_sessions_and_evicts_the_oldest() {
        let mut table = ProxyTable::new();
        table.upsert(target([1; PUB_KEY_SIZE], 1, 100));
        table.upsert(target([2; PUB_KEY_SIZE], 1, 300));
        table.upsert(target([3; PUB_KEY_SIZE], 1, 200));

        assert!(table.get(&[1; PUB_KEY_SIZE]).is_none());
        assert!(table.get(&[2; PUB_KEY_SIZE]).is_some());
        assert!(table.get(&[3; PUB_KEY_SIZE]).is_some());
        assert_eq!(table.targets().count(), MAX_PROXY_TARGETS);
    }

    #[test]
    fn timestamps_are_strictly_increasing() {
        let mut entry = target([1; PUB_KEY_SIZE], 0, 0);
        assert_eq!(next_timestamp(0, None), 1);
        assert_eq!(next_timestamp(10, None), 10);
        assert_eq!(next_timestamp(10, Some(&entry)), 10);

        entry.last_timestamp = 10;
        assert_eq!(next_timestamp(5, Some(&entry)), 11);
        assert_eq!(next_timestamp(10, Some(&entry)), 11);
        entry.last_timestamp = u32::MAX;
        assert_eq!(next_timestamp(5, Some(&entry)), u32::MAX);
    }

    #[test]
    fn hex_targets_parse_only_whole_bytes() {
        assert_eq!(parse_hex("a1b2").unwrap(), vec![0xa1, 0xb2]);
        assert!(parse_hex("abc").is_none());
        assert!(parse_hex("zz").is_none());
        assert!(parse_hex("").is_none());
    }

    #[test]
    fn neighbour_lookup_matches_the_public_key_prefix() {
        let mut table = NeighbourTable::new(4);
        let key = [0x12, 0x34, 0x56, 0x78].repeat(8);
        let mut public_key = [0; PUB_KEY_SIZE];
        public_key.copy_from_slice(&key[..PUB_KEY_SIZE]);
        table.observe_public_key(public_key, -80, 40, 0, &[]);

        assert_eq!(
            table.public_key_matching_prefix(&[0x12, 0x34]),
            Some(public_key)
        );
        assert_eq!(table.public_key_matching_prefix(&[0x12, 0x35]), None);
    }

    #[test]
    fn correlation_prefix_is_stripped_from_replies() {
        assert_eq!(
            strip_correlation_prefix("0a|OK - rebooted"),
            "OK - rebooted"
        );
        assert_eq!(strip_correlation_prefix("OK - rebooted"), "OK - rebooted");
    }

    #[test]
    fn admin_permissions_require_both_bits() {
        assert!(is_admin(PERM_ACL_ADMIN));
        assert!(!is_admin(1));
        assert!(!is_admin(0));
    }
}
