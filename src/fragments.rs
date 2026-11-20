//! Fragment-backed byte-range readers shared by the wasm bindings and the
//! native URL read path.
//!
//! A [`FragmentStore`] holds file fragments fetched so far; a
//! [`RecordingRangeReader`] serves reads from it and records the ranges still
//! missing, so a caller can run an operation as a *probe*, fetch every gap it
//! reported in one batch (concurrently), and run it again — a handful of
//! rounds instead of one blocking round-trip per small read.

// The native `http` read path uses only part of this module; the wasm
// bindings use all of it.
#![cfg_attr(not(feature = "wasm"), allow(dead_code))]

use crate::error::MdfError;
use crate::index::ByteRangeReader;

/// Sort and merge overlapping/adjacent byte ranges into a minimal cover.
pub(crate) fn merge_ranges(ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    merge_ranges_bridging(ranges, 0)
}

/// Gap between two blocks that is bridged into one span: up to 7 bytes of
/// 8-byte alignment padding.
pub(crate) const BLOCK_GAP_BRIDGE: u64 = 7;

/// [`merge_ranges`], additionally joining ranges separated by at most
/// `bridge` bytes (the gap is fetched along with them).
pub(crate) fn merge_ranges_bridging(mut ranges: Vec<(u64, u64)>, bridge: u64) -> Vec<(u64, u64)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|r| r.0);
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (offset, length) in ranges {
        let end = offset + length;
        if let Some(last) = out.last_mut() {
            let last_end = last.0 + last.1;
            if offset <= last_end.saturating_add(bridge) {
                if end > last_end {
                    last.1 = end - last.0;
                }
                continue;
            }
        }
        out.push((offset, length));
    }
    out
}

// ---------------------------------------------------------------------------
// Fragment-backed byte-range reader
// ---------------------------------------------------------------------------

/// Narrow a `u64` byte count/offset to `usize`, erroring instead of truncating
/// on a 32-bit (wasm) target where the value exceeds `usize::MAX`.
pub(crate) fn u64_to_usize(v: u64) -> Result<usize, MdfError> {
    usize::try_from(v).map_err(|_| {
        MdfError::BlockSerializationError(format!("byte value {} exceeds addressable range", v))
    })
}

/// End offset of a fragment, erroring on overflow instead of wrapping.
pub(crate) fn fragment_end(fstart: u64, len: usize) -> Result<u64, MdfError> {
    fstart.checked_add(len as u64).ok_or_else(|| {
        MdfError::BlockSerializationError("fragment offset + length overflows u64".to_string())
    })
}

/// Outcome of trying to serve a read from a set of file fragments.
pub(crate) enum Coverage {
    /// The request was fully covered; here are the assembled bytes.
    Full(Vec<u8>),
    /// A gap was found: `offset`/`length` describe the first still-missing
    /// sub-range of the request.
    Gap { offset: u64, length: u64 },
}

/// A store of file fragments — `(absolute_offset, bytes)` pairs, possibly
/// overlapping or exactly adjacent — kept sorted by `offset` so a read is
/// served by two binary searches (`partition_point`) instead of scanning
/// every stored fragment. This is what makes repeated reads against a
/// steadily growing fragment set (an incremental index build accumulating
/// fetched bytes across many passes) subquadratic: each `push` inserts once,
/// each `assemble`/read is `O(log F + w)` where `w` is the number of
/// fragments actually overlapping the request (typically tiny), not `O(F)`.
///
/// Alongside the sorted fragments, `running_max_end[i]` tracks the largest
/// end offset among `fragments[..=i]`. Because `fragments` is sorted by start
/// and `running_max_end` is therefore monotonically non-decreasing, binary
/// searching it finds the true lower bound of the fragments that can
/// possibly overlap a query — even when an early, large fragment fully spans
/// later, smaller ones — so no candidate that could satisfy a read is ever
/// skipped. This is the standard "stabbing query" trick for interval sets.
pub(crate) struct FragmentStore {
    fragments: Vec<(u64, Vec<u8>)>,
    running_max_end: Vec<u64>,
}

impl FragmentStore {
    pub(crate) fn new() -> Self {
        Self { fragments: Vec::new(), running_max_end: Vec::new() }
    }

    /// Build a store from an unordered `(offset, bytes)` list (e.g. the
    /// ranges/fragments arrays marshalled from JS in one shot), sorting once.
    pub(crate) fn from_pairs(mut fragments: Vec<(u64, Vec<u8>)>) -> Result<Self, MdfError> {
        fragments.sort_by_key(|(start, _)| *start);
        let mut store = Self { fragments, running_max_end: Vec::new() };
        store.rebuild_running_max()?;
        Ok(store)
    }

    /// Insert one fragment, keeping the store sorted by `offset`.
    pub(crate) fn push(&mut self, offset: u64, bytes: Vec<u8>) -> Result<(), MdfError> {
        let idx = self.fragments.partition_point(|(start, _)| *start <= offset);
        self.fragments.insert(idx, (offset, bytes));
        self.rebuild_running_max()
    }

