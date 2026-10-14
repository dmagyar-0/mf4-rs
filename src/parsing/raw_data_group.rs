use crate::error::MdfError;
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::blocks::{
    data_block::DataBlock,
    data_group_block::DataGroupBlock,
    common::BlockHeader,
};
use crate::parsing::compression::{collect_fragments, coalesce, Fragment};
use crate::parsing::owned_bytes::{alloc_zeroed, OwnedBytes};
use std::sync::OnceLock;

#[derive(Debug)]
pub struct RawDataGroup {
    pub block: DataGroupBlock,
    pub channel_groups: Vec<RawChannelGroup>,
    /// Owned record bytes for groups whose data cannot be served zero-copy
    /// from the mmap (`##DZ`-compressed fragments, or `##DL` fragments that
    /// split records). Filled on first access and reused afterwards.
    pub(crate) coalesced: OnceLock<OwnedBytes>,
    /// Unsorted groups only: one buffer per channel group, holding that
    /// group's records (record-id prefix kept) or, for VLSD channel groups,
    /// its `[u32 len | payload]` stream. Filled on first access.
    pub(crate) demuxed: OnceLock<Demuxed>,
}

impl RawDataGroup {
    /// Create a data group wrapper (the decompression cache starts empty).
    pub fn new(block: DataGroupBlock, channel_groups: Vec<RawChannelGroup>) -> Self {
        Self { block, channel_groups, coalesced: OnceLock::new(), demuxed: OnceLock::new() }
    }

    /// Size in bytes of one record of this (sorted) data group, or 0 when
    /// it is not uniquely defined.
    pub fn record_size(&self) -> usize {
        match self.channel_groups.as_slice() {
            [cg] => self.block.record_id_len as usize
                + cg.block.samples_byte_nr as usize
                + cg.block.invalidation_bytes_nr as usize,
            _ => 0,
        }
    }

    /// True when several channel groups share this data group's records,
    /// distinguished by a record-id prefix (the "unsorted" layout written by
    /// most bus loggers).
    pub fn is_unsorted(&self) -> bool {
        self.block.record_id_len > 0 && self.channel_groups.len() > 1
    }

