use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::{CIPHER_KEY_SIZE, PUB_KEY_SIZE, PayloadKind};

type HmacSha256 = Hmac<Sha256>;

pub fn derive_transport_code(
    transport_key: &[u8; CIPHER_KEY_SIZE],
    payload_kind: PayloadKind,
    payload_bytes: &[u8],
) -> u16 {
    let Ok(mut mac) = HmacSha256::new_from_slice(transport_key) else {
        return 0x0001;
    };
    mac.update(&[payload_kind.to_nibble()]);
    mac.update(payload_bytes);
    let digest = mac.finalize().into_bytes();
    let mut code = u16::from_le_bytes([digest[0], digest[1]]);
    if code == 0x0000 {
        code = 0x0001;
    } else if code == 0xffff {
        code = 0xfffe;
    }
    code
}

pub fn channel_hash(channel_secret: &[u8]) -> u8 {
    Sha256::digest(channel_secret)[0]
}

pub fn node_hash<const N: usize>(public_key: &[u8; PUB_KEY_SIZE]) -> [u8; N] {
    let mut out = [0; N];
    let copy_len = N.min(PUB_KEY_SIZE);
    out[..copy_len].copy_from_slice(&public_key[..copy_len]);
    out
}

pub fn ack_hash_for_plain_text(
    timestamp: u32,
    txt_type_attempt: u8,
    message_text: &[u8],
    sender_public_key: &[u8; PUB_KEY_SIZE],
) -> [u8; 4] {
    let mut hasher = Sha256::new();
    hasher.update(timestamp.to_le_bytes());
    hasher.update([txt_type_attempt]);
    hasher.update(message_text);
    hasher.update(sender_public_key);
    first_four(hasher.finalize())
}

pub fn ack_hash_for_signed_plain_text(
    timestamp: u32,
    txt_type_attempt: u8,
    sender_pubkey_prefix: &[u8; 4],
    message_text: &[u8],
    recipient_public_key: &[u8; PUB_KEY_SIZE],
) -> [u8; 4] {
    let mut hasher = Sha256::new();
    hasher.update(timestamp.to_le_bytes());
    hasher.update([txt_type_attempt]);
    hasher.update(sender_pubkey_prefix);
    hasher.update(message_text);
    hasher.update(recipient_public_key);
    first_four(hasher.finalize())
}

fn first_four(digest: impl AsRef<[u8]>) -> [u8; 4] {
    let mut out = [0; 4];
    out.copy_from_slice(&digest.as_ref()[..4]);
    out
}
