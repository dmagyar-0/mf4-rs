//! Data-chain resolution with `##HL` / `##DZ` support.
//!
//! MDF 4.1+ allows any data section (record data `##DT`, sample data `##DV`,
//! signal data `##SD`, ...) to be stored compressed in a `##DZ` block, and
//! lists of compressed fragments to be wrapped in a `##HL` header list that
//! points at an ordinary `##DL` chain.
//!
//! [`collect_fragments`] walks such a chain and returns borrowed raw
//! fragments plus still-compressed `##DZ` fragments; [`inflate_dz`] turns a
//! `##DZ` block back into the original bytes (undoing the optional
//! transposition step). Callers decide whether they can stay zero-copy (all
//! fragments raw and record-aligned) or must coalesce into an owned buffer.

use std::collections::HashSet;

use crate::blocks::common::{BlockHeader, BlockParse};
use crate::blocks::data_list_block::DataListBlock;
use crate::error::MdfError;
use crate::parsing::owned_bytes::{alloc_zeroed, OwnedBytes};

/// One fragment of a data section.
#[derive(Debug, Clone, Copy)]
pub enum Fragment<'a> {
    /// Uncompressed payload (data portion of a leaf block, header stripped).
    Raw(&'a [u8]),
    /// A complete `##DZ` block (header included), still compressed.
    Dz(&'a [u8]),
}

impl Fragment<'_> {
    pub fn is_compressed(&self) -> bool {
        matches!(self, Fragment::Dz(_))
    }
}

/// Size of the fixed `##DZ` block prefix: 24-byte header (no links) +
/// org_block_type[2] + zip_type u8 + reserved u8 + zip_parameter u32 +
/// org_data_length u64 + data_length u64.
const DZ_DATA_OFFSET: usize = 24 + 2 + 1 + 1 + 4 + 8 + 8;

fn too_short(actual: usize, expected: usize) -> MdfError {
    MdfError::TooShortBuffer { actual, expected, file: file!(), line: line!() }
}

/// Return the whole block (header included) starting at `addr`, bounds-checked
/// against the declared `block_len`.
fn block_at(mmap: &[u8], addr: u64) -> Result<(BlockHeader, &[u8]), MdfError> {
    let off = usize::try_from(addr).map_err(|_| too_short(mmap.len(), usize::MAX))?;
    let hdr_bytes = mmap
        .get(off..off.saturating_add(24))
        .ok_or_else(|| too_short(mmap.len(), off.saturating_add(24)))?;
    let header = BlockHeader::from_bytes(hdr_bytes)?;
    let len = usize::try_from(header.block_len).map_err(|_| too_short(mmap.len(), usize::MAX))?;
    if len < 24 {
        return Err(MdfError::BlockSerializationError(format!(
            "block {} at {:#x} declares length {} < 24",
            header.id, addr, len
        )));
    }
    let end = off.checked_add(len).ok_or_else(|| too_short(mmap.len(), usize::MAX))?;
    let bytes = mmap.get(off..end).ok_or_else(|| too_short(mmap.len(), end))?;
    Ok((header, bytes))
}

/// Resolve one leaf link (a fragment referenced from a `##DL`, or the
/// group's direct data link) to a [`Fragment`].
fn leaf<'a>(
    mmap: &'a [u8],
    addr: u64,
    leaf_ids: &[&str],
) -> Result<Option<Fragment<'a>>, MdfError> {
    let (header, bytes) = block_at(mmap, addr)?;
    if header.id == "##DZ" {
        return Ok(Some(Fragment::Dz(bytes)));
    }
    if leaf_ids.contains(&header.id.as_str()) {
        return Ok(Some(Fragment::Raw(&bytes[24..])));
    }
    Ok(None)
}

