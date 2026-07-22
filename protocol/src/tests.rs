use alloc::{string::String, vec, vec::Vec};

use crate::*;
use ed25519_dalek::{Signer, SigningKey};

#[test]
fn header_round_trips() {
    let header = Header {
        payload_version: 1,
        payload_kind: PayloadKind::Advert,
        route_type: RouteType::Flood,
    };
    let encoded = header.encode().unwrap();
    assert_eq!(encoded, 0x11);
    assert_eq!(Header::decode(encoded).unwrap(), header);
    assert_eq!(Header::decode(0xff), Err(Error::InvalidHeaderSentinel));
}

#[test]
fn normal_packet_round_trips_with_transport_codes_and_path() {
    let packet = Packet {
        route_type: RouteType::TransportDirect,
        transport_codes: Some(TransportCodes {
            primary: 0x1234,
            secondary: 0,
        }),
        path: RoutePath::Normal(
            Path::from_hashes(HashSize::Two, &[&[0xaa, 0xbb], &[0xcc, 0xdd]]).unwrap(),
        ),
        payload: Payload::Ack(AckPayload {
            ack_hash: [1, 2, 3, 4],
        }),
    };

    let encoded = packet.encode().unwrap();
    assert_eq!(
        encoded,
        vec![
            0x0f, 0x34, 0x12, 0x00, 0x00, 0x42, 0xaa, 0xbb, 0xcc, 0xdd, 1, 2, 3, 4
        ]
    );
    assert_eq!(Packet::decode(&encoded).unwrap(), packet);
}

#[test]
fn rejects_invalid_normal_path_hash_size_code() {
    let bytes = [0x11, 0xc0, 1, 2, 3, 4];
    assert_eq!(Packet::decode(&bytes), Err(Error::InvalidHashSizeCode(3)));
    assert_eq!(HashSize::from_code(4), Err(Error::InvalidHashSizeCode(4)));
}

#[test]
fn trace_packet_uses_snr_header_path_and_payload_hashes() {
    let packet = Packet {
        route_type: RouteType::Direct,
        transport_codes: None,
        path: RoutePath::Trace(TracePath::new(vec![-4, 12]).unwrap()),
        payload: Payload::Trace(TracePayload {
            tag: 0x01020304,
            auth_code: 0xa0b0c0d0,
            hash_size: TraceHashSize::Two,
            path_hashes: vec![0xaa, 0xbb, 0xcc, 0xdd],
        }),
    };

    let encoded = packet.encode().unwrap();
    assert_eq!(
        encoded,
        vec![
            0x26, 0x02, 0xfc, 0x0c, 0x04, 0x03, 0x02, 0x01, 0xd0, 0xc0, 0xb0, 0xa0, 0x01, 0xaa,
            0xbb, 0xcc, 0xdd
        ]
    );
    assert_eq!(Packet::decode(&encoded).unwrap(), packet);
}

#[test]
fn advert_app_data_round_trips() {
    let app_data = AdvertAppData {
        node_type: AdvertNodeType::Repeater,
        location: Some((49_123_456, -2_123_456)),
        feature1: Some(0x0102),
        feature2: None,
        name: Some(String::from("R1")),
    };
    let encoded = app_data.encode().unwrap();
    assert_eq!(AdvertAppData::decode(&encoded).unwrap(), app_data);
}

#[test]
fn advert_app_data_rejects_truncated_overlong_and_trailing_fields() {
    assert_eq!(
        AdvertAppData::decode(&[0x10, 1, 2, 3, 4, 5, 6, 7]),
        Err(Error::Truncated("advert longitude"))
    );
    assert_eq!(
        AdvertAppData::decode(&[0x02, 0xaa]),
        Err(Error::InvalidLength("advert app_data"))
    );
    assert_eq!(
        AdvertAppData::decode(&[0x80, 0xff]),
        Err(Error::InvalidUtf8)
    );

    let overlong = [0x80; MAX_ADVERT_DATA_SIZE + 1];
    assert_eq!(
        AdvertAppData::decode(&overlong),
        Err(Error::InvalidLength("advert app_data"))
    );
}

