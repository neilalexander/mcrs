use alloc::{string::String, vec, vec::Vec};

use crate::{
    AdvertNodeType, Error, MAX_ADVERT_DATA_SIZE, Result,
    wire::{read_i32_le, read_u8, read_u16_le},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertAppData {
    pub node_type: AdvertNodeType,
    pub location: Option<(i32, i32)>,
    pub feature1: Option<u16>,
    pub feature2: Option<u16>,
    pub name: Option<String>,
}

impl AdvertAppData {
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() > MAX_ADVERT_DATA_SIZE {
            return Err(Error::InvalidLength("advert app_data"));
        }

        let mut offset = 0;
        let flags = read_u8(input, &mut offset, "advert app_data flags")?;

        let location = if flags & 0x10 != 0 {
            let latitude = read_i32_le(input, &mut offset, "advert latitude")?;
            let longitude = read_i32_le(input, &mut offset, "advert longitude")?;
            Some((latitude, longitude))
        } else {
            None
        };

        let feature1 = if flags & 0x20 != 0 {
            Some(read_u16_le(input, &mut offset, "advert feature1")?)
        } else {
            None
        };

        let feature2 = if flags & 0x40 != 0 {
            Some(read_u16_le(input, &mut offset, "advert feature2")?)
        } else {
            None
        };

        let name = if flags & 0x80 != 0 {
            match String::from_utf8(input[offset..].to_vec()) {
                Ok(name) => Some(name),
                Err(_) => return Err(Error::InvalidUtf8),
            }
        } else {
            if offset != input.len() {
                return Err(Error::InvalidLength("advert app_data"));
            }
            None
        };

        Ok(Self {
            node_type: AdvertNodeType::from_nibble(flags),
            location,
            feature1,
            feature2,
            name,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut flags = self.node_type.to_nibble();
        if self.location.is_some() {
            flags |= 0x10;
        }
        if self.feature1.is_some() {
            flags |= 0x20;
        }
        if self.feature2.is_some() {
            flags |= 0x40;
        }
        if self.name.is_some() {
            flags |= 0x80;
        }

        let mut out = vec![flags];
        if let Some((latitude, longitude)) = self.location {
            out.extend_from_slice(&latitude.to_le_bytes());
            out.extend_from_slice(&longitude.to_le_bytes());
        }
        if let Some(feature) = self.feature1 {
            out.extend_from_slice(&feature.to_le_bytes());
        }
        if let Some(feature) = self.feature2 {
            out.extend_from_slice(&feature.to_le_bytes());
        }
        if let Some(name) = &self.name {
            out.extend_from_slice(name.as_bytes());
        }
        if out.len() > MAX_ADVERT_DATA_SIZE {
            return Err(Error::InvalidLength("advert app_data"));
        }
        Ok(out)
    }
}
