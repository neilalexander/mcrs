use alloc::vec::Vec;

use crate::{Error, HashSize, MAX_PATH_SIZE, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    hash_size: HashSize,
    bytes: Vec<u8>,
}

impl Path {
    pub fn empty() -> Self {
        Self {
            hash_size: HashSize::One,
            bytes: Vec::new(),
        }
    }

    pub fn new(hash_size: HashSize, bytes: Vec<u8>) -> Result<Self> {
        let path = Self { hash_size, bytes };
        path.validate()?;
        Ok(path)
    }

    pub fn from_hashes(hash_size: HashSize, hashes: &[&[u8]]) -> Result<Self> {
        let mut bytes = Vec::with_capacity(hash_size.size() * hashes.len());
        for hash in hashes {
            if hash.len() != hash_size.size() {
                return Err(Error::InvalidLength("path hash"));
            }
            bytes.extend_from_slice(hash);
        }
        Self::new(hash_size, bytes)
    }

    pub fn hash_size(&self) -> HashSize {
        self.hash_size
    }

    pub fn hop_count(&self) -> usize {
        self.bytes.len() / self.hash_size.size()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn hashes(&self) -> impl Iterator<Item = &[u8]> {
        self.bytes.chunks_exact(self.hash_size.size())
    }

    pub fn first_hash(&self) -> Option<&[u8]> {
        self.hashes().next()
    }

    pub fn first_hash_matches(&self, node_hash: &[u8]) -> bool {
        self.first_hash()
            .is_some_and(|first| hash_matches(first, node_hash))
    }

    pub fn contains_hash(&self, node_hash: &[u8]) -> bool {
        self.hashes().any(|hash| hash_matches(hash, node_hash))
    }

    pub fn append_hash(&mut self, node_hash: &[u8]) -> Result<()> {
        let size = self.hash_size.size();
        if node_hash.len() < size {
            return Err(Error::InvalidLength("node hash"));
        }
        if self.bytes.len() + size > MAX_PATH_SIZE {
            return Err(Error::PathTooLong {
                len: self.bytes.len() + size,
            });
        }
        if self.hop_count() >= 63 {
            return Err(Error::InvalidPathLength);
        }

        self.bytes.extend_from_slice(&node_hash[..size]);
        Ok(())
    }

    pub fn remove_first_hash(&mut self) -> Option<Vec<u8>> {
        let size = self.hash_size.size();
        if self.bytes.len() < size {
            return None;
        }
        Some(self.bytes.drain(..size).collect())
    }

    fn validate(&self) -> Result<()> {
        let size = self.hash_size.size();
        if self.bytes.len() > MAX_PATH_SIZE {
            return Err(Error::PathTooLong {
                len: self.bytes.len(),
            });
        }
        if !self.bytes.len().is_multiple_of(size) {
            return Err(Error::InvalidPathLength);
        }
        if self.hop_count() > 63 {
            return Err(Error::InvalidPathLength);
        }
        Ok(())
    }

    pub fn encoded_length_byte(&self) -> Result<u8> {
        self.validate()?;
        Ok((self.hash_size.code() << 6) | (self.hop_count() as u8))
    }

    pub fn decode_wire(path_length: u8, input: &[u8]) -> Result<(Self, usize)> {
        let hash_size = HashSize::from_code(path_length >> 6)?;
        let hop_count = (path_length & 0x3f) as usize;
        let len = hop_count
            .checked_mul(hash_size.size())
            .ok_or(Error::InvalidPathLength)?;

        if len > MAX_PATH_SIZE {
            return Err(Error::PathTooLong { len });
        }
        if input.len() < len {
            return Err(Error::Truncated("path"));
        }

        Ok((Self::new(hash_size, input[..len].to_vec())?, len))
    }

    pub fn encode_wire(&self, out: &mut Vec<u8>) -> Result<()> {
        out.push(self.encoded_length_byte()?);
        out.extend_from_slice(self.bytes());
        Ok(())
    }
}

fn hash_matches(path_hash: &[u8], node_hash: &[u8]) -> bool {
    node_hash.len() >= path_hash.len() && &node_hash[..path_hash.len()] == path_hash
}