    /// Data blocks holding the records of one channel group of this data
    /// group. Works for sorted and **unsorted** data groups; for unsorted
    /// groups the records are demultiplexed once (cached) and returned as a
    /// single block whose records keep their record-id prefix, so record
    /// framing is `record_id_len + samples_byte_nr + invalidation_bytes_nr`
    /// exactly as for sorted groups.
    pub fn data_blocks_for<'a>(
        &'a self,
        cg: &RawChannelGroup,
        mmap: &'a [u8],
    ) -> Result<Vec<DataBlock<'a>>, MdfError> {
        if !self.is_unsorted() {
            return self.data_blocks(mmap);
        }
        let idx = self
            .channel_groups
            .iter()
            .position(|c| std::ptr::eq(c, cg))
            .ok_or_else(|| {
                MdfError::BlockSerializationError(
                    "channel group does not belong to this data group".into(),
                )
            })?;
        let buf = self.demuxed_group(idx, mmap)?;
        Ok(vec![DataBlock {
            header: BlockHeader {
                id: "##DT".to_string(),
                reserved0: 0,
                block_len: 24 + buf.len() as u64,
                links_nr: 0,
            },
            data: buf,
        }])
    }

    /// Demultiplex this unsorted data group once (cached): a single pass
    /// copies fixed-length records per channel group and records the offset
    /// of every VLSD entry — VLSD payloads (the bulk of a bus log) are never
    /// copied and are later read straight from the mmap.
    fn demux<'a>(&'a self, mmap: &'a [u8]) -> Result<&'a Demuxed, MdfError> {
        if let Some(d) = self.demuxed.get() {
            return Ok(d);
        }
        let fragments = collect_fragments(mmap, self.block.data_block_addr, &["##DT", "##DV"])?;
        let (source_mmap, source_owned) = match fragments.as_slice() {
            [] => (Some(0..0), None),
            [Fragment::Raw(r)] => {
                let start = r.as_ptr() as usize - mmap.as_ptr() as usize;
                (Some(start..start + r.len()), None)
            }
            _ => (None, Some(coalesce(&fragments)?)),
        };
        let src: &[u8] = match (&source_mmap, &source_owned) {
            (Some(r), _) => &mmap[r.clone()],
            (None, Some(o)) => &o[..],
            _ => unreachable!(),
        };
        let (fixed, ranges, vlsd, packed) = demux_records(src, self.block.record_id_len, &self.channel_groups)?;
        let _ = self.demuxed.set(Demuxed { source_mmap, source_owned, fixed, ranges, vlsd, packed });
        Ok(self.demuxed.get().expect("just set"))
    }

    /// Fixed-length records of channel group `idx` of an unsorted data group
    /// (record-id prefix kept). See [`Self::data_blocks_for`].
    pub fn demuxed_group<'a>(&'a self, idx: usize, mmap: &'a [u8]) -> Result<&'a [u8], MdfError> {
        let d = self.demux(mmap)?;
        let r = d.ranges.get(idx).ok_or_else(|| {
            MdfError::BlockSerializationError(format!("channel group index {idx} out of range"))
        })?;
        Ok(&d.fixed[r.clone()])
    }

    /// VLSD channel group `idx` of an unsorted data group.
    ///
    /// `Packed(stream)`: the group's `[u32 len | payload]` entries packed
    /// contiguously (same layout as an `##SD` block). `Offsets(src, offs)`:
    /// zero-copy — entry `i` starts at `src[offs[i]]`.
    pub fn vlsd_group<'a>(&'a self, idx: usize, mmap: &'a [u8]) -> Result<VlsdGroup<'a>, MdfError> {
        let d = self.demux(mmap)?;
        if d.packed.get(idx).copied().unwrap_or(false) {
            return Ok(VlsdGroup::Packed(&d.fixed[d.ranges[idx].clone()]));
        }
        let offs = d.vlsd.get(idx).ok_or_else(|| {
            MdfError::BlockSerializationError(format!("channel group index {idx} out of range"))
        })?;
        let src: &'a [u8] = match (&d.source_mmap, &d.source_owned) {
            (Some(r), _) => &mmap[r.clone()],
            (None, Some(o)) => &o[..],
            _ => unreachable!(),
        };
        Ok(VlsdGroup::Offsets(src, offs.as_slice()))
    }

    /// Collect all data blocks referenced by this data group.
    ///
    /// Transparently follows `##DL` chains and `##HL` header lists and
    /// decompresses `##DZ` fragments. Plain, record-aligned data is returned
    /// zero-copy as slices of the mmap. Compressed data, or fragments that
    /// split a record across a boundary, are decoded once into an owned
    /// buffer cached on this group and returned as a single block.
    ///
    /// # Arguments
    /// * `mmap` - Memory mapped file containing the MDF data
    ///
    /// # Returns
    /// A vector of [`DataBlock`] objects or an [`MdfError`] if parsing fails.
    pub fn data_blocks<'a>(
        &'a self,
        mmap: &'a [u8],
    ) -> Result<Vec<DataBlock<'a>>, MdfError> {
        // Unsorted data groups interleave records of several channel groups
        // (each prefixed by a record id). Framing them as fixed-size records
        // of a single group silently mis-decodes the data, so refuse loudly.
        // Metadata access (names, channels) is unaffected — only record/data
        // access goes through here.
        if self.block.record_id_len > 0 && self.channel_groups.len() > 1 {
            return Err(MdfError::BlockSerializationError(
                "unsorted data groups (multiple channel groups per data group) are not supported"
                    .to_string(),
            ));
        }

        let synthetic = |data: &'a [u8]| DataBlock {
            header: BlockHeader {
                id: "##DT".to_string(),
                reserved0: 0,
                block_len: 24 + data.len() as u64,
                links_nr: 0,
            },
            data,
        };

        if let Some(buf) = self.coalesced.get() {
            return Ok(vec![synthetic(&buf[..])]);
        }

        let fragments = collect_fragments(mmap, self.block.data_block_addr, &["##DT", "##DV"])?;
        let record_size = self.record_size();
        let zero_copy = fragments.iter().all(|f| match f {
            Fragment::Raw(r) => record_size == 0 || r.len() % record_size == 0,
            Fragment::Dz(_) => false,
        });
        // A single trailing partial record in the last plain fragment is
        // tolerated (readers trim it), matching the previous behaviour.
        let only_tail_misaligned = record_size > 0
            && !fragments.is_empty()
            && fragments.iter().all(|f| matches!(f, Fragment::Raw(_)))
            && fragments[..fragments.len() - 1].iter().all(|f| match f {
                Fragment::Raw(r) => r.len() % record_size == 0,
                Fragment::Dz(_) => false,
            });

        if zero_copy || only_tail_misaligned {
            return Ok(fragments
                .into_iter()
                .map(|f| match f {
                    Fragment::Raw(r) => synthetic(r),
                    Fragment::Dz(_) => unreachable!("zero-copy path has no DZ"),
                })
                .collect());
        }

        let buf = coalesce(&fragments)?;
        let _ = self.coalesced.set(buf);
        Ok(vec![synthetic(&self.coalesced.get().expect("just set")[..])])
    }
}

