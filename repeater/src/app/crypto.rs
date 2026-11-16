use super::{identity::Identity, remote::ReplyRoute};
use crate::{Platform, app::AppContext, platform::Storage};
use aes::{
    Aes128,
    cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit},
};
use alloc::vec::Vec;
use hmac::{Hmac, Mac};
use mcrs_protocol::{
    AnonymousRequestPayload, DirectEncryptedPayload, PERM_ACL_ADMIN, Packet, Path, PathPlaintext,
    Payload, PayloadKind, RepeaterLoginResponsePlaintext, RoutePath, RouteType,
    TextMessagePlaintext, TextType,
};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub struct AnonymousPlaintext {
    pub sender_pubkey: [u8; 32],
    pub shared_secret: [u8; 32],
    pub plaintext: Vec<u8>,
}

pub struct AuthenticatedPlaintext {
    pub sender_pubkey: [u8; 32],
    pub shared_secret: [u8; 32],
    pub privilege: super::remote::RemotePrivilege,
    pub reply_route: ReplyRoute,
    pub plaintext: Vec<u8>,
}

impl AuthenticatedPlaintext {
    /// Text and legacy requests omit AES zero padding; PATH must retain binary zeroes.
    pub fn unpadded(&self) -> &[u8] {
        let len = self
            .plaintext
            .iter()
            .rposition(|byte| *byte != 0)
            .map_or(0, |i| i + 1);
        &self.plaintext[..len]
    }
}

pub fn decrypt_anonymous_request(
    payload: &AnonymousRequestPayload,
    identity: &Identity,
) -> Option<AnonymousPlaintext> {
    if payload.destination_hash != identity.public_key()[0] {
        return None;
    }
    if payload.ciphertext.is_empty() || !payload.ciphertext.len().is_multiple_of(16) {
        return None;
    }

    let shared_secret = identity.shared_secret_with_ed25519_public(&payload.sender_pubkey)?;
    if !verify_mac(&shared_secret, payload) {
        return None;
    }

    let plaintext = decrypt_payload(&shared_secret, &payload.ciphertext)?;

    // Keep binary fields intact: a reply path can end in zero bytes.
    // The caller handles padding after identifying the request type.
    Some(AnonymousPlaintext {
        sender_pubkey: payload.sender_pubkey,
        shared_secret,
        plaintext,
    })
}

pub fn decrypt_authenticated_direct_payload(
    payload: &DirectEncryptedPayload,
    identity: &Identity,
    sessions: &[Option<super::remote::RemoteSession>],
) -> Option<AuthenticatedPlaintext> {
    if payload.destination_hash != identity.public_key()[0]
        || !payload.has_complete_ciphertext_blocks()
    {
        return None;
    }
    for session in sessions.iter().flatten() {
        if payload.source_hash != session.public_key[0] {
            continue;
        }
        if !verify_direct_mac(&session.shared_secret, payload) {
            continue;
        }

        let plaintext = decrypt_payload(&session.shared_secret, &payload.ciphertext)?;
        return Some(AuthenticatedPlaintext {
            sender_pubkey: session.public_key,
            shared_secret: session.shared_secret,
            privilege: session.privilege,
            reply_route: session.reply_route.clone(),
            plaintext,
        });
    }

    None
}

pub async fn encode_login_response<P: Platform, S: Storage>(
    context: &AppContext<P, S>,
    request: &Packet,
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    acl_permissions: u8,
    reply_route: ReplyRoute,
) -> Option<Vec<u8>> {
    let response = RepeaterLoginResponsePlaintext {
        server_timestamp: context.platform.now_seconds(),
        keep_alive_interval: 0,
        legacy_permissions: u8::from(acl_permissions == PERM_ACL_ADMIN),
        acl_permissions,
        nonce: response_nonce(
            context,
            shared_secret,
            requester_public_key,
            responder_public_key,
        ),
        firmware_version_level: 2,
    };

    if request.route_type.is_flood() {
        let packet = encode_path_response_packet(
            shared_secret,
            requester_public_key,
            responder_public_key,
            &response.encode(),
            request.normal_path()?.clone(),
        )?;
        return packet.encode().ok();
    }

    encode_response_plaintext(
        shared_secret,
        requester_public_key,
        responder_public_key,
        &response.encode(),
        reply_route,
    )
}

