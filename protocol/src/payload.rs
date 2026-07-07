use alloc::vec::Vec;

use crate::{
    AckPayload, AdvertPayload, AnonymousRequestPayload, ControlPayload, DirectEncryptedPayload,
    GroupEncryptedPayload, MultipartPayload, PayloadKind, Result, TracePayload,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    Request(DirectEncryptedPayload),
    Response(DirectEncryptedPayload),
    TextMessage(DirectEncryptedPayload),
    Ack(AckPayload),
    Advert(AdvertPayload),
    GroupText(GroupEncryptedPayload),
    GroupData(GroupEncryptedPayload),
    AnonymousRequest(AnonymousRequestPayload),
    Path(DirectEncryptedPayload),
    Trace(TracePayload),
    Multipart(MultipartPayload),
    Control(ControlPayload),
    Reserved { kind: u8, bytes: Vec<u8> },
    RawCustom(Vec<u8>),
}

impl Payload {
    pub fn kind(&self) -> PayloadKind {
        match self {
            Self::Request(_) => PayloadKind::Request,
            Self::Response(_) => PayloadKind::Response,
            Self::TextMessage(_) => PayloadKind::TextMessage,
            Self::Ack(_) => PayloadKind::Ack,
            Self::Advert(_) => PayloadKind::Advert,
            Self::GroupText(_) => PayloadKind::GroupText,
            Self::GroupData(_) => PayloadKind::GroupData,
            Self::AnonymousRequest(_) => PayloadKind::AnonymousRequest,
            Self::Path(_) => PayloadKind::Path,
            Self::Trace(_) => PayloadKind::Trace,
            Self::Multipart(_) => PayloadKind::Multipart,
            Self::Control(_) => PayloadKind::Control,
            Self::Reserved { kind, .. } => PayloadKind::Reserved(*kind),
            Self::RawCustom(_) => PayloadKind::RawCustom,
        }
    }

    pub fn decode(kind: PayloadKind, input: &[u8]) -> Result<Self> {
        Ok(match kind {
            PayloadKind::Request => Self::Request(DirectEncryptedPayload::decode(input)?),
            PayloadKind::Response => Self::Response(DirectEncryptedPayload::decode(input)?),
            PayloadKind::TextMessage => Self::TextMessage(DirectEncryptedPayload::decode(input)?),
            PayloadKind::Ack => Self::Ack(AckPayload::decode(input)?),
            PayloadKind::Advert => Self::Advert(AdvertPayload::decode(input)?),
            PayloadKind::GroupText => Self::GroupText(GroupEncryptedPayload::decode(input)?),
            PayloadKind::GroupData => Self::GroupData(GroupEncryptedPayload::decode(input)?),
            PayloadKind::AnonymousRequest => {
                Self::AnonymousRequest(AnonymousRequestPayload::decode(input)?)
            }
            PayloadKind::Path => Self::Path(DirectEncryptedPayload::decode(input)?),
            PayloadKind::Trace => Self::Trace(TracePayload::decode(input)?),
            PayloadKind::Multipart => Self::Multipart(MultipartPayload::decode(input)?),
            PayloadKind::Control => Self::Control(ControlPayload::decode(input)?),
            PayloadKind::Reserved(kind) => Self::Reserved {
                kind,
                bytes: input.to_vec(),
            },
            PayloadKind::RawCustom => Self::RawCustom(input.to_vec()),
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        match self {
            Self::Request(payload)
            | Self::Response(payload)
            | Self::TextMessage(payload)
            | Self::Path(payload) => payload.encode(&mut out),
            Self::Ack(payload) => payload.encode(&mut out),
            Self::Advert(payload) => payload.encode(&mut out)?,
            Self::GroupText(payload) | Self::GroupData(payload) => payload.encode(&mut out),
            Self::AnonymousRequest(payload) => payload.encode(&mut out),
            Self::Trace(payload) => payload.encode(&mut out)?,
            Self::Multipart(payload) => payload.encode(&mut out),
            Self::Control(payload) => payload.encode(&mut out),
            Self::Reserved { bytes, .. } | Self::RawCustom(bytes) => out.extend_from_slice(bytes),
        }
        Ok(out)
    }
}