/// Walk a data chain starting at `start` and collect its fragments in order.
///
/// Accepts, at any level: a leaf block whose id is in `leaf_ids` (e.g.
/// `##DT`/`##DV` or `##SD`), `##DZ`, `##DL` (following `dl_dl_next`), and
/// `##HL` (descending into `hl_dl_first`). Cycles are rejected.
pub fn collect_fragments<'a>(
    mmap: &'a [u8],
    start: u64,
    leaf_ids: &[&str],
) -> Result<Vec<Fragment<'a>>, MdfError> {
    let mut out = Vec::new();
    let mut visited = HashSet::new();
    let mut addr = start;
    let expected = || format!("{} / ##DL / ##HL / ##DZ", leaf_ids.join(" / "));

    while addr != 0 {
        if !visited.insert(addr) {
            return Err(MdfError::BlockLinkError(format!(
                "cycle detected in data block chain at address {:#x}",
                addr
            )));
        }
        let (header, bytes) = block_at(mmap, addr)?;
        match header.id.as_str() {
            "##HL" => {
                // hl_dl_first is the first (and only) link.
                if header.links_nr < 1 || bytes.len() < 32 {
                    return Err(too_short(bytes.len(), 32));
                }
                addr = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
            }
            "##DL" => {
                let dl = DataListBlock::from_bytes(bytes)?;
                for &link in &dl.data_links {
                    if link == 0 {
                        continue;
                    }
                    match leaf(mmap, link, leaf_ids)? {
                        Some(f) => out.push(f),
                        None => {
                            let (h, _) = block_at(mmap, link)?;
                            return Err(MdfError::BlockIDError {
                                actual: h.id,
                                expected: format!("{} / ##DZ", leaf_ids.join(" / ")),
                            });
                        }
                    }
                }
                addr = dl.next;
            }
            id if id == "##DZ" || leaf_ids.contains(&id) => {
                out.push(leaf(mmap, addr, leaf_ids)?.expect("checked id"));
                addr = 0;
            }
            other => {
                return Err(MdfError::BlockIDError {
                    actual: other.to_string(),
                    expected: expected(),
                });
            }
        }
    }
    Ok(out)
}

/// Parsed `##DZ` header fields.
#[derive(Debug, Clone, Copy)]
pub struct DzInfo {
    pub org_block_type: [u8; 2],
    pub zip_type: u8,
    pub zip_parameter: u32,
    pub org_data_length: u64,
    pub data_length: u64,
}

pub fn dz_info(block: &[u8]) -> Result<DzInfo, MdfError> {
    if block.len() < DZ_DATA_OFFSET {
        return Err(too_short(block.len(), DZ_DATA_OFFSET));
    }
    Ok(DzInfo {
        org_block_type: [block[24], block[25]],
        zip_type: block[26],
        zip_parameter: u32::from_le_bytes(block[28..32].try_into().unwrap()),
        org_data_length: u64::from_le_bytes(block[32..40].try_into().unwrap()),
        data_length: u64::from_le_bytes(block[40..48].try_into().unwrap()),
    })
}

/// Validated pieces of a `##DZ` block: header info, compressed bytes and the
/// declared original length.
fn dz_parts(block: &[u8]) -> Result<(DzInfo, &[u8], usize), MdfError> {
    let info = dz_info(block)?;
    let data_len = usize::try_from(info.data_length).map_err(|_| too_short(block.len(), usize::MAX))?;
    let end = DZ_DATA_OFFSET
        .checked_add(data_len)
        .ok_or_else(|| too_short(block.len(), usize::MAX))?;
    let compressed = block.get(DZ_DATA_OFFSET..end).ok_or_else(|| too_short(block.len(), end))?;
    let org_len = usize::try_from(info.org_data_length)
        .map_err(|_| too_short(block.len(), usize::MAX))?;
    // Deflate cannot expand beyond ~1032:1. Reject larger claims before
    // allocating, so a forged header cannot trigger a huge allocation.
    if org_len > compressed.len().saturating_mul(1032).saturating_add(64) {
        return Err(MdfError::BlockSerializationError(format!(
            "##DZ declares {} original bytes for {} compressed bytes (impossible ratio)",
            org_len,
            compressed.len()
        )));
    }
    if info.zip_type > 1 {
        return Err(MdfError::BlockSerializationError(format!(
            "unsupported ##DZ zip_type {}",
            info.zip_type
        )));
    }
    Ok((info, compressed, org_len))
}

