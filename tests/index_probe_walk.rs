//! Regression test for the tolerant, batched gap-discovery metadata walk
//! ([`ByteRangeReader::is_probing`]).
//!
//! Before this fix, `reader_walk::walk` treated every structural read (##DG /
//! ##CG / ##CN block bytes) as fatal: the first missing range aborted the
//! *entire* pass with an `Err`, even when the walk already knew about (and
//! could have kept discovering) other, independent gaps elsewhere in the
//! file. For a file with many channel groups this meant one network
//! round-trip per group instead of a handful for the whole file.
//!
//! The fix lets a **probing** reader (`is_probing() == true`) skip just the
//! unreadable subtree — the rest of one channel group's channels, or one data
//! group's remaining channel groups — instead of aborting the whole walk, so
//! a single pass reports the frontier gap of *every* independent subtree at
//! once. Readers that are not probes (the trait's default, `is_probing() ==
//! false` — local files, plain HTTP, `CachingRangeReader`) are completely
//! unaffected: a failed read stays fatal, exactly as before.
//!
//! This test builds a fixture whose `##DG`/`##CG` headers for every group are
//! clustered near the front of the file (so the whole group chain is
//! discoverable from a small seed) while each group's `##CN` channel blocks
//! and `##DT` data sit later, scattered behind the *other* groups' clusters —
//! adapted from the scattering recipe in
//! `tests/cloud_index.rs::cloud_index_handles_scattered_metadata`. Seeding
//! the probe with only the first metadata cluster and running **one** pass
//! must report misses spanning more than one group, proving the walk no
//! longer stops at the first gap.

use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::index::{ByteRangeReader, MdfIndex};
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

const GROUPS: usize = 6;
const CHANNELS_PER_GROUP: usize = 4; // 1 master + 3 data, all f64
const RECORDS: usize = 50;

/// A [`ByteRangeReader`] that serves reads only from a fixed set of
/// `(offset, bytes)` fragments it was seeded with, and records every miss
/// instead of fetching on demand.
///
/// `probing` controls [`ByteRangeReader::is_probing`]: `true` mirrors the
/// wasm `RecordingRangeReader` (the tolerant, batching walk); `false` proves
/// the on-demand/native behavior is unchanged (fatal on the first miss).
struct FragReader {
    fragments: Vec<(u64, Vec<u8>)>,
    misses: Vec<(u64, u64)>,
    probing: bool,
}

impl FragReader {
    fn seeded(fragments: Vec<(u64, Vec<u8>)>, probing: bool) -> Self {
        Self { fragments, misses: Vec::new(), probing }
    }

    /// Assemble `[offset, offset+length)` from the stored fragments, or
    /// report the first uncovered sub-range.
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

impl ByteRangeReader for FragReader {
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