#[test]
fn advert_payload_rejects_overlong_app_data_instead_of_truncating() {
    let input = vec![0; PUB_KEY_SIZE + 4 + SIGNATURE_SIZE + MAX_ADVERT_DATA_SIZE + 1];
    assert_eq!(
        AdvertPayload::decode(&input),
        Err(Error::InvalidLength("advert app_data"))
    );
}

#[test]
fn public_payload_decoders_reject_oversized_input() {
    let input = vec![0; MAX_PACKET_PAYLOAD + 1];
    let expected = Some(Error::PayloadTooLong { len: input.len() });

    assert_eq!(
        Payload::decode(PayloadKind::RawCustom, &input).err(),
        expected
    );
    assert_eq!(DirectEncryptedPayload::decode(&input).err(), expected);
    assert_eq!(AnonymousRequestPayload::decode(&input).err(), expected);
    assert_eq!(GroupEncryptedPayload::decode(&input).err(), expected);
    assert_eq!(ControlPayload::decode(&input).err(), expected);
    assert_eq!(MultipartPayload::decode(&input).err(), expected);
    assert_eq!(TracePayload::decode(&input).err(), expected);
    assert_eq!(RequestPlaintext::decode(&input).err(), expected);
    assert_eq!(RepeaterResponsePlaintext::decode(&input).err(), expected);
    assert_eq!(TextMessagePlaintext::decode(&input).err(), expected);
    assert_eq!(PathPlaintext::decode(&input).err(), expected);
}

#[test]
fn advert_payload_verifies_signature_over_public_key_timestamp_and_app_data() {
    let signing_key = SigningKey::from_bytes(&[7; SEED_SIZE]);
    let public_key = signing_key.verifying_key().to_bytes();
    let timestamp: u32 = 0x11223344;
    let app_data = AdvertAppData {
        node_type: AdvertNodeType::Repeater,
        location: None,
        feature1: None,
        feature2: None,
        name: Some(String::from("R1")),
    };

    let mut signed_message = Vec::new();
    signed_message.extend_from_slice(&public_key);
    signed_message.extend_from_slice(&timestamp.to_le_bytes());
    signed_message.extend_from_slice(&app_data.encode().unwrap());

    let advert = AdvertPayload {
        public_key,
        timestamp,
        signature: signing_key.sign(&signed_message).to_bytes(),
        app_data: Some(app_data),
    };
    assert!(advert.verify_signature());

    let mut tampered = advert.clone();
    tampered.timestamp = timestamp.wrapping_add(1);
    assert!(!tampered.verify_signature());
}

#[test]
fn control_discover_request_decodes_to_struct() {
    let payload = ControlPayload::discover_request(0b0000_0100, 0x11223344, Some(0x55667788), true);
    assert_eq!(payload.flags, 0x81);
    assert_eq!(
        payload.message().unwrap(),
        ControlMessage::DiscoverRequest {
            prefix_only: true,
            type_filter: 0b0000_0100,
            tag: 0x11223344,
            since: Some(0x55667788),
        }
    );
}

#[test]
fn zero_hop_control_packets_must_be_direct_zero_hop() {
    let payload = Payload::Control(ControlPayload::discover_request(
        0b0000_0100,
        0x11223344,
        None,
        false,
    ));
    let direct = Packet {
        route_type: RouteType::Direct,
        transport_codes: None,
        path: RoutePath::Normal(Path::empty()),
        payload: payload.clone(),
    };
    assert_eq!(Packet::decode(&direct.encode().unwrap()).unwrap(), direct);

    let flood = Packet {
        route_type: RouteType::Flood,
        transport_codes: None,
        path: RoutePath::Normal(Path::empty()),
        payload,
    };
    assert_eq!(flood.encode(), Err(Error::InvalidZeroHopControlRoute));

    let encoded_flood_control = [0x2d, 0x00, 0x80, 0x04, 0x44, 0x33, 0x22, 0x11];
    assert_eq!(
        Packet::decode(&encoded_flood_control),
        Err(Error::InvalidZeroHopControlRoute)
    );
}