/// Inflate a zlib stream into `out`, requiring it to produce exactly
/// `out.len()` bytes.
fn inflate_exact(compressed: &[u8], out: &mut [u8]) -> Result<(), MdfError> {
    use flate2::{Decompress, FlushDecompress, Status};
    let mut d = Decompress::new(true);
    let status = d
        .decompress(compressed, out, FlushDecompress::Finish)
        .map_err(|e| MdfError::BlockSerializationError(format!("##DZ inflate failed: {e}")))?;
    let produced = d.total_out() as usize;
    match status {
        Status::StreamEnd if produced == out.len() => Ok(()),
        Status::StreamEnd => Err(MdfError::BlockSerializationError(format!(
            "##DZ inflated to {} bytes, header declares {}",
            produced,
            out.len()
        ))),
        _ => Err(MdfError::BlockSerializationError(format!(
            "##DZ stream exceeds the declared {} bytes (or is truncated)",
            out.len()
        ))),
    }
}

/// Decompress a `##DZ` block directly into `out` (which must be exactly
/// `dz_org_data_length` bytes). Supports `zip_type` 0 (deflate) and 1
/// (transposition + deflate).
pub fn inflate_dz_into(block: &[u8], out: &mut [u8]) -> Result<(), MdfError> {
    let (info, compressed, org_len) = dz_parts(block)?;
    if out.len() != org_len {
        return Err(MdfError::BlockSerializationError(format!(
            "##DZ output buffer is {} bytes, header declares {}",
            out.len(),
            org_len
        )));
    }
    let cols = info.zip_parameter as usize;
    if info.zip_type == 1 && cols > 1 && org_len / cols > 1 {
        let mut tmp = vec![0u8; org_len];
        inflate_exact(compressed, &mut tmp)?;
        untranspose_into(&tmp, cols, out);
        Ok(())
    } else {
        inflate_exact(compressed, out)
    }
}

/// Decompress a complete `##DZ` block into its original payload.
pub fn inflate_dz(block: &[u8]) -> Result<Vec<u8>, MdfError> {
    let (_, _, org_len) = dz_parts(block)?;
    let mut out = vec![0u8; org_len];
    inflate_dz_into(block, &mut out)?;
    Ok(out)
}

/// Undo MDF "transposition before deflate" from `data` into `out`.
///
/// The writer views the first `rows * cols` bytes (`cols` = zip_parameter,
/// usually the record size) as a row-major matrix and stores it column-major;
/// trailing bytes that do not fill a whole row are appended untouched.
pub fn untranspose_into(data: &[u8], cols: usize, out: &mut [u8]) {
    debug_assert_eq!(data.len(), out.len());
    let rows = if cols > 1 { data.len() / cols } else { 0 };
    if rows <= 1 {
        out.copy_from_slice(data);
        return;
    }
    let body = rows * cols;
    // Row tiles keep both the strided reads and contiguous writes in cache.
    const TILE: usize = 64;
    for r0 in (0..rows).step_by(TILE) {
        let r1 = (r0 + TILE).min(rows);
        for c in 0..cols {
            let src = &data[c * rows + r0..c * rows + r1];
            for (k, &v) in src.iter().enumerate() {
                out[(r0 + k) * cols + c] = v;
            }
        }
    }
    out[body..].copy_from_slice(&data[body..]);
}

/// Owned-buffer convenience wrapper around [`untranspose_into`].
pub fn untranspose(data: Vec<u8>, cols: usize) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    untranspose_into(&data, cols, &mut out);
    out
}