/// Payload access for one VLSD channel group of an unsorted data group.
pub enum VlsdGroup<'a> {
    Packed(&'a [u8]),
    Offsets(&'a [u8], &'a [u64]),
}

/// Demultiplexed view of an unsorted data group.
#[derive(Debug)]
pub(crate) struct Demuxed {
    /// Record stream borrowed from the mmap (single plain fragment) ...
    source_mmap: Option<std::ops::Range<usize>>,
    /// ... or owned (compressed / multi-fragment streams).
    source_owned: Option<OwnedBytes>,
    /// Fixed-length records of every non-VLSD channel group, concatenated.
    fixed: OwnedBytes,
    /// Per channel group: its region in `fixed` (empty for VLSD groups).
    ranges: Vec<std::ops::Range<usize>>,
    /// Per channel group: offsets of its `[u32 len | payload]` entries in
    /// the record stream (only for VLSD groups that are *not* packed).
    vlsd: Vec<Vec<u64>>,
    /// Per channel group: VLSD group whose `[u32 len | payload]` entries were
    /// packed into `fixed` (small payload groups, e.g. CAN FD data bytes).
    packed: Vec<bool>,
}

/// Owned fixed-record buffer, per-group ranges, VLSD offsets, packed flags.
type DemuxOut = (OwnedBytes, Vec<std::ops::Range<usize>>, Vec<Vec<u64>>, Vec<bool>);

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Fixed(usize),
    /// VLSD group; `true` = pack payloads into the owned buffer.
    Vlsd(bool),
}

