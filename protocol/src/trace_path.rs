use alloc::vec::Vec;

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracePath {
    pub consumed_hops: u8,
    pub snr_samples: Vec<i8>,
}

impl TracePath {
    pub fn new(snr_samples: Vec<i8>) -> Result<Self> {
        if snr_samples.len() > 63 {
            return Err(Error::InvalidTracePathLength(snr_samples.len() as u8));
        }
        Ok(Self {
            consumed_hops: snr_samples.len() as u8,
            snr_samples,
        })
    }

    pub(crate) fn decode(path_length: u8, input: &[u8]) -> Result<(Self, usize)> {
        if path_length & 0xc0 != 0 {
            return Err(Error::InvalidTracePathLength(path_length));
        }
        let consumed = path_length & 0x3f;
        let len = consumed as usize;
        if input.len() < len {
            return Err(Error::Truncated("trace snr path"));
        }
        let snr_samples = input[..len].iter().map(|b| *b as i8).collect();
        Ok((
            Self {
                consumed_hops: consumed,
                snr_samples,
            },
            len,
        ))
    }

    pub(crate) fn encoded_length_byte(&self) -> Result<u8> {
        if self.consumed_hops as usize != self.snr_samples.len() || self.consumed_hops > 63 {
            return Err(Error::InvalidTracePathLength(self.consumed_hops));
        }
        Ok(self.consumed_hops)
    }

    pub(crate) fn encode_bytes(&self, out: &mut Vec<u8>) {
        out.extend(self.snr_samples.iter().map(|sample| *sample as u8));
    }

    pub fn append_snr(&mut self, snr_quarters: i8) -> Result<()> {
        if self.consumed_hops >= 63 {
            return Err(Error::InvalidTracePathLength(self.consumed_hops));
        }
        self.snr_samples.push(snr_quarters);
        self.consumed_hops += 1;
        Ok(())
    }
}