pub fn encode_cli_text_response<P: Platform, S: Storage>(
    context: &AppContext<P, S>,
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    request_timestamp: u32,
    message: &[u8],
    reply_route: ReplyRoute,
) -> Option<Vec<u8>> {
    let mut timestamp: u32 = context.platform.now_seconds();
    if timestamp == request_timestamp {
        timestamp = timestamp.wrapping_add(1);
    }

    let plaintext = TextMessagePlaintext {
        timestamp,
        text_type: TextType::CliData,
        attempt: 0,
        message: message.to_vec(),
    }
    .encode()
    .ok()?;

    encode_direct_payload(
        PayloadKindForEncoding::TextMessage,
        shared_secret,
        requester_public_key,
        responder_public_key,
        &plaintext,
        reply_route,
    )
}

pub fn encode_response_plaintext(
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    plaintext: &[u8],
    reply_route: ReplyRoute,
) -> Option<Vec<u8>> {
    encode_direct_payload(
        PayloadKindForEncoding::Response,
        shared_secret,
        requester_public_key,
        responder_public_key,
        plaintext,
        reply_route,
    )
}

pub fn encode_path_response_packet(
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    plaintext: &[u8],
    discovered_path: Path,
) -> Option<Packet> {
    let outer_path = Path::new(discovered_path.hash_size(), Vec::new()).ok()?;
    let path_plaintext = PathPlaintext {
        path: discovered_path,
        extra_type: Some(PayloadKind::Response),
        extra_payload: plaintext.to_vec(),
    }
    .encode()
    .ok()?;
    let (mac, ciphertext) = encrypt_payload(shared_secret, &path_plaintext)?;

    Some(Packet {
        route_type: RouteType::Flood,
        transport_codes: None,
        path: RoutePath::Normal(outer_path),
        payload: Payload::Path(DirectEncryptedPayload {
            destination_hash: requester_public_key[0],
            source_hash: responder_public_key[0],
            mac,
            ciphertext,
        }),
    })
}

enum PayloadKindForEncoding {
    Response,
    TextMessage,
}

fn encode_direct_payload(
    kind: PayloadKindForEncoding,
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    plaintext: &[u8],
    reply_route: ReplyRoute,
) -> Option<Vec<u8>> {
    let (mac, ciphertext) = encrypt_payload(shared_secret, plaintext)?;
    let payload = DirectEncryptedPayload {
        destination_hash: requester_public_key[0],
        source_hash: responder_public_key[0],
        mac,
        ciphertext,
    };

    let (route_type, path) = match reply_route {
        ReplyRoute::Direct(path) => (RouteType::Direct, path),
        ReplyRoute::Flood(hash_size) => (RouteType::Flood, Path::new(hash_size, Vec::new()).ok()?),
    };
    Packet {
        route_type,
        transport_codes: None,
        path: RoutePath::Normal(path),
        payload: match kind {
            PayloadKindForEncoding::Response => Payload::Response(payload),
            PayloadKindForEncoding::TextMessage => Payload::TextMessage(payload),
        },
    }
    .encode()
    .ok()
}

fn response_nonce<P: Platform, S: Storage>(
    context: &AppContext<P, S>,
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
) -> [u8; 4] {
    let Ok(mut mac) = <HmacSha256 as KeyInit>::new_from_slice(shared_secret) else {
        return context.platform.now_seconds().to_le_bytes();
    };

    mac.update(&context.platform.now_millis().to_le_bytes());
    mac.update(requester_public_key);
    mac.update(responder_public_key);

    let digest = mac.finalize().into_bytes();
    [digest[0], digest[1], digest[2], digest[3]]
}

