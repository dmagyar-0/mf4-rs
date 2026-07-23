//! Regression tests for the incremental (range-fetched) index build.
//!
//! The wasm `MdfIndex.buildIndexStep` used to restart the metadata walk from
//! scratch and abort on the *first* missing byte range, so a file with N
//! metadata blocks needed O(N) fetch-restart round-trips and O(N²) CPU. For a
//! ~3000-channel file that meant thousands of requests and many seconds.
//!
//! The fix makes a single walk *gather* every range it still needs
//! ([`ByteRangeReader::read_range_optional`] records a gap and lets the walk
//! continue past optional leaf blocks), so the driver fetches them in a batch
//! and the build converges in a handful of passes. These tests exercise that
//! behaviour natively (no wasm toolchain) via a reader that mirrors the wasm
//! `RecordingRangeReader`, asserting both the request-count bound and that the
//! incrementally built index is identical to a direct `from_bytes` build.

use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::index::{ByteRangeReader, MdfIndex};
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

/// A reader that serves reads from accumulated fragments and *gathers* the
/// ranges it still needs instead of fetching on demand — the native mirror of
/// the wasm `RecordingRangeReader`.
struct CollectingReader {
    fragments: Vec<(u64, Vec<u8>)>,
    misses: Vec<(u64, u64)>,
}

impl CollectingReader {
    /// First still-uncovered sub-range of `[offset, offset+length)`, or `None`
    /// if fully covered; assembles the covered bytes when it is.
    fn cover(&self, offset: u64, length: u64) -> Result<Vec<u8>, (u64, u64)> {
        let req_end = offset + length;
        let len = length as usize;
        let mut out = vec![0u8; len];
        let mut covered = vec![false; len];
        for (start, bytes) in &self.fragments {
            let fstart = *start;
            let fend = fstart + bytes.len() as u64;
            let lo = offset.max(fstart);
            let hi = req_end.min(fend);
            if lo < hi {
                let dlo = (lo - offset) as usize;
                let dhi = (hi - offset) as usize;
                let slo = (lo - fstart) as usize;
                out[dlo..dhi].copy_from_slice(&bytes[slo..slo + (dhi - dlo)]);
                for c in &mut covered[dlo..dhi] {
                    *c = true;
                }
            }
        }
        match covered.iter().position(|&c| !c) {
            None => Ok(out),
            Some(pos) => {
                let miss = offset + pos as u64;
                Err((miss, req_end - miss))
            }
        }
    }
}

impl ByteRangeReader for CollectingReader {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        match self.cover(offset, length) {
            Ok(bytes) => Ok(bytes),
            Err(gap) => {
                self.misses.push(gap);
                Err(MdfError::BlockSerializationError("gap".into()))
            }
        }
    }

    fn read_range_optional(
        &mut self,
        offset: u64,
        length: u64,
    ) -> Result<Option<Vec<u8>>, MdfError> {
        match self.cover(offset, length) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(gap) => {
                self.misses.push(gap);
                Ok(None)
            }
        }
    }
}

/// Merge overlapping/adjacent ranges, bridging gaps up to `bridge` bytes.
fn merge_ranges(mut ranges: Vec<(u64, u64)>, bridge: u64) -> Vec<(u64, u64)> {
    ranges.sort_by_key(|r| r.0);
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (off, len) in ranges {
        let end = off + len;
        if let Some(last) = out.last_mut() {
            if off <= last.0 + last.1 + bridge {
                last.1 = last.1.max(end - last.0);
                continue;
            }
        }
        out.push((off, len));
    }
    out
}

/// Outcome of an incremental build: the index plus request/byte accounting.
struct BuildStats {
    index: MdfIndex,
    reads: usize,
    bytes_read: u64,
}

/// Drive an incremental build over `bytes`, mirroring the JS `fromRangeSource`
/// loop (seed prefix + geometric look-ahead + gap coalescing).
fn incremental_build(bytes: &[u8]) -> BuildStats {
    const SEED: u64 = 256 * 1024;
    const BASE_LA: u64 = 64 * 1024;
    const MAX_LA: u64 = 4 * 1024 * 1024;

    let size = bytes.len() as u64;
    let mut fragments: Vec<(u64, Vec<u8>)> = Vec::new();
    let mut reads = 0usize;
    let mut bytes_read = 0u64;
    let mut fetch = |fragments: &mut Vec<(u64, Vec<u8>)>, off: u64, len: u64| {
        let clamped = len.min(size - off);
        fragments.push((off, bytes[off as usize..(off + clamped) as usize].to_vec()));
        reads += 1;
        bytes_read += clamped;
    };

    if size > 0 {
        fetch(&mut fragments, 0, SEED.min(size));
    }

    for round in 0..500usize {
        let mut reader = CollectingReader { fragments: fragments.clone(), misses: Vec::new() };
        let res = MdfIndex::from_range_reader(&mut reader, size);
        // Done only when the walk succeeded *and* nothing was skipped this pass.
        if res.is_ok() && reader.misses.is_empty() {
            return BuildStats { index: res.unwrap(), reads, bytes_read };
        }
        assert!(
            !reader.misses.is_empty(),
            "walk failed without recording a gap: {:?}",
            res.err()
        );
        let la = (BASE_LA.saturating_mul(1u64 << round.min(20))).min(MAX_LA);
        for (off, len) in merge_ranges(reader.misses, la) {
            fetch(&mut fragments, off, len.max(la).min(size - off));
        }
    }
    panic!("incremental build did not converge");
}