#[test]
fn control_messages_reject_bad_lengths_without_panicking() {
    for len in [0usize, 1, 4, 6, 8, 10, 12, 14, 36, 38] {
        let request = ControlPayload::new(0x08, 0, vec![0; len]);
        if len != 5 && len != 9 {
            assert_eq!(
                request.message(),
                Err(Error::InvalidLength("discover request"))
            );
        }

        let response = ControlPayload::new(0x09, 0, vec![0; len]);
        if len != 13 && len != 37 {
            assert_eq!(
                response.message(),
                Err(Error::InvalidLength("discover response"))
            );
        }
    }
}

#[test]
fn plaintext_helpers_round_trip() {
    let text = TextMessagePlaintext {
        timestamp: 123,
        text_type: TextType::SignedPlain,
        attempt: 2,
        message: b"abcdhello".to_vec(),
    };
    assert_eq!(
        TextMessagePlaintext::decode(&text.encode().unwrap()).unwrap(),
        text
    );

    let reply_path = Path::from_hashes(HashSize::One, &[&[1], &[2], &[3]]).unwrap();
    let req = RepeaterSubRequestPlaintext {
        timestamp: 99,
        req_type: 0x01,
        reply_path,
    };
    assert_eq!(
        RepeaterSubRequestPlaintext::decode(&req.encode().unwrap()).unwrap(),
        req
    );
}

#[test]
fn forwarding_helpers_mutate_paths() {
    let mut packet = Packet {
        route_type: RouteType::Flood,
        transport_codes: None,
        path: RoutePath::Normal(Path::empty()),
        payload: Payload::RawCustom(vec![1, 2, 3]),
    };

    packet.append_flood_hop(&[0xab]).unwrap();
    packet.append_flood_hop(&[0xcd]).unwrap();
    assert_eq!(packet.normal_path().unwrap().bytes(), &[0xab, 0xcd]);
    assert!(packet.consume_direct_hop(&[0xab]).unwrap());
    assert_eq!(packet.normal_path().unwrap().bytes(), &[0xcd]);
    assert!(!packet.consume_direct_hop(&[0xee]).unwrap());
}

#[test]
fn path_detects_hash_using_path_hash_size() {
    let path = Path::from_hashes(HashSize::Two, &[&[0xab, 0xcd], &[0x12, 0x34]]).unwrap();

    assert!(path.contains_hash(&[0xab, 0xcd, 0xef, 0x01]));
    assert!(path.contains_hash(&[0x12, 0x34]));
    assert!(!path.contains_hash(&[0xab]));
    assert!(!path.contains_hash(&[0xab, 0xce]));
}

#[test]
fn dedup_signature_uses_trace_path_length() {
    let mut packet = Packet {
        route_type: RouteType::Direct,
        transport_codes: None,
        path: RoutePath::Trace(TracePath::new(vec![]).unwrap()),
        payload: Payload::Trace(TracePayload {
            tag: 1,
            auth_code: 2,
            hash_size: TraceHashSize::One,
            path_hashes: vec![0xaa],
        }),
    };
    let before = packet.dedup_signature().unwrap();
    packet.append_trace_snr(8).unwrap();
    let after = packet.dedup_signature().unwrap();
    assert_ne!(before, after);
}

#[test]
fn dedup_signature_distinguishes_route_and_transport_scope() {
    let payload = Payload::RawCustom(vec![1, 2, 3]);
    let unscoped = Packet {
        route_type: RouteType::Flood,
        transport_codes: None,
        path: RoutePath::Normal(Path::empty()),
        payload: payload.clone(),
    };
    let scoped_a = Packet {
        route_type: RouteType::TransportFlood,
        transport_codes: Some(TransportCodes::new(0x1234)),
        path: RoutePath::Normal(Path::empty()),
        payload: payload.clone(),
    };
    let scoped_b = Packet {
        route_type: RouteType::TransportFlood,
        transport_codes: Some(TransportCodes::new(0x5678)),
        path: RoutePath::Normal(Path::empty()),
        payload,
    };

    assert_ne!(
        unscoped.dedup_signature().unwrap(),
        scoped_a.dedup_signature().unwrap()
    );
    assert_ne!(
        scoped_a.dedup_signature().unwrap(),
        scoped_b.dedup_signature().unwrap()
    );
}

