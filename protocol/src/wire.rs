use crate::{Error, MAX_PACKET_PAYLOAD, Result};

pub(crate) fn ensure_payload_len(input: &[u8]) -> Result<()> {
    if input.len() > MAX_PACKET_PAYLOAD {
        return Err(Error::PayloadTooLong { len: input.len() });
    }
    Ok(())
}

fn read_bytes<'a>(
    input: &'a [u8],
    offset: &mut usize,
    len: usize,
    field: &'static str,
) -> Result<&'a [u8]> {
    let end = offset.checked_add(len).ok_or(Error::InvalidLength(field))?;
    let bytes = input.get(*offset..end).ok_or(Error::Truncated(field))?;
    *offset = end;
    Ok(bytes)
}

pub(crate) fn read_array<const N: usize>(
    input: &[u8],
    offset: &mut usize,
    field: &'static str,
) -> Result<[u8; N]> {
    match read_bytes(input, offset, N, field)?.try_into() {
        Ok(bytes) => Ok(bytes),
        Err(_) => Err(Error::InvalidLength(field)),
    }
}

pub(crate) fn read_u8(input: &[u8], offset: &mut usize, field: &'static str) -> Result<u8> {
    Ok(read_bytes(input, offset, 1, field)?[0])
}

pub(crate) fn read_u16_le(input: &[u8], offset: &mut usize, field: &'static str) -> Result<u16> {
    Ok(u16::from_le_bytes(read_array(input, offset, field)?))
}

pub(crate) fn read_u32_le(input: &[u8], offset: &mut usize, field: &'static str) -> Result<u32> {
    Ok(u32::from_le_bytes(read_array(input, offset, field)?))
}

pub(crate) fn read_i32_le(input: &[u8], offset: &mut usize, field: &'static str) -> Result<i32> {
    Ok(i32::from_le_bytes(read_array(input, offset, field)?))
}
