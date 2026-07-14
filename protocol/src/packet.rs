use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use crate::{
    Error, Header, MAX_PACKET_PAYLOAD, MAX_TRANS_UNIT, Path, Payload, PayloadKind, Result,
    RoutePath, RouteType, TracePath, TransportCodes,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub route_type: RouteType,
    pub transport_codes: Option<TransportCodes>,
    pub path: RoutePath,
    pub payload: Payload,
}

impl Packet {
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.is_empty() {
            return Err(Error::EmptyPacket);
        }
        if input.len() > MAX_TRANS_UNIT {
            return Err(Error::PacketTooLong { len: input.len() });
        }

        let header = Header::decode(input[0])?;
        let mut offset = 1;

        let transport_codes = if header.route_type.has_transport_codes() {
            let codes = TransportCodes::decode(input.get(offset..).unwrap_or_default())?;
            offset += 4;
            Some(codes)
        } else {
            None
        };

        let path_length = *input.get(offset).ok_or(Error::Truncated("path_length"))?;
        offset += 1;

        let path = if header.payload_kind == PayloadKind::Trace {
            let (path, used) = TracePath::decode(path_length, &input[offset..])?;
            offset += used;
            RoutePath::Trace(path)
        } else {
            let (path, used) = Path::decode_wire(path_length, &input[offset..])?;
            offset += used;
            RoutePath::Normal(path)
        };

        let payload_bytes = &input[offset..];
        if payload_bytes.len() > MAX_PACKET_PAYLOAD {
            return Err(Error::PayloadTooLong {
                len: payload_bytes.len(),
            });
        }

        let packet = Self {
            route_type: header.route_type,
            transport_codes,
            path,
            payload: Payload::decode(header.payload_kind, payload_bytes)?,
        };
        packet.validate_semantics()?;
        Ok(packet)
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.route_type.has_transport_codes() && self.transport_codes.is_none() {
            return Err(Error::MissingTransportCodes);
        }
        if !self.route_type.has_transport_codes() && self.transport_codes.is_some() {
            return Err(Error::UnexpectedTransportCodes);
        }
        self.validate_semantics()?;

        let payload_kind = self.payload.kind();
        let mut out = Vec::new();
        out.push(
            Header {
                payload_version: 1,
                payload_kind,
                route_type: self.route_type,
            }
            .encode()?,
        );

        if let Some(codes) = self.transport_codes {
            codes.encode(&mut out);
        }

        match (&self.path, payload_kind) {
            (RoutePath::Trace(path), PayloadKind::Trace) => {
                out.push(path.encoded_length_byte()?);
                path.encode_bytes(&mut out);
            }
            (RoutePath::Normal(path), kind) if kind != PayloadKind::Trace => {
                out.push(path.encoded_length_byte()?);
                out.extend_from_slice(path.bytes());
            }
            _ => return Err(Error::PathKindMismatch),
        }

        let payload = self.payload.encode()?;
        if payload.len() > MAX_PACKET_PAYLOAD {
            return Err(Error::PayloadTooLong { len: payload.len() });
        }
        out.extend_from_slice(&payload);

        if out.len() > MAX_TRANS_UNIT {
            return Err(Error::PacketTooLong { len: out.len() });
        }

        Ok(out)
    }

    pub fn payload_kind(&self) -> PayloadKind {
        self.payload.kind()
    }

    pub fn normal_path(&self) -> Option<&Path> {
        match &self.path {
            RoutePath::Normal(path) => Some(path),
            RoutePath::Trace(_) => None,
        }
    }

    pub fn normal_path_mut(&mut self) -> Option<&mut Path> {
        match &mut self.path {
            RoutePath::Normal(path) => Some(path),
            RoutePath::Trace(_) => None,
        }
    }

    pub fn trace_path(&self) -> Option<&TracePath> {
        match &self.path {
            RoutePath::Trace(path) => Some(path),
            RoutePath::Normal(_) => None,
        }
    }

    pub fn trace_path_mut(&mut self) -> Option<&mut TracePath> {
        match &mut self.path {
            RoutePath::Trace(path) => Some(path),
            RoutePath::Normal(_) => None,
        }
    }

    pub fn append_flood_hop(&mut self, node_hash: &[u8]) -> Result<()> {
        self.normal_path_mut()
            .ok_or(Error::PathKindMismatch)?
            .append_hash(node_hash)
    }

    pub fn consume_direct_hop(&mut self, node_hash: &[u8]) -> Result<bool> {
        let path = self.normal_path_mut().ok_or(Error::PathKindMismatch)?;
        if !path.first_hash_matches(node_hash) {
            return Ok(false);
        }
        path.remove_first_hash();
        Ok(true)
    }

    pub fn trace_next_hop_matches(&self, node_hash: &[u8]) -> Result<bool> {
        let trace_path = self.trace_path().ok_or(Error::PathKindMismatch)?;
        let Payload::Trace(payload) = &self.payload else {
            return Err(Error::PathKindMismatch);
        };
        let size = payload.hash_size.size();
        let offset = trace_path.consumed_hops as usize * size;
        if offset >= payload.path_hashes.len() {
            return Ok(false);
        }
        if offset + size > payload.path_hashes.len() {
            return Err(Error::InvalidLength("trace path_hashes"));
        }
        let expected = &payload.path_hashes[offset..offset + size];
        Ok(node_hash.len() >= size && &node_hash[..size] == expected)
    }

    pub fn trace_is_complete(&self) -> Result<bool> {
        let trace_path = self.trace_path().ok_or(Error::PathKindMismatch)?;
        let Payload::Trace(payload) = &self.payload else {
            return Err(Error::PathKindMismatch);
        };
        Ok(trace_path.consumed_hops as usize * payload.hash_size.size()
            >= payload.path_hashes.len())
    }

    pub fn append_trace_snr(&mut self, snr_quarters: i8) -> Result<()> {
        self.trace_path_mut()
            .ok_or(Error::PathKindMismatch)?
            .append_snr(snr_quarters)
    }

    pub fn dedup_signature(&self) -> Result<[u8; 8]> {
        if self.route_type.has_transport_codes() && self.transport_codes.is_none() {
            return Err(Error::MissingTransportCodes);
        }
        if !self.route_type.has_transport_codes() && self.transport_codes.is_some() {
            return Err(Error::UnexpectedTransportCodes);
        }

        let mut hasher = Sha256::new();
        hasher.update([self.payload.kind().to_nibble()]);
        hasher.update([self.route_type.to_bits()]);
        if let Some(codes) = self.transport_codes {
            hasher.update(codes.primary.to_le_bytes());
            hasher.update(codes.secondary.to_le_bytes());
        }
        if self.payload.kind() == PayloadKind::Trace {
            let RoutePath::Trace(path) = &self.path else {
                return Err(Error::PathKindMismatch);
            };
            hasher.update([path.encoded_length_byte()?]);
        }
        hasher.update(self.payload.encode()?);
        let digest = hasher.finalize();
        let mut signature = [0; 8];
        signature.copy_from_slice(&digest[..8]);
        Ok(signature)
    }

    fn validate_semantics(&self) -> Result<()> {
        if let Payload::Control(payload) = &self.payload
            && payload.zero_hop_only()
            && !self.is_direct_zero_hop()
        {
            return Err(Error::InvalidZeroHopControlRoute);
        }
        Ok(())
    }

    fn is_direct_zero_hop(&self) -> bool {
        self.route_type.is_direct()
            && matches!(&self.path, RoutePath::Normal(path) if path.hop_count() == 0)
    }
}