fn encrypt_payload(shared_secret: &[u8; 32], plaintext: &[u8]) -> Option<([u8; 2], Vec<u8>)> {
    let padded_len = plaintext.len().next_multiple_of(16);
    let mut ciphertext = Vec::with_capacity(padded_len);
    ciphertext.extend_from_slice(plaintext);
    ciphertext.resize(padded_len, 0);

    let cipher = Aes128::new_from_slice(&shared_secret[..16]).ok()?;
    for block in ciphertext.as_chunks_mut::<16>().0 {
        cipher.encrypt_block(block.into());
    }

    Some((mac_for_ciphertext(shared_secret, &ciphertext)?, ciphertext))
}

fn decrypt_payload(shared_secret: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
    let mut plaintext = ciphertext.to_vec();
    let cipher = Aes128::new_from_slice(&shared_secret[..16]).ok()?;

    for block in plaintext.as_chunks_mut::<16>().0 {
        cipher.decrypt_block(block.into());
    }

    Some(plaintext)
}

fn verify_mac(shared_secret: &[u8; 32], payload: &AnonymousRequestPayload) -> bool {
    mac_for_ciphertext(shared_secret, &payload.ciphertext).is_some_and(|mac| mac == payload.mac)
}

fn verify_direct_mac(shared_secret: &[u8; 32], payload: &DirectEncryptedPayload) -> bool {
    mac_for_ciphertext(shared_secret, &payload.ciphertext).is_some_and(|mac| mac == payload.mac)
}

