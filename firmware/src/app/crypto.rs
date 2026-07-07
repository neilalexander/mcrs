extern crate alloc;

use aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray},
};
use alloc::vec::Vec;
use hmac::{Hmac, Mac};
use mcrs_protocol::{
    AnonymousRequestPayload, DirectEncryptedPayload, PERM_ACL_ADMIN, Packet, Path, Payload,
    RepeaterLoginResponsePlaintext, RoutePath, RouteType, TextMessagePlaintext, TextType,
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
    let cipher = Aes128::new(GenericArray::from_slice(&shared_secret[..16]));

    for block in plaintext.chunks_exact_mut(16) {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
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
    if payload.ciphertext.is_empty() || !payload.ciphertext.len().is_multiple_of(16) {
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
            plaintext,
        });
    }

    None
}

pub fn encode_zero_hop_login_response(
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    acl_permissions: u8,
) -> Option<Vec<u8>> {
    let response = RepeaterLoginResponsePlaintext {
        server_timestamp: crate::platform::now_seconds(),
        keep_alive_interval: 0,
        legacy_permissions: u8::from(acl_permissions == PERM_ACL_ADMIN),
        acl_permissions,
        nonce: response_nonce(shared_secret, requester_public_key, responder_public_key),
        firmware_version_level: 2,
    };

    encode_zero_hop_response_plaintext(
        shared_secret,
        requester_public_key,
        responder_public_key,
        &response.encode(),
    )
}

pub fn encode_zero_hop_cli_text_response(
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    request_timestamp: u32,
    message: &[u8],
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
    };

    encode_zero_hop_direct_payload(
        PayloadKindForEncoding::TextMessage,
        shared_secret,
        requester_public_key,
        responder_public_key,
        &plaintext.encode(),
    )
}

pub fn encode_zero_hop_response_plaintext(
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    plaintext: &[u8],
) -> Option<Vec<u8>> {
    encode_zero_hop_direct_payload(
        PayloadKindForEncoding::Response,
        shared_secret,
        requester_public_key,
        responder_public_key,
        plaintext,
    )
}

enum PayloadKindForEncoding {
    Response,
    TextMessage,
}

fn encode_zero_hop_direct_payload(
    kind: PayloadKindForEncoding,
    shared_secret: &[u8; 32],
    requester_public_key: &[u8; 32],
    responder_public_key: &[u8; 32],
    plaintext: &[u8],
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
        path: RoutePath::Normal(Path::empty()),
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
    let Ok(mut mac) = <HmacSha256 as Mac>::new_from_slice(shared_secret) else {
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

    let cipher = Aes128::new(GenericArray::from_slice(&shared_secret[..16]));
    for block in ciphertext.chunks_exact_mut(16) {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }

    Some((mac_for_ciphertext(shared_secret, &ciphertext)?, ciphertext))
}

fn decrypt_payload(shared_secret: &[u8; 32], ciphertext: &[u8]) -> Option<Vec<u8>> {
    let mut plaintext = ciphertext.to_vec();
    let cipher = Aes128::new(GenericArray::from_slice(&shared_secret[..16]));

    for block in plaintext.chunks_exact_mut(16) {
        cipher.decrypt_block(GenericArray::from_mut_slice(block));
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
    let Ok(mut mac) = <HmacSha256 as Mac>::new_from_slice(shared_secret) else {
        return None;
    };
    mac.update(ciphertext);
    let digest = mac.finalize().into_bytes();
    Some([digest[0], digest[1]])
}