/// VLSD groups whose declared payload total is at most this are packed into
/// the owned buffer during the scan (compact, cache-friendly reads); larger
/// ones (e.g. Ethernet payloads) stay zero-copy as offsets into the stream.
/// Override with `MF4RS_VLSD_PACK_BYTES`.
fn vlsd_pack_limit() -> u64 {
    std::env::var("MF4RS_VLSD_PACK_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512 << 20)
}

/// Single-pass demultiplexer for an unsorted record stream.
///
/// Fixed-length groups are copied (record id kept) into one buffer sized
/// from the channel-group headers (`cycles_nr`); VLSD groups only get entry
/// offsets. If the headers are wrong or zero (unfinalized files) a measuring
/// pass sizes the buffer and the copy is redone. The buffer spills to a
/// temp-file mapping when large (see [`alloc_zeroed`]).
///
/// An unknown record id or an entry overrunning the data is an error: the
/// stream cannot be re-synchronised safely after either.
pub(crate) fn demux_records(
    src: &[u8],
    record_id_len: u8,
    groups: &[RawChannelGroup],
) -> Result<DemuxOut, MdfError> {
    let rid = record_id_len as usize;
    if !matches!(rid, 1 | 2 | 4 | 8) {
        return Err(MdfError::BlockSerializationError(format!(
            "invalid record id length {rid}"
        )));
    }
    let mut kinds = Vec::with_capacity(groups.len());
    let mut by_id: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for (i, g) in groups.iter().enumerate() {
        let b = &g.block;
        kinds.push(if b.flags & 0x01 != 0 {
            // VLSD CG: cg_data_bytes + cg_inval_bytes form the u64 total.
            let total = b.samples_byte_nr as u64 | ((b.invalidation_bytes_nr as u64) << 32);
            Kind::Vlsd(total <= vlsd_pack_limit() && total as usize <= src.len())
        } else {
            Kind::Fixed(rid + b.samples_byte_nr as usize + b.invalidation_bytes_nr as usize)
        });
        if by_id.insert(b.record_id, i).is_some() {
            return Err(MdfError::BlockSerializationError(format!(
                "duplicate record id {} in unsorted data group",
                b.record_id
            )));
        }
    }
    // Small ids (the norm) resolve through a dense table, not a hash map.
    let max_id = by_id.keys().copied().max().unwrap_or(0);
    let dense: Option<Vec<u32>> = (max_id < 65_536).then(|| {
        let mut t = vec![u32::MAX; max_id as usize + 1];
        for (&id, &gi) in &by_id {
            t[id as usize] = gi as u32;
        }
        t
    });
    let lookup = |id: u64| -> Option<usize> {
        match &dense {
            Some(t) => t.get(id as usize).copied().filter(|&g| g != u32::MAX).map(|g| g as usize),
            None => by_id.get(&id).copied(),
        }
    };
    let read_id = |p: usize| -> u64 {
        match rid {
            1 => src[p] as u64,
            2 => u16::from_le_bytes([src[p], src[p + 1]]) as u64,
            4 => u32::from_le_bytes(src[p..p + 4].try_into().unwrap()) as u64,
            _ => u64::from_le_bytes(src[p..p + 8].try_into().unwrap()),
        }
    };

    // One scan. `copy_cb(gi, pos, len)` for every region to copy (fixed
    // records, packed VLSD entries incl. their u32 length); `vlsd_cb(gi, pos)`
    // per zero-copy VLSD entry (pos = offset of its u32 length).
    let scan = |copy_cb: &mut dyn FnMut(usize, usize, usize), vlsd_cb: &mut dyn FnMut(usize, usize)| -> Result<(), MdfError> {
        let mut pos = 0usize;
        while pos + rid <= src.len() {
            let id = read_id(pos);
            let gi = lookup(id).ok_or_else(|| {
                MdfError::BlockSerializationError(format!(
                    "unknown record id {id} at data offset {pos} in unsorted data group"
                ))
            })?;
            match kinds[gi] {
                Kind::Fixed(len) => {
                    if pos + len > src.len() {
                        break; // trailing partial record
                    }
                    copy_cb(gi, pos, len);
                    pos += len;
                }
                Kind::Vlsd(pack) => {
                    let lp = pos + rid;
                    if lp + 4 > src.len() {
                        break;
                    }
                    let len = u32::from_le_bytes(src[lp..lp + 4].try_into().unwrap()) as usize;
                    if lp + 4 + len > src.len() {
                        return Err(MdfError::BlockSerializationError(format!(
                            "VLSD record at data offset {pos} overruns the data ({len} bytes)"
                        )));
                    }
                    if pack {
                        copy_cb(gi, lp, 4 + len);
                    } else {
                        vlsd_cb(gi, lp);
                    }
                    pos = lp + 4 + len;
                }
            }
        }
        Ok(())
    };

    let layout = |sizes: &[usize]| -> Vec<std::ops::Range<usize>> {
        let mut acc = 0usize;
        sizes.iter().map(|&n| { let r = acc..acc + n; acc += n; r }).collect()
    };

    // Sizes declared by the headers, if plausible.
    let declared: Vec<usize> = groups
        .iter()
        .zip(&kinds)
        .map(|(g, k)| match k {
            Kind::Fixed(len) => (g.block.cycles_nr as usize).saturating_mul(*len),
            // packed VLSD: entries are [u32 len | payload], i.e. total + 4/entry
            Kind::Vlsd(true) => ((g.block.samples_byte_nr as u64 | ((g.block.invalidation_bytes_nr as u64) << 32)) as usize)
                .saturating_add((g.block.cycles_nr as usize).saturating_mul(4)),
            Kind::Vlsd(false) => 0,
        })
        .collect();
    let declared_total = declared.iter().fold(0usize, |a, &b| a.saturating_add(b));
    let mut sizes = if declared_total <= src.len() { declared } else { vec![0; groups.len()] };

    let mut vlsd: Vec<Vec<u64>> = groups
        .iter()
        .zip(&kinds)
        .map(|(g, k)| match k {
            Kind::Vlsd(false) => Vec::with_capacity((g.block.cycles_nr as usize).min(src.len() / 4)),
            _ => Vec::new(),
        })
        .collect();

    for attempt in 0..2 {
        let ranges = layout(&sizes);
        let total = ranges.last().map_or(0, |r| r.end);
        let mut out = alloc_zeroed(total)?;
        let mut cursor: Vec<usize> = ranges.iter().map(|r| r.start).collect();
        let mut measured = vec![0usize; groups.len()];
        let mut overflow = false;
        for v in vlsd.iter_mut() {
            v.clear();
        }
        scan(
            &mut |gi, pos, len| {
                measured[gi] += len;
                if cursor[gi] + len <= ranges[gi].end {
                    out[cursor[gi]..cursor[gi] + len].copy_from_slice(&src[pos..pos + len]);
                    cursor[gi] += len;
                } else {
                    overflow = true;
                }
            },
            &mut |gi, lp| vlsd[gi].push(lp as u64),
        )?;
        if !overflow {
            // Headers may over-declare (truncated logs): trim to what was written.
            let ranges = ranges.iter().zip(&cursor).map(|(r, &c)| r.start..c).collect();
            let packed = kinds.iter().map(|k| *k == Kind::Vlsd(true)).collect();
            return Ok((out, ranges, vlsd, packed));
        }
        debug_assert_eq!(attempt, 0);
        sizes = measured;
    }
    Err(MdfError::BlockSerializationError("unsorted demultiplexing did not converge".into()))
}