    fn rebuild_running_max(&mut self) -> Result<(), MdfError> {
        self.running_max_end.clear();
        self.running_max_end.reserve(self.fragments.len());
        let mut max_end = 0u64;
        for (start, bytes) in &self.fragments {
            let end = fragment_end(*start, bytes.len())?;
            max_end = max_end.max(end);
            self.running_max_end.push(max_end);
        }
        Ok(())
    }

    /// The minimal contiguous slice of `fragments` that could possibly
    /// overlap `[offset, req_end)`, found via two binary searches instead of
    /// a linear scan over every stored fragment.
    fn candidates(&self, offset: u64, req_end: u64) -> &[(u64, Vec<u8>)] {
        // Fragments beyond `hi` start at/after `req_end`, so they cannot
        // overlap a half-open request ending at `req_end`.
        let hi = self.fragments.partition_point(|(start, _)| *start < req_end);
        if hi == 0 {
            return &[];
        }
        // `running_max_end` is non-decreasing, so the first index whose
        // running max exceeds `offset` is the earliest fragment that could
        // possibly reach into the request — every fragment before it has
        // `end <= offset` and so cannot overlap.
        let lo = self.running_max_end[..hi].partition_point(|&end| end <= offset);
        &self.fragments[lo..hi]
    }

    /// Try to assemble the absolute range `[offset, offset + length)`.
    /// Returns [`Coverage::Full`] with the bytes, or [`Coverage::Gap`] naming
    /// the first uncovered sub-range — identical semantics to the original
    /// (pre-binary-search) linear-scan implementation.
    pub(crate) fn assemble(&self, offset: u64, length: u64) -> Result<Coverage, MdfError> {
        if length == 0 {
            return Ok(Coverage::Full(Vec::new()));
        }
        let req_end = offset.checked_add(length).ok_or_else(|| {
            MdfError::BlockSerializationError(
                "requested byte range offset + length overflows u64".to_string(),
            )
        })?;
        let len = u64_to_usize(length)?;
        let window = self.candidates(offset, req_end);

        // Fast path: a single fragment fully contains the request.
        for (start, bytes) in window {
            let fstart = *start;
            let fend = fragment_end(fstart, bytes.len())?;
            if offset >= fstart && req_end <= fend {
                let s = u64_to_usize(offset - fstart)?;
                return Ok(Coverage::Full(bytes[s..s + len].to_vec()));
            }
        }

        // Assembly path: stitch the request together from overlapping
        // fragments. They are sorted by start, so one sweep finds the first
        // uncovered byte: a fragment starting past the covered prefix leaves a
        // gap no later fragment can fill. (A large read fetched as several
        // concurrent chunks lands here; no per-byte bookkeeping.)
        let mut covered_to = offset;
        for (start, bytes) in window {
            let fstart = *start;
            if fstart > covered_to {
                break;
            }
            covered_to = covered_to.max(fragment_end(fstart, bytes.len())?.min(req_end));
            if covered_to == req_end {
                break;
            }
        }
        if covered_to < req_end {
            return Ok(Coverage::Gap { offset: covered_to, length: req_end - covered_to });
        }
        let mut out = Vec::with_capacity(len);
        for (start, bytes) in window {
            let cursor = offset + out.len() as u64;
            if cursor == req_end {
                break;
            }
            let fend = fragment_end(*start, bytes.len())?;
            if *start <= cursor && fend > cursor {
                let src_lo = u64_to_usize(cursor - start)?;
                let src_hi = u64_to_usize(fend.min(req_end) - start)?;
                out.extend_from_slice(&bytes[src_lo..src_hi]);
            }
        }
        Ok(Coverage::Full(out))
    }
}

/// A [`ByteRangeReader`] that serves absolute `(offset, length)` requests from a
/// caller-provided list of file fragments, each tagged with its absolute file
/// offset. A request is satisfied if the union of fragments fully covers it
/// (assembled across fragments when necessary); otherwise a clear error is
/// returned naming the first uncovered offset.
pub(crate) struct FragmentRangeReader {
    pub(crate) store: FragmentStore,
}

impl ByteRangeReader for FragmentRangeReader {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        match self.store.assemble(offset, length)? {
            Coverage::Full(bytes) => Ok(bytes),
            Coverage::Gap { offset: miss, .. } => Err(MdfError::BlockSerializationError(format!(
                "requested byte range {}..{} is not fully covered by the provided fragments \
                 (missing at offset {})",
                offset,
                offset + length,
                miss
            ))),
        }
    }
}