#[test]
fn seen_packet_cache_suppresses_until_ttl_expires() {
    let mut cache = SeenPacketCache::new(5);
    let now = 10;
    assert!(cache.check_and_insert([1; 8], now));
    assert!(!cache.check_and_insert([1; 8], now + 1));
    assert!(cache.check_and_insert([1; 8], now + 6));
}

#[test]
fn seen_packet_cache_drops_oldest_when_capacity_is_reached() {
    let mut cache = SeenPacketCache::new_with_capacity(100, 2);
    assert!(cache.check_and_insert([1; 8], 1));
    assert!(cache.check_and_insert([2; 8], 2));
    assert!(cache.check_and_insert([3; 8], 3));
    assert!(cache.check_and_insert([1; 8], 4));
    assert!(!cache.check_and_insert([3; 8], 5));
}

#[test]
fn seen_packet_cache_touch_restarts_ttl() {
    let mut cache = SeenPacketCache::new(5);
    assert!(cache.check_and_insert([1; 8], 10));
    cache.touch([1; 8], 14);

    assert!(cache.contains([1; 8], 19));
    assert!(!cache.contains([1; 8], 20));
}

#[test]
fn node_hash_is_public_key_prefix() {
    let mut public_key = [0; PUB_KEY_SIZE];
    public_key[0..5].copy_from_slice(&[1, 2, 3, 4, 5]);

    assert_eq!(node_hash::<1>(&public_key), [1]);
    assert_eq!(node_hash::<2>(&public_key), [1, 2]);
    assert_eq!(node_hash::<4>(&public_key), [1, 2, 3, 4]);

    let oversized = node_hash::<34>(&public_key);
    assert_eq!(&oversized[..PUB_KEY_SIZE], &public_key);
    assert_eq!(&oversized[PUB_KEY_SIZE..], &[0, 0]);
}

#[test]
fn malformed_packets_return_errors_instead_of_panicking() {
    let cases: &[&[u8]] = &[
        &[],
        &[0xff],
        &[0x40],
        &[0x00],
        &[0x00, 0x01, 0x02],
        &[0x01],
        &[0x01, 0xc0],
        &[0x01, 0x45, 0xaa],
        &[0x0d, 0x00],
        &[0x11, 0x00, 1, 2, 3],
        &[0x26, 0xc0],
        &[0x26, 0x01],
        &[0x26, 0x00, 1, 2, 3],
        &[0x26, 0x00, 1, 2, 3, 4, 5, 6, 7, 8, 0x03],
    ];

    for case in cases {
        let _ = Packet::decode(case);
    }
}

#[test]
fn packet_decoder_rejects_truncated_frames() {
    // Flood ACK header with no path-length byte.
    assert_eq!(
        Packet::decode(&[0x0d]),
        Err(Error::Truncated("path_length"))
    );

    // Transport ACK header with neither 16-bit transport code present.
    assert_eq!(
        Packet::decode(&[0x0c]),
        Err(Error::Truncated("transport primary"))
    );

    // Two complete transport codes, but no path-length byte after them.
    assert_eq!(
        Packet::decode(&[0x0c, 0, 0, 0, 0]),
        Err(Error::Truncated("path_length"))
    );

    // A normal route declaring one one-byte hash, with no hash byte present.
    assert_eq!(Packet::decode(&[0x0d, 0x01]), Err(Error::Truncated("path")));
}

#[test]
fn packet_decoder_rejects_empty_control_and_trace_payloads() {
    // Direct CONTROL with a zero-hop path but no control flags byte.
    assert_eq!(
        Packet::decode(&[0x2e, 0x00]),
        Err(Error::Truncated("control payload"))
    );

    // Direct TRACE with a zero-length SNR path but none of its nine-byte
    // fixed payload bytes (tag, auth code, and flags).
    assert_eq!(
        Packet::decode(&[0x26, 0x00]),
        Err(Error::Truncated("trace tag"))
    );
}

#[test]
fn path_plaintext_rejects_route_and_extra_type_truncation() {
    // The embedded path claims 63 one-byte hashes, but the plaintext ends
    // immediately after the length byte.
    assert_eq!(
        PathPlaintext::decode(&[0x3f]),
        Err(Error::Truncated("path"))
    );

    // A valid empty embedded path still requires an extra-type byte.
    assert_eq!(
        PathPlaintext::decode(&[0x00]),
        Err(Error::Truncated("path plaintext extra_type"))
    );
}