fn write_many_channel_file(groups: usize, records: usize) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!("mf4_inc_many_{groups}_{records}.mf4"));
    let path = path.to_str().unwrap();
    let mut w = MdfWriter::new(path).unwrap();
    w.init_mdf_file().unwrap();
    for g in 0..groups {
        let cg = w.add_channel_group(None, |_| {}).unwrap();
        let time = w
            .add_channel(&cg, None, |c| {
                c.data_type = DataType::FloatLE;
                c.bit_count = 64;
                c.name = Some(format!("Time_{g}"));
            })
            .unwrap();
        w.set_time_channel(&time).unwrap();
        let mut prev = time;
        for c in 0..15 {
            prev = w
                .add_channel(&cg, Some(&prev), |ch| {
                    ch.data_type = DataType::FloatLE;
                    ch.bit_count = 32;
                    ch.name = Some(format!("Signal_{g}_{c}"));
                })
                .unwrap();
        }
        w.start_data_block_for_cg(&cg, 0).unwrap();
        for i in 0..records {
            let mut vals = Vec::with_capacity(16);
            vals.push(DecodedValue::Float(i as f64 * 0.01));
            for _ in 0..15 {
                vals.push(DecodedValue::Float((i as f64 * 0.1).sin()));
            }
            w.write_record(&cg, &vals).unwrap();
        }
        w.finish_data_block(&cg).unwrap();
    }
    w.finalize().unwrap();
    let out = std::fs::read(path).unwrap();
    let _ = std::fs::remove_file(path);
    out
}

fn write_single_group_file(records: usize) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!("mf4_inc_single_{records}.mf4"));
    let path = path.to_str().unwrap();
    let mut w = MdfWriter::new(path).unwrap();
    w.init_mdf_file().unwrap();
    let cg = w.add_channel_group(None, |_| {}).unwrap();
    let time = w
        .add_channel(&cg, None, |c| {
            c.data_type = DataType::FloatLE;
            c.bit_count = 64;
            c.name = Some("Time".into());
        })
        .unwrap();
    w.set_time_channel(&time).unwrap();
    let v = w
        .add_channel(&cg, Some(&time), |c| {
            c.data_type = DataType::FloatLE;
            c.bit_count = 64;
            c.name = Some("Value".into());
        })
        .unwrap();
    w.add_channel(&cg, Some(&v), |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Extra".into());
    })
    .unwrap();
    w.start_data_block_for_cg(&cg, 0).unwrap();
    for i in 0..records {
        w.write_record(
            &cg,
            &[
                DecodedValue::Float(i as f64 * 0.01),
                DecodedValue::Float(i as f64),
                DecodedValue::Float(i as f64 * 2.0),
            ],
        )
        .unwrap();
    }
    w.finish_data_block(&cg).unwrap();
    w.finalize().unwrap();
    let out = std::fs::read(path).unwrap();
    let _ = std::fs::remove_file(path);
    out
}

/// A many-channel file (metadata interleaved with data throughout the file) is
/// the pathological case the old walk turned into thousands of requests. The
/// gather-all-misses walk must build it in a small, bounded number of reads.
#[test]
fn many_channel_build_is_bounded_and_correct() {
    let bytes = write_many_channel_file(185, 500);
    let stats = incremental_build(&bytes);

    // Correctness: identical to a direct build, with every name resolved.
    let eager = MdfIndex::from_bytes(bytes.clone()).unwrap();
    assert_eq!(stats.index.channel_names(), eager.channel_names());
    assert_eq!(stats.index.channel_names().len(), 185 * 16);
    assert!(stats.index.channel_names().iter().all(|n| !n.is_empty()));

    // The whole point of the fix: a handful of requests, not one per block.
    // (~2960 channels ⇒ ~5500 metadata blocks; the old code needed one round
    // per block.)
    assert!(
        stats.reads < 50,
        "expected < 50 reads, got {} for a {}-channel file",
        stats.reads,
        185 * 16
    );
}

/// A single-group file has its metadata contiguous at the front, so the build
/// must stay truly metadata-only (never pull the bulk sample data).
#[test]
fn contiguous_metadata_build_fetches_little() {
    let bytes = write_single_group_file(300_000);
    let stats = incremental_build(&bytes);

    let eager = MdfIndex::from_bytes(bytes.clone()).unwrap();
    assert_eq!(stats.index.channel_names(), eager.channel_names());

    // Metadata sits at the front; the build should read a small fraction only.
    assert!(
        (stats.bytes_read as f64) < (bytes.len() as f64) * 0.15,
        "fetched {} of {} bytes ({:.1}%), expected < 15%",
        stats.bytes_read,
        bytes.len(),
        stats.bytes_read as f64 / bytes.len() as f64 * 100.0
    );
}
