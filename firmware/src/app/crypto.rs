extern crate alloc;

use aes::{
    Aes128,
    cipher::{Block, BlockCipherDecrypt, BlockCipherEncrypt, KeyInit},
};
use alloc::vec::Vec;
use hmac::{Hmac, Mac};
use mcrs_protocol::{
    AnonymousRequestPayload, DirectEncryptedPayload, PERM_ACL_ADMIN, Packet, Path, PathPlaintext,
    Payload, PayloadKind, RepeaterLoginResponsePlaintext, RoutePath, RouteType,
    TextMessagePlaintext, TextType,
};
use sha2::Sha256;

use super::identity::Identity;

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
    pub reply_path: Path,
    pub plaintext: Vec<u8>,
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

    let mut plaintext = payload.ciphertext.clone();
    let cipher = Aes128::new_from_slice(&shared_secret[..16]).ok()?;

    for block in plaintext.chunks_exact_mut(16) {
        cipher.decrypt_block(<&mut Block<Aes128>>::try_from(block).ok()?);
    }

    while plaintext.last().copied() == Some(0) {
        plaintext.pop();
    }

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
    if payload.destination_hash != identity.public_key()[0] {
        return None;
    }
    if !payload.has_complete_ciphertext_blocks() {
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
            reply_path: session.reply_path.clone(),
            plaintext,
        });
    }

    None
}

pub fn encode_login_response(
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    acl_permissions: u8,
    reply_path: Path,
) -> Option<Vec<u8>> {
    let response = RepeaterLoginResponsePlaintext {
        server_timestamp: crate::platform::now_seconds(),
        keep_alive_interval: 0,
        legacy_permissions: u8::from(acl_permissions == PERM_ACL_ADMIN),
        acl_permissions,
        nonce: response_nonce(shared_secret, requester_public_key, responder_public_key),
        firmware_version_level: 2,
    };

    encode_response_plaintext(
        shared_secret,
        requester_public_key,
        responder_public_key,
        &response.encode(),
        reply_path,
    )
}

pub fn encode_cli_text_response(
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    request_timestamp: u32,
    message: &[u8],
    reply_path: Path,
) -> Option<Vec<u8>> {
    let mut timestamp = crate::platform::now_seconds();
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
        reply_path,
    )
}

pub fn encode_response_plaintext(
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    plaintext: &[u8],
    reply_path: Path,
) -> Option<Vec<u8>> {
    encode_direct_payload(
        PayloadKindForEncoding::Response,
        shared_secret,
        requester_public_key,
        responder_public_key,
        plaintext,
        reply_path,
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
    reply_path: Path,
) -> Option<Vec<u8>> {
    let (mac, ciphertext) = encrypt_payload(shared_secret, plaintext)?;
    let payload = DirectEncryptedPayload {
        destination_hash: requester_public_key[0],
        source_hash: responder_public_key[0],
        mac,
        ciphertext,
    };

    Packet {
        route_type: RouteType::Direct,
        transport_codes: None,
        path: RoutePath::Normal(reply_path),
        payload: match kind {
            PayloadKindForEncoding::Response => Payload::Response(payload),
            PayloadKindForEncoding::TextMessage => Payload::TextMessage(payload),
        },
    }
    .encode()
    .ok()
}

fn response_nonce(
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
) -> [u8; 4] {
    let Ok(mut mac) = <HmacSha256 as KeyInit>::new_from_slice(shared_secret) else {
        return crate::platform::now_seconds().to_le_bytes();
    };

    mac.update(&crate::platform::now_millis().to_le_bytes());
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
    for block in ciphertext.chunks_exact_mut(16) {
        cipher.encrypt_block(<&mut Block<Aes128>>::try_from(block).ok()?);
    }

    Some((mac_for_ciphertext(shared_secret, &ciphertext)?, ciphertext))
}

fn decrypt_payload(shared_secret: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
    let mut plaintext = ciphertext.to_vec();
    let cipher = Aes128::new_from_slice(&shared_secret[..16]).ok()?;

    for block in plaintext.chunks_exact_mut(16) {
        cipher.decrypt_block(<&mut Block<Aes128>>::try_from(block).ok()?);
    }

    while plaintext.last().copied() == Some(0) {
        plaintext.pop();
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

    use super::super::identity::PrivateKey;
    use super::*;

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
            assert_eq!(decrypt_payload(&shared_secret, &ciphertext).unwrap(), plain);
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
        let wrapped = PathPlaintext::decode(&decrypted).unwrap();
        assert_eq!(wrapped.path, discovered_path);
        assert_eq!(wrapped.extra_type, Some(PayloadKind::Response));
        assert_eq!(wrapped.extra_payload, b"reply");
    }
}