fn mac_for_ciphertext(shared_secret: &[u8; 32], ciphertext: &[u8]) -> Option<[u8; 2]> {
    let Ok(mut mac) = <HmacSha256 as KeyInit>::new_from_slice(shared_secret) else {
        return None;
    };
    mac.update(ciphertext);
    let digest = mac.finalize().into_bytes();
    Some([digest[0], digest[1]])
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::{super::identity::PrivateKey, *};

    struct TestPlatform;
    struct TestDelay;

    impl embedded_hal_async::delay::DelayNs for TestDelay {
        async fn delay_ns(&mut self, _: u32) {}
    }

    impl Platform for TestPlatform {
        type Delay = TestDelay;
        fn model(&self) -> &'static str {
            "test"
        }
        fn memory_profile(&self) -> crate::memory::MemoryProfile {
            crate::memory::MemoryProfile::new(4096, 1, 1, 1, 1)
        }
        fn identity_seed(&self) -> [u8; 32] {
            [1; 32]
        }
        fn delay(&self) -> TestDelay {
            TestDelay
        }
        fn now_millis(&self) -> u64 {
            100
        }
        fn now_seconds(&self) -> u32 {
            100
        }
        fn set_wall_clock_if_forward(&self, _: u32) -> bool {
            false
        }
        fn random_u32(&self) -> u32 {
            1
        }
        fn reboot(&self) -> ! {
            panic!("unexpected reboot")
        }
        fn start_ota(&self) {}
        fn stop_ota(&self) {}
        fn ota_status(&self) -> crate::platform::OtaStatus {
            crate::platform::OtaStatus::Stopped
        }
    }

    fn ready<T>(future: impl core::future::Future<Output = T>) -> T {
        use core::task::{Context, Poll, Waker};
        match core::pin::pin!(future)
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("uncontended test context should not suspend"),
        }
    }

    fn login_request(
        server: &Identity,
        sender: &Identity,
        route_type: RouteType,
        path: Path,
        timestamp: u32,
    ) -> Packet {
        let secret = sender
            .shared_secret_with_ed25519_public(server.public_key())
            .unwrap();
        let mut plaintext = timestamp.to_le_bytes().to_vec();
        plaintext.extend_from_slice(b"meshcore");
        let (mac, ciphertext) = encrypt_payload(&secret, &plaintext).unwrap();
        Packet {
            route_type,
            transport_codes: None,
            path: RoutePath::Normal(path),
            payload: Payload::AnonymousRequest(AnonymousRequestPayload {
                destination_hash: server.public_key()[0],
                sender_pubkey: *sender.public_key(),
                mac,
                ciphertext,
            }),
        }
    }

    #[test]
    fn known_zero_hop_return_route_is_retained_when_floods_are_denied() {
        for scoped in [false, true] {
            for hash_size in [
                mcrs_protocol::HashSize::One,
                mcrs_protocol::HashSize::Two,
                mcrs_protocol::HashSize::Three,
            ] {
                let mut config = super::super::config::AppConfig::generated_defaults([1; 32]);
                config.set_region_flood_allowed("*", false).unwrap();
                config.put_region("home").unwrap();
                config.set_default_region(Some("home")).unwrap();
                config.set_region_flood_allowed("home", false).unwrap();
                let server = Identity::from_private_key(PrivateKey::Seed([1; 32]));
                let sender = Identity::from_private_key(PrivateKey::Seed([2; 32]));
                let secret = sender
                    .shared_secret_with_ed25519_public(server.public_key())
                    .unwrap();
                let path = Path::new(hash_size, vec![]).unwrap();
                let mut request =
                    login_request(&server, &sender, RouteType::Direct, path.clone(), 10);
                if scoped {
                    let region = config.regions().default_region().unwrap();
                    config.regions().apply_scope(&mut request, &region).unwrap();
                    request.route_type = RouteType::TransportDirect;
                }
                let context = AppContext::new(TestPlatform, config, crate::platform::NoStorage);
                // A known zero-hop path remains direct regardless of flood policy.
                assert!(ready(context.authenticate_remote_login(
                    sender.public_key(),
                    &secret,
                    super::super::remote::RemotePrivilege::Admin,
                    9,
                    100,
                    &ReplyRoute::Direct(path.clone()),
                )));
                for timestamp in [10u32, 12] {
                    if timestamp == 12 {
                        assert!(ready(context.authenticate_remote_login(
                            sender.public_key(),
                            &secret,
                            super::super::remote::RemotePrivilege::Admin,
                            11,
                            100,
                            &ReplyRoute::Direct(path.clone()),
                        )));
                        request = login_request(
                            &server,
                            &sender,
                            RouteType::Direct,
                            path.clone(),
                            timestamp,
                        );
                    }
                    let reply =
                        ready(super::super::cli::handle_remote_packet(&request, &context)).unwrap();
                    let reply = Packet::decode(&reply).unwrap();
                    assert_eq!(reply.route_type, RouteType::Direct);
                    assert_eq!(reply.normal_path(), Some(&path));
                    let Payload::Response(payload) = reply.payload else {
                        panic!("expected login response")
                    };
                    assert_eq!(decrypt_payload(&secret, &payload.ciphertext).unwrap()[4], 0);
                    assert_eq!(
                        ready(context.remote_reply_route_for(sender.public_key(), 100)),
                        Some(ReplyRoute::Direct(path.clone()))
                    );
                }
                let plaintext = TextMessagePlaintext {
                    timestamp: 13,
                    text_type: TextType::CliData,
                    attempt: 0,
                    message: b"get name".to_vec(),
                }
                .encode()
                .unwrap();
                let (mac, ciphertext) = encrypt_payload(&secret, &plaintext).unwrap();
                let request = Packet {
                    route_type: RouteType::Direct,
                    transport_codes: None,
                    path: RoutePath::Normal(path.clone()),
                    payload: Payload::TextMessage(DirectEncryptedPayload {
                        destination_hash: server.public_key()[0],
                        source_hash: sender.public_key()[0],
                        mac,
                        ciphertext,
                    }),
                };
                let reply =
                    ready(super::super::cli::handle_remote_packet(&request, &context)).unwrap();
                let reply = Packet::decode(&reply).unwrap();
                assert_eq!(reply.route_type, RouteType::Direct);
                assert_eq!(reply.normal_path(), Some(&path));
                assert!(matches!(reply.payload, Payload::TextMessage(_)));
            }
        }
    }

    #[test]
    fn direct_login_without_a_return_route_floods_on_default_region() {
        for hash_size in [
            mcrs_protocol::HashSize::One,
            mcrs_protocol::HashSize::Two,
            mcrs_protocol::HashSize::Three,
        ] {
            let mut config = super::super::config::AppConfig::generated_defaults([1; 32]);
            config.set_region_flood_allowed("*", false).unwrap();
            config.put_region("home").unwrap();
            config.set_default_region(Some("home")).unwrap();
            let sender = Identity::from_private_key(PrivateKey::Seed([2; 32]));
            // All forward hops have already been consumed on arrival.
            let request = login_request(
                config.identity(),
                &sender,
                RouteType::Direct,
                Path::new(hash_size, vec![]).unwrap(),
                10,
            );
            let context = AppContext::new(TestPlatform, config, crate::platform::NoStorage);
            let reply = ready(super::super::cli::handle_remote_packet(&request, &context)).unwrap();
            let reply = Packet::decode(&reply).unwrap();
            assert_eq!(reply.route_type, RouteType::TransportFlood);
            assert_eq!(
                ready(context.with_config(|config| config.regions().match_flood_region(&reply)))
                    .unwrap()
                    .name,
                "home"
            );
            assert_eq!(reply.normal_path().unwrap().hash_size(), hash_size);
            assert!(matches!(reply.payload, Payload::Response(_)));
            assert_eq!(
                ready(context.remote_reply_route_for(sender.public_key(), 100)),
                Some(ReplyRoute::Flood(hash_size))
            );
        }
    }

    #[test]
    fn direct_login_without_a_return_route_floods_unscoped_when_no_default_exists() {
        for hash_size in [
            mcrs_protocol::HashSize::One,
            mcrs_protocol::HashSize::Two,
            mcrs_protocol::HashSize::Three,
        ] {
            let mut config = super::super::config::AppConfig::generated_defaults([1; 32]);
            config.set_region_flood_allowed("*", false).unwrap();
            config.put_region("home").unwrap();
            // A named region exists, but none is selected as the default.
            let sender = Identity::from_private_key(PrivateKey::Seed([2; 32]));
            // All forward hops have already been consumed on arrival.
            let request = login_request(
                config.identity(),
                &sender,
                RouteType::Direct,
                Path::new(hash_size, vec![]).unwrap(),
                10,
            );
            let context = AppContext::new(TestPlatform, config, crate::platform::NoStorage);
            let reply = ready(super::super::cli::handle_remote_packet(&request, &context)).unwrap();
            let reply = Packet::decode(&reply).unwrap();
            assert_eq!(reply.route_type, RouteType::Flood);
            assert!(reply.transport_codes.is_none());
            assert_eq!(reply.normal_path().unwrap().hash_size(), hash_size);
            assert!(matches!(reply.payload, Payload::Response(_)));
            assert_eq!(
                ready(context.remote_reply_route_for(sender.public_key(), 100)),
                Some(ReplyRoute::Flood(hash_size))
            );
        }
    }

    #[test]
    fn flood_login_learns_authenticated_return_paths_and_preserves_them_on_direct_relogin() {
        for hash_size in [
            mcrs_protocol::HashSize::One,
            mcrs_protocol::HashSize::Two,
            mcrs_protocol::HashSize::Three,
        ] {
            let config = super::super::config::AppConfig::generated_defaults([1; 32]);
            let sender = Identity::from_private_key(PrivateKey::Seed([2; 32]));
            let server = Identity::from_private_key(PrivateKey::Seed([1; 32]));
            let secret = sender
                .shared_secret_with_ed25519_public(server.public_key())
                .unwrap();
            let forward = Path::new(hash_size, vec![0x11; hash_size.size() * 2]).unwrap();
            let request = login_request(&server, &sender, RouteType::Flood, forward.clone(), 10);
            let context = AppContext::new(TestPlatform, config, crate::platform::NoStorage);
            let reply = ready(super::super::cli::handle_remote_packet(&request, &context)).unwrap();
            let reply = Packet::decode(&reply).unwrap();
            assert_eq!(reply.route_type, RouteType::Flood);
            assert_eq!(reply.normal_path().unwrap().hash_size(), hash_size);
            let Payload::Path(payload) = reply.payload else {
                panic!("expected path return")
            };
            let plaintext = decrypt_payload(&secret, &payload.ciphertext).unwrap();
            let wrapped = PathPlaintext::decode(&plaintext).unwrap();
            assert_eq!(wrapped.path, forward);
            assert_eq!(wrapped.extra_type, Some(PayloadKind::Response));

            // The companion supplies its actual return route, which may differ from
            // the reversed incoming path. Keep binary zero bytes and the legacy
            // zero extra-type marker; trimming AES padding before parsing corrupts it.
            let mut bytes = vec![0x77; hash_size.size() * 2];
            *bytes.last_mut().unwrap() = 0;
            let return_path = Path::new(hash_size, bytes).unwrap();
            let mut plaintext = vec![return_path.encoded_length_byte().unwrap()];
            plaintext.extend_from_slice(return_path.bytes());
            plaintext.push(0);
            let (mac, ciphertext) = encrypt_payload(&secret, &plaintext).unwrap();
            let mut path_packet = Packet {
                route_type: RouteType::Direct,
                transport_codes: None,
                path: RoutePath::Normal(Path::new(hash_size, vec![]).unwrap()),
                payload: Payload::Path(DirectEncryptedPayload {
                    destination_hash: server.public_key()[0],
                    source_hash: sender.public_key()[0],
                    mac,
                    ciphertext,
                }),
            };
            // Unauthenticated PATH packets must not change a session's route.
            if let Payload::Path(payload) = &mut path_packet.payload {
                payload.mac[0] ^= 1;
            }
            ready(super::super::cli::handle_remote_packet(
                &path_packet,
                &context,
            ));
            assert_eq!(
                ready(context.remote_reply_route_for(sender.public_key(), 100)),
                Some(ReplyRoute::Direct(forward.reversed()))
            );
            if let Payload::Path(payload) = &mut path_packet.payload {
                payload.mac[0] ^= 1;
            }
            ready(super::super::cli::handle_remote_packet(
                &path_packet,
                &context,
            ));
            assert_eq!(
                ready(context.remote_reply_route_for(sender.public_key(), 100)),
                Some(ReplyRoute::Direct(return_path.clone()))
            );

            // A learned/configured return route must still win when floods
            // become unavailable, rather than being replaced by zero-hop.
            ready(async {
                context
                    .config
                    .lock()
                    .await
                    .set_region_flood_allowed("*", false)
                    .unwrap();
            });
            let request = login_request(
                &server,
                &sender,
                RouteType::Direct,
                Path::new(hash_size, vec![]).unwrap(),
                11,
            );
            let reply = ready(super::super::cli::handle_remote_packet(&request, &context)).unwrap();
            let reply = Packet::decode(&reply).unwrap();
            assert_eq!(reply.route_type, RouteType::Direct);
            assert_eq!(reply.normal_path(), Some(&return_path));
            assert_eq!(
                ready(context.remote_reply_route_for(sender.public_key(), 100)),
                Some(ReplyRoute::Direct(return_path))
            );
            assert!(ready(super::super::cli::handle_remote_packet(&request, &context)).is_none());
        }
    }

    #[test]
    fn flooded_management_replies_keep_incoming_region_instead_of_default() {
        for route in [RouteType::Flood, RouteType::Direct] {
            let mut config = super::super::config::AppConfig::generated_defaults([1; 32]);
            config.put_region("home").unwrap();
            config.set_default_region(Some("home")).unwrap();
            config.put_region("incoming").unwrap();
            config.set_region_flood_allowed("incoming", true).unwrap();
            let region = config.regions().find_by_name_prefix("incoming").unwrap();
            let sender = Identity::from_private_key(PrivateKey::Seed([2; 32]));
            let server = Identity::from_private_key(PrivateKey::Seed([1; 32]));
            let secret = sender
                .shared_secret_with_ed25519_public(server.public_key())
                .unwrap();
            let path = Path::new(mcrs_protocol::HashSize::Three, vec![]).unwrap();
            let mut login = login_request(&server, &sender, RouteType::Direct, path.clone(), 10);
            config.regions().apply_scope(&mut login, &region).unwrap();
            login.route_type = RouteType::TransportDirect;
            let context = AppContext::new(TestPlatform, config, crate::platform::NoStorage);
            let check = |request: &Packet| {
                let reply =
                    ready(super::super::cli::handle_remote_packet(request, &context)).unwrap();
                let reply = Packet::decode(&reply).unwrap();
                assert_eq!(reply.route_type, RouteType::TransportFlood);
                assert_eq!(reply.normal_path().unwrap().hash_size(), path.hash_size());
                let matched = ready(
                    context.with_config(|config| config.regions().match_flood_region(&reply)),
                )
                .unwrap();
                assert_eq!(matched.name, region.name);
            };
            check(&login);
            for (timestamp, is_text) in [(11, false), (12, true)] {
                let plaintext = if is_text {
                    TextMessagePlaintext {
                        timestamp,
                        text_type: TextType::CliData,
                        attempt: 0,
                        message: b"get name".to_vec(),
                    }
                    .encode()
                    .unwrap()
                } else {
                    mcrs_protocol::RequestPlaintext {
                        timestamp,
                        request_data: vec![0x03],
                    }
                    .encode()
                };
                let (mac, ciphertext) = encrypt_payload(&secret, &plaintext).unwrap();
                let payload = DirectEncryptedPayload {
                    destination_hash: server.public_key()[0],
                    source_hash: sender.public_key()[0],
                    mac,
                    ciphertext,
                };
                let mut request = Packet {
                    route_type: route,
                    transport_codes: None,
                    path: RoutePath::Normal(path.clone()),
                    payload: if is_text {
                        Payload::TextMessage(payload)
                    } else {
                        Payload::Request(payload)
                    },
                };
                ready(
                    context
                        .with_config(|config| config.regions().apply_scope(&mut request, &region)),
                )
                .unwrap();
                if route.is_direct() {
                    request.route_type = RouteType::TransportDirect;
                }
                check(&request);
            }
        }
    }

    #[test]
    fn flood_login_path_return_uses_incoming_region() {
        let mut config = super::super::config::AppConfig::generated_defaults([1; 32]);
        config.put_region("home").unwrap();
        config.set_default_region(Some("home")).unwrap();
        config.put_region("incoming").unwrap();
        config.set_region_flood_allowed("incoming", true).unwrap();
        let region = config.regions().find_by_name_prefix("incoming").unwrap();
        let sender = Identity::from_private_key(PrivateKey::Seed([2; 32]));
        let path = Path::new(mcrs_protocol::HashSize::Three, vec![0x11, 0x22, 0x33]).unwrap();
        let mut request = login_request(
            config.identity(),
            &sender,
            RouteType::Flood,
            path.clone(),
            10,
        );
        config.regions().apply_scope(&mut request, &region).unwrap();
        let context = AppContext::new(TestPlatform, config, crate::platform::NoStorage);
        let reply = ready(super::super::cli::handle_remote_packet(&request, &context)).unwrap();
        let reply = Packet::decode(&reply).unwrap();
        assert!(matches!(reply.payload, Payload::Path(_)));
        assert_eq!(reply.normal_path().unwrap().hash_size(), path.hash_size());
        assert_eq!(
            ready(context.with_config(|config| config.regions().match_flood_region(&reply)))
                .unwrap()
                .name,
            region.name
        );
    }

    #[test]
    fn login_in_unknown_or_denied_region_does_not_fall_back_to_default() {
        for known in [false, true] {
            let mut config = super::super::config::AppConfig::generated_defaults([1; 32]);
            config.put_region("home").unwrap();
            config.set_default_region(Some("home")).unwrap();
            let mut sender_config = super::super::config::AppConfig::generated_defaults([2; 32]);
            sender_config.put_region("incoming").unwrap();
            let region = sender_config
                .regions()
                .find_by_name_prefix("incoming")
                .unwrap();
            if known {
                config.put_region("incoming").unwrap();
                config.set_region_flood_allowed("incoming", false).unwrap();
            }
            let mut request = login_request(
                config.identity(),
                sender_config.identity(),
                RouteType::Flood,
                Path::empty(),
                10,
            );
            sender_config
                .regions()
                .apply_scope(&mut request, &region)
                .unwrap();
            let context = AppContext::new(TestPlatform, config, crate::platform::NoStorage);
            assert!(ready(super::super::cli::handle_remote_packet(&request, &context)).is_none());
            assert!(
                ready(context.remote_privilege_for(sender_config.identity().public_key(), 100))
                    .is_none()
            );
        }
    }

    #[test]
    fn encrypted_payload_matches_aes_and_hmac_vectors() {
        let shared_secret = core::array::from_fn(|i| i as u8);
        let plaintext = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        // First ciphertext is the FIPS 197 AES-128 example. The padded case
        // was checked with OpenSSL; MACs with Python's hmac/sha256.
        for (plain, expected_ciphertext, expected_mac) in [
            (
                plaintext.as_slice(),
                "69c4e0d86a7b0430d8cdb78070b4c55a",
                [0xb7, 0x18],
            ),
            (
                b"abc".as_slice(),
                "7516b2e97d7ecdc3ffd9c47b69c29174",
                [0xc7, 0xc2],
            ),
        ] {
            let (mac, ciphertext) = encrypt_payload(&shared_secret, plain).unwrap();
            let hex: alloc::string::String = ciphertext
                .iter()
                .map(|byte| alloc::format!("{byte:02x}"))
                .collect();
            assert_eq!(hex, expected_ciphertext);
            assert_eq!(mac, expected_mac);
            let decrypted = decrypt_payload(&shared_secret, &ciphertext).unwrap();
            assert_eq!(&decrypted[..plain.len()], plain);
            assert!(decrypted[plain.len()..].iter().all(|byte| *byte == 0));
        }
    }

    #[test]
    fn authenticated_direct_decrypt_rejects_unaligned_ciphertext_before_aes() {
        let identity = Identity::from_private_key(PrivateKey::Seed([1; 32]));
        let payload = DirectEncryptedPayload {
            destination_hash: identity.public_key()[0],
            source_hash: 0,
            mac: [0; 2],
            ciphertext: vec![0],
        };

        assert!(decrypt_authenticated_direct_payload(&payload, &identity, &[]).is_none());
    }

    #[test]
    fn anonymous_decrypt_rejects_unaligned_ciphertext_before_aes() {
        let identity = Identity::from_private_key(PrivateKey::Seed([1; 32]));
        let payload = AnonymousRequestPayload {
            destination_hash: identity.public_key()[0],
            sender_pubkey: [2; 32],
            mac: [0; 2],
            ciphertext: vec![0],
        };

        assert!(decrypt_anonymous_request(&payload, &identity).is_none());
    }

    #[test]
    fn path_response_wraps_response_and_preserves_discovered_path() {
        let shared_secret = [7; 32];
        let requester = [8; 32];
        let responder = [9; 32];
        let discovered_path =
            Path::new(mcrs_protocol::HashSize::Two, vec![0x11, 0x22, 0x33, 0x44]).unwrap();
        let packet = encode_path_response_packet(
            &shared_secret,
            &requester,
            &responder,
            b"reply",
            discovered_path.clone(),
        )
        .unwrap();

        assert_eq!(packet.route_type, RouteType::Flood);
        assert_eq!(
            packet.normal_path().unwrap().hash_size(),
            mcrs_protocol::HashSize::Two
        );
        assert_eq!(packet.normal_path().unwrap().hop_count(), 0);
        let Payload::Path(payload) = packet.payload else {
            panic!("expected PATH payload");
        };
        let decrypted = decrypt_payload(&shared_secret, &payload.ciphertext).unwrap();
        let unpadded_len = decrypted
            .iter()
            .rposition(|byte| *byte != 0)
            .map_or(0, |i| i + 1);
        let wrapped = PathPlaintext::decode(&decrypted[..unpadded_len]).unwrap();
        assert_eq!(wrapped.path, discovered_path);
        assert_eq!(wrapped.extra_type, Some(PayloadKind::Response));
        assert_eq!(wrapped.extra_payload, b"reply");
    }
}