    fn is_probing(&self) -> bool {
        self.probing
    }
}

/// Fixture layout: all `##DG`/`##CG` (+ name/comment `##TX`) blocks for every
/// group are written first (Phase 1), clustered near the front of the file.
/// Only afterwards (Phase 2) does each group get its `##CN` channels and its
/// `##DT` data, group by group — so group *g*'s channel blocks sit behind
/// every earlier group's data. `seed_end` is the exact byte length of Phase 1
/// (via `MdfWriter::offset()`), so a seed of `bytes[0..seed_end]` covers every
/// group's `##DG`/`##CG` but none of their channels or data.
struct Fixture {
    bytes: Vec<u8>,
    file_size: u64,
    seed_end: u64,
    /// File offset of each group's first (master/time) channel block —
    /// used to check which group a recorded miss belongs to.
    first_channel_addr: Vec<u64>,
}

fn build_fixture() -> Fixture {
    // A per-test temp directory (rather than a fixed path derived from the
    // process id) avoids collisions between tests running in parallel
    // threads within the same test binary.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("probe_walk_fixture.mf4");
    let path = path.to_str().unwrap();
    let mut w = MdfWriter::new(path).unwrap();
    w.init_mdf_file().unwrap();

    // Phase 1: create every group's ##DG + ##CG + name/comment ##TX blocks,
    // clustering all structural headers near the front of the file.
    let mut cg_ids = Vec::with_capacity(GROUPS);
    for g in 0..GROUPS {
        let cg_id = w.add_channel_group(None, |_| {}).unwrap();
        w.set_channel_group_name(&cg_id, &format!("Group {g}")).unwrap();
        w.set_channel_group_comment(&cg_id, &format!("Comment {g}")).unwrap();
        cg_ids.push(cg_id);
    }
    let seed_end = w.offset();

    // Phase 2: for each group (in order), add its channels and write its
    // data block — scattering each group's ##CN cluster behind every
    // earlier group's ##DT block.
    let mut first_channel_addr = Vec::with_capacity(GROUPS);
    for (g, cg_id) in cg_ids.iter().enumerate() {
        let time_id = w
            .add_channel(cg_id, None, |ch| {
                ch.data_type = DataType::FloatLE;
                ch.bit_count = 64;
                ch.name = Some(format!("t_{g}"));
            })
            .unwrap();
        w.set_time_channel(&time_id).unwrap();
        first_channel_addr.push(w.get_block_position(&time_id).unwrap());

        let mut prev = time_id;
        for c in 1..CHANNELS_PER_GROUP {
            prev = w
                .add_channel(cg_id, Some(&prev), |ch| {
                    ch.data_type = DataType::FloatLE;
                    ch.bit_count = 64;
                    ch.name = Some(format!("ch_{g}_{c}"));
                })
                .unwrap();
        }

        w.start_data_block_for_cg(cg_id, 0).unwrap();
        for r in 0..RECORDS {
            let mut record = Vec::with_capacity(CHANNELS_PER_GROUP);
            for k in 0..CHANNELS_PER_GROUP {
                record.push(DecodedValue::Float((g * 1000 + r * 10 + k) as f64));
            }
            w.write_record(cg_id, &record).unwrap();
        }
        w.finish_data_block(cg_id).unwrap();
    }
    w.finalize().unwrap();

    let bytes = std::fs::read(path).unwrap();
    let _ = std::fs::remove_file(path);
    let file_size = bytes.len() as u64;

    Fixture { bytes, file_size, seed_end, first_channel_addr }
}

/// A single probing pass, seeded with only the first metadata cluster,
/// reports misses spanning more than one group — the walk no longer stops at
/// the first gap.
#[test]
fn probing_walk_batches_gaps_across_groups() {
    let fx = build_fixture();
    assert!(
        fx.seed_end < fx.file_size,
        "fixture too small to scatter channels/data behind the seed"
    );

    let seed = fx.bytes[..fx.seed_end as usize].to_vec();
    let mut reader = FragReader::seeded(vec![(0, seed)], /* probing = */ true);

    let result = MdfIndex::from_range_reader(&mut reader, fx.file_size);

    // The whole point of the fix: the pass does not abort with an error just
    // because most of the file's channels/data are still missing.
    assert!(
        result.is_ok(),
        "a probing reader must not abort the walk on a structural miss: {:?}",
        result.err()
    );
    assert!(
        !reader.misses.is_empty(),
        "expected the pass to record misses for the not-yet-fetched channels/data"
    );

    // Count how many distinct groups have a recorded miss covering their
    // first channel's address — i.e. how many groups' metadata the single
    // pass made progress on discovering, not just the first one.
    let groups_with_miss = fx
        .first_channel_addr
        .iter()
        .filter(|&&addr| {
            reader
                .misses
                .iter()
                .any(|&(off, len)| off <= addr && addr < off + len)
        })
        .count();

    assert!(
        groups_with_miss > 1,
        "expected misses spanning more than one group's metadata in a single pass, got {} \
         (misses: {:?})",
        groups_with_miss,
        reader.misses
    );
    // With this fixture every group's ##DG/##CG is in the seed but every
    // group's first channel is scattered behind data, so all of them should
    // miss in this one pass.
    assert_eq!(
        groups_with_miss, GROUPS,
        "expected every group's first channel to miss in the first pass"
    );
}

/// Feeding the recorded misses back in rounds converges to an index
/// identical (group/channel names and counts) to one built directly from the
/// full bytes via `MdfIndex::from_bytes`.
#[test]
fn probing_walk_converges_to_full_index() {
    let fx = build_fixture();
    let seed = fx.bytes[..fx.seed_end as usize].to_vec();
    let mut fragments = vec![(0u64, seed)];

    let mut built: Option<MdfIndex> = None;
    for _round in 0..200 {
        let mut reader = FragReader::seeded(fragments.clone(), true);
        let result = MdfIndex::from_range_reader(&mut reader, fx.file_size);
        if reader.misses.is_empty() {
            built = Some(result.expect("walk with no misses must succeed"));
            break;
        }
        assert!(
            result.is_ok() || !reader.misses.is_empty(),
            "a probing reader's failed pass must always record a miss"
        );
        for (off, len) in reader.misses {
            let off_u = off as usize;
            let end_u = (off + len) as usize;
            fragments.push((off, fx.bytes[off_u..end_u].to_vec()));
        }
    }

    let built = built.expect("probing build did not converge within the round budget");
    let direct = MdfIndex::from_bytes(fx.bytes.clone()).unwrap();

    assert_eq!(built.channel_groups.len(), direct.channel_groups.len());
    assert_eq!(built.channel_names(), direct.channel_names());
    assert_eq!(built.channel_names().len(), GROUPS * CHANNELS_PER_GROUP);
    for (bg, dg) in built.channel_groups.iter().zip(direct.channel_groups.iter()) {
        assert_eq!(bg.name, dg.name);
        assert_eq!(bg.channels.len(), dg.channels.len());
        assert_eq!(bg.record_count, dg.record_count);
    }
}

/// Regression guard: a reader that is *not* a probe (`is_probing() ==
/// false`, the trait default) must still abort the whole walk on the first
/// missing range — native/on-demand readers (local files, plain HTTP) keep
/// today's fatal-on-first-miss behavior unchanged.
#[test]
fn non_probing_reader_still_fails_hard_on_missing_range() {
    let fx = build_fixture();
    let seed = fx.bytes[..fx.seed_end as usize].to_vec();
    let mut reader = FragReader::seeded(vec![(0, seed)], /* probing = */ false);

    let result = MdfIndex::from_range_reader(&mut reader, fx.file_size);

    assert!(
        result.is_err(),
        "a non-probing reader must fail the whole walk on a missing structural range"
    );
    // Exactly one miss: the walk aborted at the very first unreadable read
    // instead of continuing to discover the other groups' gaps.
    assert_eq!(
        reader.misses.len(),
        1,
        "a non-probing reader must abort after the first miss, not batch further gaps: {:?}",
        reader.misses
    );
}