/// Materialise all fragments into one contiguous buffer.
///
/// Sizes are known up front from the `##DZ` headers, so every fragment is
/// inflated (or copied) **straight into its final position** — no
/// intermediate per-fragment buffers, so peak memory is the output size.
/// Fragments are processed in parallel on native targets.
pub fn coalesce(fragments: &[Fragment<'_>]) -> Result<OwnedBytes, MdfError> {
    let mut sizes = Vec::with_capacity(fragments.len());
    for f in fragments {
        sizes.push(match f {
            Fragment::Raw(r) => r.len(),
            Fragment::Dz(b) => dz_parts(b)?.2,
        });
    }
    let total = sizes
        .iter()
        .try_fold(0usize, |acc, &n| acc.checked_add(n))
        .ok_or_else(|| MdfError::BlockSerializationError("data section size overflows".into()))?;
    let mut out = alloc_zeroed(total)?;

    let mut work: Vec<(Fragment<'_>, &mut [u8])> = Vec::with_capacity(fragments.len());
    let mut rest: &mut [u8] = &mut out[..];
    for (f, &n) in fragments.iter().zip(&sizes) {
        let (head, tail) = rest.split_at_mut(n);
        work.push((*f, head));
        rest = tail;
    }

    let fill = |f: Fragment<'_>, dst: &mut [u8]| -> Result<(), MdfError> {
        match f {
            Fragment::Raw(r) => {
                dst.copy_from_slice(r);
                Ok(())
            }
            Fragment::Dz(b) => inflate_dz_into(b, dst),
        }
    };

    let n_dz = fragments.iter().filter(|f| f.is_compressed()).count();
    let threads = worker_threads().min(n_dz);
    if threads <= 1 {
        for (f, dst) in work {
            fill(f, dst)?;
        }
        return Ok(out);
    }

    let queue = std::sync::Mutex::new(work);
    let result: Result<(), MdfError> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let queue = &queue;
                let fill = &fill;
                s.spawn(move || -> Result<(), MdfError> {
                    loop {
                        let item = queue.lock().map_err(|_| {
                            MdfError::BlockSerializationError("inflate queue poisoned".into())
                        })?.pop();
                        match item {
                            Some((f, dst)) => fill(f, dst)?,
                            None => return Ok(()),
                        }
                    }
                })
            })
            .collect();
        handles.into_iter().try_for_each(|h| {
            h.join().unwrap_or_else(|_| {
                Err(MdfError::BlockSerializationError("inflate worker panicked".into()))
            })
        })
    });
    result?;
    Ok(out)
}

/// Number of worker threads for embarrassingly parallel work. Always 1 on
/// targets without threads (wasm32).
pub fn worker_threads() -> usize {
    #[cfg(target_arch = "wasm32")]
    {
        1
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("MF4RS_THREADS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&n: &usize| n >= 1)
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transpose(data: &[u8], cols: usize) -> Vec<u8> {
        let rows = data.len() / cols;
        let body = rows * cols;
        let mut out = vec![0u8; data.len()];
        for r in 0..rows {
            for c in 0..cols {
                out[c * rows + r] = data[r * cols + c];
            }
        }
        out[body..].copy_from_slice(&data[body..]);
        out
    }

    fn make_dz(org: &[u8], zip_type: u8, param: u32) -> Vec<u8> {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;
        let pre = if zip_type == 1 { transpose(org, param as usize) } else { org.to_vec() };
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&pre).unwrap();
        let comp = enc.finish().unwrap();
        let len = (DZ_DATA_OFFSET + comp.len()) as u64;
        let mut b = Vec::new();
        b.extend_from_slice(b"##DZ");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&len.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(b"DT");
        b.push(zip_type);
        b.push(0);
        b.extend_from_slice(&param.to_le_bytes());
        b.extend_from_slice(&(org.len() as u64).to_le_bytes());
        b.extend_from_slice(&(comp.len() as u64).to_le_bytes());
        b.extend_from_slice(&comp);
        b
    }

    #[test]
    fn roundtrip_plain_and_transposed() {
        let org: Vec<u8> = (0..10_007u32).map(|i| (i * 31 % 251) as u8).collect();
        for (zt, p) in [(0u8, 0u32), (1, 13), (1, 1), (1, 20_000)] {
            let dz = make_dz(&org, zt, p);
            assert_eq!(inflate_dz(&dz).unwrap(), org, "zip_type={zt} param={p}");
        }
    }

    #[test]
    fn rejects_length_mismatch() {
        let org = vec![7u8; 100];
        let mut dz = make_dz(&org, 0, 0);
        dz[32..40].copy_from_slice(&99u64.to_le_bytes());
        assert!(inflate_dz(&dz).is_err());
    }

    #[test]
    fn rejects_truncated_block() {
        let dz = make_dz(&[1u8; 64], 0, 0);
        assert!(inflate_dz(&dz[..dz.len() - 3]).is_err());
        assert!(inflate_dz(&dz[..30]).is_err());
    }
}
