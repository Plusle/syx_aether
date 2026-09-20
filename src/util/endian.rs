#![allow(dead_code)]

//! Endian-aware byte reading utilities.

/// Read u16 from bytes with specified endianness.
#[inline]
pub fn read_u16(bytes: &[u8], offset: usize, big_endian: bool) -> u16 {
    if big_endian {
        u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
    } else {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }
}

/// Read u32 from bytes with specified endianness.
#[inline]
pub fn read_u32(bytes: &[u8], offset: usize, big_endian: bool) -> u32 {
    if big_endian {
        u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    } else {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }
}

/// Read u64 from bytes with specified endianness.
#[inline]
pub fn read_u64(bytes: &[u8], offset: usize, big_endian: bool) -> u64 {
    if big_endian {
        u64::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ])
    } else {
        u64::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ])
    }
}

/// Read i16 from bytes with specified endianness.
#[inline]
pub fn read_i16(bytes: &[u8], offset: usize, big_endian: bool) -> i16 {
    if big_endian {
        i16::from_be_bytes([bytes[offset], bytes[offset + 1]])
    } else {
        i16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    }
}

/// Read i32 from bytes with specified endianness.
#[inline]
pub fn read_i32(bytes: &[u8], offset: usize, big_endian: bool) -> i32 {
    if big_endian {
        i32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    } else {
        i32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }
}

/// Read f32 from bytes with specified endianness.
#[inline]
pub fn read_f32(bytes: &[u8], offset: usize, big_endian: bool) -> f32 {
    if big_endian {
        f32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    } else {
        f32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }
}
