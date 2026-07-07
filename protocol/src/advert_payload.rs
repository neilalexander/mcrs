use alloc::vec::Vec;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use crate::{
    AdvertAppData, Error, MAX_ADVERT_DATA_SIZE, PUB_KEY_SIZE, Result, SIGNATURE_SIZE,
    wire::{read_array, read_u32_le},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertPayload {
    pub public_key: [u8; PUB_KEY_SIZE],
    pub timestamp: u32,
    pub signature: [u8; SIGNATURE_SIZE],
    pub app_data: Option<AdvertAppData>,
}

impl AdvertPayload {
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut offset = 0;
        let public_key = read_array(input, &mut offset, "advert public_key")?;
        let timestamp = read_u32_le(input, &mut offset, "advert timestamp")?;
        let signature = read_array(input, &mut offset, "advert signature")?;
        let app_data_bytes = &input[offset..];
        if app_data_bytes.len() > MAX_ADVERT_DATA_SIZE {
            return Err(Error::InvalidLength("advert app_data"));
        }
        let app_data = if app_data_bytes.is_empty() {
            None
        } else {
            Some(AdvertAppData::decode(app_data_bytes)?)
        };

        Ok(Self {
            public_key,
            timestamp,
            signature,
            app_data,
        })
    }

    pub(crate) fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        out.extend_from_slice(&self.public_key);
        out.extend_from_slice(&self.timestamp.to_le_bytes());
        out.extend_from_slice(&self.signature);
        if let Some(app_data) = &self.app_data {
            out.extend_from_slice(&app_data.encode()?);
        }
        Ok(())
    }

    pub fn verify_signature(&self) -> bool {
        let Ok(verifying_key) = VerifyingKey::from_bytes(&self.public_key) else {
            return false;
        };
        let signature = Signature::from_bytes(&self.signature);
        let Ok(message) = self.signed_message() else {
            return false;
        };

        verifying_key.verify(&message, &signature).is_ok()
    }

    fn signed_message(&self) -> Result<Vec<u8>> {
        let mut message = Vec::new();
        message.extend_from_slice(&self.public_key);
        message.extend_from_slice(&self.timestamp.to_le_bytes());
        if let Some(app_data) = &self.app_data {
            message.extend_from_slice(&app_data.encode()?);
        }
        Ok(message)
    }
}