#[test]
fn direct_ciphertext_requires_nonempty_complete_aes_blocks() {
    let payload = DirectEncryptedPayload {
        destination_hash: 1,
        source_hash: 2,
        mac: [0; CIPHER_MAC_SIZE],
        ciphertext: vec![],
    };
    assert!(!payload.has_complete_ciphertext_blocks());

    let unaligned = DirectEncryptedPayload {
        ciphertext: vec![0; CIPHER_BLOCK_SIZE - 1],
        ..payload.clone()
    };
    assert!(!unaligned.has_complete_ciphertext_blocks());

    let aligned = DirectEncryptedPayload {
        ciphertext: vec![0; CIPHER_BLOCK_SIZE],
        ..payload
    };
    assert!(aligned.has_complete_ciphertext_blocks());
}

#[test]
fn public_trace_helper_rejects_invalid_manual_trace_payload() {
    let packet = Packet {
        route_type: RouteType::Direct,
        transport_codes: None,
        path: RoutePath::Trace(TracePath::new(vec![]).unwrap()),
        payload: Payload::Trace(TracePayload {
            tag: 1,
            auth_code: 2,
            hash_size: TraceHashSize::Two,
            path_hashes: vec![0xaa],
        }),
    };

    assert_eq!(
        packet.trace_next_hop_matches(&[0xaa, 0xbb]),
        Err(Error::InvalidLength("trace path_hashes"))
    );
}

#[test]
fn payload_decoders_reject_truncation() {
    assert!(DirectEncryptedPayload::decode(&[1, 2, 3]).is_err());
    assert!(AnonymousRequestPayload::decode(&[0; 34]).is_err());
    assert!(GroupEncryptedPayload::decode(&[1, 2]).is_err());
    assert!(AckPayload::decode(&[1, 2, 3]).is_err());
    assert!(AdvertPayload::decode(&[0; PUB_KEY_SIZE + 4 + SIGNATURE_SIZE - 1]).is_err());
    assert!(TracePayload::decode(&[0; 8]).is_err());
    assert!(MultipartPayload::decode(&[]).is_err());
    assert!(ControlPayload::decode(&[]).is_err());
    assert!(RequestPlaintext::decode(&[0; 3]).is_err());
    assert!(TextMessagePlaintext::decode(&[0; 4]).is_err());
    assert!(PathPlaintext::decode(&[]).is_err());
    assert!(RepeaterSubRequestPlaintext::decode(&[0; 5]).is_err());
    assert!(RepeaterLoginResponsePlaintext::decode(&[0; 12]).is_err());
    assert!(RepeaterResponsePlaintext::decode(&[0; 7]).is_err());
}

#[test]
fn text_message_rejects_messages_over_the_protocol_limit() {
    let text = TextMessagePlaintext {
        timestamp: 1,
        text_type: TextType::Plain,
        attempt: 0,
        message: vec![0; MAX_TEXT_LEN + 1],
    };

    assert_eq!(text.encode(), Err(Error::InvalidLength("text message")));

    let mut encoded = vec![0; 5];
    encoded.extend_from_slice(&text.message);
    assert_eq!(
        TextMessagePlaintext::decode(&encoded),
        Err(Error::InvalidLength("text message"))
    );
}

#[test]
fn repeater_login_response_roundtrips() {
    let response = RepeaterLoginResponsePlaintext {
        server_timestamp: 0x11223344,
        keep_alive_interval: 0,
        legacy_permissions: 1,
        acl_permissions: PERM_ACL_ADMIN,
        nonce: [1, 2, 3, 4],
        firmware_version_level: 2,
    };

    let encoded = response.encode();

    assert_eq!(encoded.len(), RepeaterLoginResponsePlaintext::ENCODED_LEN);
    assert_eq!(
        &encoded[..5],
        &[0x44, 0x33, 0x22, 0x11, RESP_SERVER_LOGIN_OK]
    );
    assert_eq!(
        RepeaterLoginResponsePlaintext::decode(&encoded),
        Ok(response)
    );
}