/// A [`ByteRangeReader`] that drives an incremental, range-fetched index
/// build. It serves reads from the fragments gathered so far (borrowed from a
/// [`FragmentStore`] — see there for the lookup complexity) and **gathers**
/// the ranges it still needs in [`misses`](Self::misses) rather than fetching
/// them on demand. [`is_probing`](ByteRangeReader::is_probing) reports `true`:
/// this is precisely the kind of reader the tolerant metadata walk (see
/// [`crate::parsing::reader_walk`]) is for — it cannot serve every range, so a
/// failed structural read skips just the unreadable subtree instead of
/// aborting the whole pass.
///
/// The key to avoiding an O(N²) fetch-restart loop is that a single walk does
/// not stop at the first miss:
///
/// - An *optional* leaf read (a channel name/unit/comment text block, via
///   [`read_range_optional`](ByteRangeReader::read_range_optional)) records the
///   gap and returns `Ok(None)`, so the walk carries on and collects every
///   other reachable leaf's gap in the same pass.
/// - A *structural* read (a block header whose bytes yield the next link
///   address, via [`read_range`](ByteRangeReader::read_range)) cannot be
///   satisfied with a placeholder, so it records the gap and returns an error
///   — but because this reader is a probe (`is_probing() == true`), the walk
///   only skips the affected subtree rather than aborting entirely, so every
///   other independent gap is still discovered in the same pass.
///
/// The JS driver fetches all recorded ranges (coalesced, with look-ahead) and
/// retries, so a file with N metadata blocks builds in O(passes) ≈ a handful,
/// not O(N). Only metadata blocks are ever requested, so the bulk sample data
/// is never downloaded (unless look-ahead over-fetches interspersed metadata,
/// which the driver accepts to keep the request count low).
pub(crate) struct RecordingRangeReader<'a> {
    pub(crate) store: &'a FragmentStore,
    pub(crate) misses: Vec<(u64, u64)>,
    /// Record the whole request on a miss instead of only its first missing
    /// sub-range. Windowed reads set this: a large data read that is partly
    /// covered by earlier small probes would otherwise come back one hole per
    /// round.
    pub(crate) whole_request: bool,
}

impl<'a> ByteRangeReader for RecordingRangeReader<'a> {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        match self.store.assemble(offset, length)? {
            Coverage::Full(bytes) => Ok(bytes),
            Coverage::Gap { offset: miss, length: needed } => {
                self.misses.push(if self.whole_request { (offset, length) } else { (miss, needed) });
                Err(MdfError::BlockSerializationError(
                    "range not yet available; fetch and retry".to_string(),
                ))
            }
        }
    }

    fn read_range_optional(
        &mut self,
        offset: u64,
        length: u64,
    ) -> Result<Option<Vec<u8>>, MdfError> {
        match self.store.assemble(offset, length)? {
            Coverage::Full(bytes) => Ok(Some(bytes)),
            Coverage::Gap { offset: miss, length: needed } => {
                self.misses.push((miss, needed));
                Ok(None)
            }
        }
    }

    fn is_probing(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Coverage, FragmentStore};

    fn store(fragments: &[(u64, &[u8])]) -> FragmentStore {
        FragmentStore::from_pairs(fragments.iter().map(|(o, b)| (*o, b.to_vec())).collect()).unwrap()
    }

    fn full(s: &FragmentStore, offset: u64, length: u64) -> Option<Vec<u8>> {
        match s.assemble(offset, length).unwrap() {
            Coverage::Full(bytes) => Some(bytes),
            Coverage::Gap { .. } => None,
        }
    }

    fn gap(s: &FragmentStore, offset: u64, length: u64) -> Option<(u64, u64)> {
        match s.assemble(offset, length).unwrap() {
            Coverage::Full(_) => None,
            Coverage::Gap { offset, length } => Some((offset, length)),
        }
    }

    #[test]
    fn assembles_across_adjacent_and_overlapping_fragments() {
        // Chunks of one big read, plus overlapping and spanning fragments (all
        // slices of the same file, so overlaps agree).
        let s = store(&[(10, b"abcd"), (14, b"efgh"), (12, b"cdef"), (18, b"ij"), (0, b"0123456789ab")]);
        assert_eq!(full(&s, 10, 10).unwrap(), b"abcdefghij");
        assert_eq!(full(&s, 8, 6).unwrap(), b"89abcd");
        assert_eq!(full(&s, 15, 3).unwrap(), b"fgh");
        assert_eq!(full(&s, 0, 0).unwrap(), b"");
    }

    #[test]
    fn reports_the_first_missing_byte() {
        let s = store(&[(0, b"0123"), (6, b"67"), (2, b"23")]);
        assert_eq!(gap(&s, 0, 8), Some((4, 4)));
        assert_eq!(gap(&s, 5, 2), Some((5, 2)));
        assert_eq!(gap(&s, 6, 4), Some((8, 2)));
        assert_eq!(gap(&s, 20, 1), Some((20, 1)));
        assert_eq!(full(&s, 6, 2).unwrap(), b"67");
    }
}
