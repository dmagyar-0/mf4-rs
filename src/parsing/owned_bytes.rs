//! Owned byte buffers that may live in RAM or in an unlinked, memory-mapped
//! temporary file.
//!
//! Decompressed (`##DZ`) and demultiplexed (unsorted) data must be owned by
//! the reader, and for multi-GB logger files it can exceed physical RAM.
//! Buffers at or above [`spill_threshold`] are therefore backed by an
//! anonymous temp file mapped into memory: the kernel pages it in and out
//! like any file-backed mapping instead of the process being OOM-killed.
//! The file is unlinked on creation, so it disappears with the mapping —
//! even if the process crashes.

use std::ops::{Deref, DerefMut};

use crate::error::MdfError;

pub enum OwnedBytes {
    Mem(Vec<u8>),
    #[cfg(not(target_arch = "wasm32"))]
    Map(memmap2::MmapMut),
}

impl std::fmt::Debug for OwnedBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OwnedBytes::Mem(v) => write!(f, "OwnedBytes::Mem({} bytes)", v.len()),
            #[cfg(not(target_arch = "wasm32"))]
            OwnedBytes::Map(m) => write!(f, "OwnedBytes::Map({} bytes)", m.len()),
        }
    }
}

impl Deref for OwnedBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            OwnedBytes::Mem(v) => v,
            #[cfg(not(target_arch = "wasm32"))]
            OwnedBytes::Map(m) => m,
        }
    }
}

impl DerefMut for OwnedBytes {
    fn deref_mut(&mut self) -> &mut [u8] {
        match self {
            OwnedBytes::Mem(v) => v,
            #[cfg(not(target_arch = "wasm32"))]
            OwnedBytes::Map(m) => m,
        }
    }
}

impl OwnedBytes {
    /// True when backed by a temp-file mapping rather than heap memory.
    pub fn is_spilled(&self) -> bool {
        !matches!(self, OwnedBytes::Mem(_))
    }
}

/// Size from which owned buffers spill to a temp-file mapping. Default
/// 1 GiB; override with `MF4RS_SPILL_BYTES` (`0` disables spilling).
pub fn spill_threshold() -> u64 {
    std::env::var("MF4RS_SPILL_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|v| if v == 0 { u64::MAX } else { v })
        .unwrap_or(1 << 30)
}

/// Allocate a zero-filled buffer of `len` bytes, spilling to a temp-file
/// mapping when `len >= spill_threshold()` (native targets only). The temp
/// file goes to `MF4RS_TMPDIR` if set, else the system temp directory.
pub fn alloc_zeroed(len: usize) -> Result<OwnedBytes, MdfError> {
    #[cfg(not(target_arch = "wasm32"))]
    if len as u64 >= spill_threshold() {
        let file = match std::env::var_os("MF4RS_TMPDIR") {
            Some(dir) => tempfile::tempfile_in(dir),
            None => tempfile::tempfile(),
        }
        .map_err(MdfError::IOError)?;
        file.set_len(len as u64).map_err(MdfError::IOError)?;
        // SAFETY: the file is private to this process (unlinked temp file),
        // so no other process can truncate or modify it underneath us.
        let map = unsafe { memmap2::MmapMut::map_mut(&file) }.map_err(MdfError::IOError)?;
        return Ok(OwnedBytes::Map(map));
    }
    Ok(OwnedBytes::Mem(vec![0u8; len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spills_above_threshold_and_roundtrips() {
        // Threshold is read per call; set a tiny one for this test only.
        // (Tests run in parallel threads, so use a dedicated size.)
        unsafe { std::env::set_var("MF4RS_SPILL_BYTES", "4096") };
        let mut b = alloc_zeroed(1 << 20).unwrap();
        assert!(b.is_spilled());
        b[123_456] = 42;
        assert_eq!(b[123_456], 42);
        assert!(b.iter().filter(|&&x| x != 0).count() == 1);
        let small = alloc_zeroed(100).unwrap();
        assert!(!small.is_spilled());
        unsafe { std::env::remove_var("MF4RS_SPILL_BYTES") };
    }
}
