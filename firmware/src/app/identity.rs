use curve25519_dalek::edwards::CompressedEdwardsY;
use ed25519_dalek::{Signer, SigningKey};
use mcrs_protocol::{PUB_KEY_SIZE, node_hash};
use sha2::{Digest, Sha512};

pub const NODE_HASH_SIZE: usize = 4;
pub const REMOTE_CLI_PASSWORD: &str = "meshcore";

#[derive(Clone, Copy)]
pub struct Identity {
    public_key: [u8; PUB_KEY_SIZE],
    node_hash: [u8; NODE_HASH_SIZE],
    x25519_scalar: [u8; 32],
}

impl Identity {
    pub fn from_private_key_seed(seed: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(seed);
        let public_key = signing_key.verifying_key().to_bytes();
        let node_hash = node_hash::<NODE_HASH_SIZE>(&public_key);
        let x25519_scalar = x25519_scalar_from_ed25519_seed(seed);

        Self {
            public_key,
            node_hash,
            x25519_scalar,
        }
    }

    pub fn public_key(&self) -> &[u8; PUB_KEY_SIZE] {
        &self.public_key
    }

    pub fn node_hash(&self) -> &[u8; NODE_HASH_SIZE] {
        &self.node_hash
    }

    pub fn shared_secret_with_ed25519_public(
        &self,
        peer_public_key: &[u8; PUB_KEY_SIZE],
    ) -> Option<[u8; 32]> {
        let peer_point = CompressedEdwardsY(*peer_public_key).decompress()?;
        let shared = peer_point.to_montgomery().mul_clamped(self.x25519_scalar).0;

        if shared == [0; 32] {
            return None;
        }

        Some(shared)
    }
}

pub fn sign_with_seed(seed: &[u8; 32], message: &[u8]) -> [u8; 64] {
    SigningKey::from_bytes(seed).sign(message).to_bytes()
}

fn x25519_scalar_from_ed25519_seed(seed: &[u8; 32]) -> [u8; 32] {
    let digest = Sha512::digest(seed);
    let mut scalar = [0; 32];
    scalar.copy_from_slice(&digest[..32]);
    scalar
}
