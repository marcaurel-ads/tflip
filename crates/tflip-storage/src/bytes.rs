//! Readers of the little numbers and NUL-ended text the file headers hold.

/// The little-endian `u16` at `offset`.
pub(crate) fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

/// The little-endian `u32` at `offset`.
pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// The big-endian `u32` at `offset`.
pub(crate) fn u32_be_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// `bytes` up to its first NUL.
pub(crate) fn trim_nul(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())]
}

/// Text up to its first NUL, as Windows-1252.
pub(crate) fn text(bytes: &[u8]) -> String {
    crate::cp1252::decode(trim_nul(bytes))
}
