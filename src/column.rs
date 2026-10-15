//! Convert any MDF 4 file into a **column-oriented** MDF 4.20 file that is
//! optimised for reading a single signal from cloud storage.
//!
//! # Output layout
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────┐
//! │ ##ID  ##HD                                                   │  metadata:
//! │ ##FH / ##MD history, then per output group:                  │  one contiguous
//! │   ##DG ##CG ##CN (+ ##TX/##MD/##CC/##SI they reference)      │  region at the
//! │   ##DL listing the signal's data blocks                      │  front
//! ├──────────────────────────────────────────────────────────────┤
//! │ time 0:     ##DV ##DV ##DV …   (contiguous)                  │
//! │ signal A:   ##DV ##DV …        (contiguous)                  │  data: every
//! │ signal B:   ##DT …  ##SD       (VLSD stream right after)     │  signal's blocks
//! │ time 1 …                                                     │  back to back
//! ├──────────────────────────────────────────────────────────────┤
//! │ ##AT attachments (if any)                                    │
//! └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! * Every signal lives in its **own data group / channel group** holding just
//!   that one channel ("column storage"). Its samples are written as a run of
//!   data blocks of about [`ColumnOptions::block_size`] bytes, placed directly
//!   after each other and listed by an equal-length `##DL` block, so one HTTP
//!   range request fetches the whole signal.
//! * Timestamps are **shared** using the MDF 4.2 *remote master* feature: the
//!   master (time) channel of each source group is written once into its own
//!   group, and every signal group points at it via `cg_cg_master` (channel
//!   group flag bit 3). With [`ColumnOptions::share_identical_time`] source
//!   groups whose time axes are byte-identical share one time group.
//! * Every non-data block sits in one region at the front of the file, so a
//!   reader learns the whole structure from a single prefix fetch. Since the
//!   data-block sizes are all derivable from the metadata (equal-length `##DL`
//!   lists plus the exact cycle counts), an index build never has to touch the
//!   data region.
//!
//! # Streaming
//!
//! The output layout is fully computed from the source metadata before any
//! sample is copied. The source data is then streamed **once** (in
//! [`ColumnOptions::read_chunk_size`] pieces) and each channel's bytes are
//! appended to a small per-channel buffer that is flushed straight to its
//! pre-computed position in the output. Memory stays bounded by
//! [`ColumnOptions::memory_budget`] regardless of file size; nothing is
//! decoded, so the conversion is lossless (raw bytes, conversions, units,
//! comments and sources are carried over).
//!
//! # Supported input
//!
//! Sorted and unsorted data groups, `##DT`/`##DV`/`##DL`/`##HL` data chains,
//! invalidation bits, VLSD (`##SD`) channels, sources, conversions (any
//! chain), file history and attachments. Not supported: compressed `##DZ`
//! blocks (rejected with an error). Channels that cannot be represented as a
//! single column — array (`##CA`) compositions and VLSD channels stored in a
//! VLSD channel group — are skipped and listed in
//! [`ColumnReport::skipped_channels`].

use std::collections::{HashMap, HashSet};
use std::hash::Hasher;

use crate::blocks::common::{read_string_block_via_reader, BlockHeader};
use crate::blocks::data_list_block::DataListBlock;
use crate::blocks::identification_block::IdentificationBlock;
use crate::error::MdfError;
use crate::index::ByteRangeReader;

/// Default payload size of one data block (4 MiB).
pub const DEFAULT_BLOCK_SIZE: u64 = 4 * 1024 * 1024;
/// Default cap on the memory used by the per-channel write buffers (64 MiB).
pub const DEFAULT_MEMORY_BUDGET: u64 = 64 * 1024 * 1024;
/// Default size of each sequential read from the source (4 MiB).
pub const DEFAULT_READ_CHUNK: u64 = 4 * 1024 * 1024;

/// Channel-group flag: the group stores VLSD data for another group's channel.
const CG_FLAG_VLSD: u16 = 1;
pub use crate::blocks::channel_group_block::CG_FLAG_REMOTE_MASTER;
const CN_FLAG_ALL_INVALID: u32 = 1;
const CN_FLAG_INVAL_BIT_VALID: u32 = 1 << 1;
const CN_FLAG_DEFAULT_X: u32 = 1 << 12;
/// Upper bound for a single metadata block (sanity check against corrupt links).
const MAX_METADATA_BLOCK: u64 = 256 * 1024 * 1024;
/// Smallest per-channel write buffer, whatever the memory budget.
const MIN_STREAM_BUFFER: usize = 4096;
/// Header of every MDF block.
const HDR: u64 = 24;

/// Tuning for [`to_column_oriented`] / [`to_column_oriented_with`].
#[derive(Debug, Clone)]
pub struct ColumnOptions {
    /// Target payload size of one data block, in bytes. Each block holds a
    /// whole number of records; a signal larger than this is split into
    /// several equally sized blocks written back to back and listed by a
    /// `##DL` block. Default [`DEFAULT_BLOCK_SIZE`] (4 MiB).
    pub block_size: u64,
    /// Upper bound for the per-channel write buffers in bytes (each channel
    /// gets `memory_budget / channels_in_group`, at least 4 KiB and at most
    /// one block). Default [`DEFAULT_MEMORY_BUDGET`] (64 MiB).
    pub memory_budget: u64,
    /// Size of each sequential read from the source. Default 4 MiB.
    pub read_chunk_size: u64,
    /// Merge time channels of different source groups when they hold exactly
    /// the same raw timestamps (same layout, conversion and bytes), so all
    /// their signals reference a single time group. Costs one extra read of
    /// the candidate groups' data. Default `true`.
    pub share_identical_time: bool,
}

impl Default for ColumnOptions {
    fn default() -> Self {
        Self {
            block_size: DEFAULT_BLOCK_SIZE,
            memory_budget: DEFAULT_MEMORY_BUDGET,
            read_chunk_size: DEFAULT_READ_CHUNK,
            share_identical_time: true,
        }
    }
}

/// Summary of a column-oriented conversion.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ColumnReport {
    /// Channel groups found in the source (excluding VLSD storage groups).
    pub source_groups: usize,
    /// Time (master) groups written.
    pub time_groups: usize,
    /// Signal groups written (one per signal).
    pub signal_groups: usize,
    /// Source groups whose time axis was identical to an earlier group's and
    /// therefore shares that group's time channel.
    pub shared_time_groups: usize,
    /// Data blocks written (`##DV`/`##DT`/`##SD`).
    pub data_blocks: usize,
    /// Byte offset where the data region starts: everything before it is
    /// metadata, so a reader fetching this prefix sees the whole structure.
    pub metadata_size: u64,
    /// Total size of the output file in bytes.
    pub file_size: u64,
    /// Channels that could not be represented as a column, as
    /// `"group/channel: reason"`.
    pub skipped_channels: Vec<String>,
}

/// Destination of a column-oriented conversion.
///
/// The output layout is computed up front, so data is written at known
/// absolute offsets rather than strictly sequentially.
pub trait ColumnSink {
    /// Write `bytes` at absolute file position `offset`. Positions never
    /// written before must read back as zero (true for a fresh file or a
    /// zero-filled buffer).
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), MdfError>;

    /// Called once, before any write, with the final output size.
    fn set_size(&mut self, _size: u64) -> Result<(), MdfError> {
        Ok(())
    }
}

impl ColumnSink for Vec<u8> {
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), MdfError> {
        let start = usize::try_from(offset)
            .map_err(|_| err("output offset exceeds the address space"))?;
        let end = start + bytes.len();
        if self.len() < end {
            self.resize(end, 0);
        }
        self[start..end].copy_from_slice(bytes);
        Ok(())
    }

    fn set_size(&mut self, size: u64) -> Result<(), MdfError> {
        let size = usize::try_from(size).map_err(|_| err("output exceeds the address space"))?;
        self.clear();
        self.resize(size, 0);
        Ok(())
    }
}

/// [`ColumnSink`] over anything that is `Write + Seek` (e.g. a `File`).
///
/// Seeks only when the next write is not at the current position.
pub struct SeekSink<W: std::io::Write + std::io::Seek> {
    inner: W,
    pos: Option<u64>,
}

impl<W: std::io::Write + std::io::Seek> SeekSink<W> {
    pub fn new(inner: W) -> Self {
        Self { inner, pos: None }
    }

    /// Flush and return the wrapped writer.
    pub fn into_inner(mut self) -> Result<W, MdfError> {
        self.inner.flush()?;
        Ok(self.inner)
    }
}

impl<W: std::io::Write + std::io::Seek> ColumnSink for SeekSink<W> {
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), MdfError> {
        if self.pos != Some(offset) {
            self.inner.seek(std::io::SeekFrom::Start(offset))?;
        }
        self.inner.write_all(bytes)?;
        self.pos = Some(offset + bytes.len() as u64);
        Ok(())
    }
}

/// Convert the MDF file at `input` into a column-oriented MDF 4.20 file at
/// `output` (see the [module docs](self) for the layout).
///
/// The source is streamed once with positional reads; memory use is bounded by
/// [`ColumnOptions::memory_budget`]. Not available on `wasm32`; use
/// [`to_column_oriented_with`] there.
#[cfg(not(target_arch = "wasm32"))]
pub fn to_column_oriented(
    input: impl AsRef<std::path::Path>,
    output: impl AsRef<std::path::Path>,
    options: &ColumnOptions,
) -> Result<ColumnReport, MdfError> {
    let input = input.as_ref();
    let output = output.as_ref();
    if same_file(input, output) {
        return Err(err("input and output must be different files"));
    }
    let path = input
        .to_str()
        .ok_or_else(|| err("input path is not valid UTF-8"))?;
    // Plain positional reads rather than a memory map: the data is copied out
    // in chunks either way, and this keeps the resident set small.
    let mut reader = crate::index::FileRangeReader::new(path)?;
    let file = std::fs::File::create(output)?;
    let mut sink = SeekSink::new(std::io::BufWriter::with_capacity(1 << 20, file));
    let report = to_column_oriented_with(&mut reader, &mut sink, options)?;
    sink.into_inner()?;
    Ok(report)
}

#[cfg(not(target_arch = "wasm32"))]
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Convert an in-memory MDF file into an in-memory column-oriented file.
pub fn to_column_oriented_bytes(
    input: &[u8],
    options: &ColumnOptions,
) -> Result<(Vec<u8>, ColumnReport), MdfError> {
    let mut reader = SliceReader { data: input };
    let mut out = Vec::new();
    let report = to_column_oriented_with(&mut reader, &mut out, options)?;
    Ok((out, report))
}

/// Core conversion over any [`ByteRangeReader`] source and [`ColumnSink`]
/// destination.
///
/// Metadata is read first; then the sample data of each data group is
/// streamed sequentially (plus one extra pass over the data groups whose
/// time axes are candidates for sharing, and a counting pass over unsorted
/// data groups).
pub fn to_column_oriented_with<R, S>(
    reader: &mut R,
    sink: &mut S,
    options: &ColumnOptions,
) -> Result<ColumnReport, MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
    S: ColumnSink + ?Sized,
{
    if options.block_size == 0 {
        return Err(err("block_size must be greater than zero"));
    }
    let chunk = options.read_chunk_size.clamp(64 * 1024, 1 << 30) as usize;

    let mut src = walk_source(reader)?;
    for dg in 0..src.dgs.len() {
        count_records(reader, &mut src, dg, chunk)?;
    }
    let mut report = ColumnReport {
        source_groups: src.groups.iter().filter(|g| !g.is_vlsd_storage()).count(),
        ..Default::default()
    };
    let plan = plan_output(reader, &src, options, chunk, &mut report)?;
    let layout = build_layout(reader, &src, plan, options)?;

    sink.set_size(layout.file_size)?;
    for (addr, bytes) in &layout.fixed_writes {
        sink.write_at(*addr, bytes)?;
    }
    stream_data(reader, &src, &layout, options, chunk, sink)?;

    report.data_blocks = layout.groups.iter().map(|g| g.n_blocks as usize + usize::from(g.sd_len.is_some())).sum();
    report.metadata_size = layout.metadata_end;
    report.file_size = layout.file_size;
    Ok(report)
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn err(msg: impl Into<String>) -> MdfError {
    MdfError::BlockSerializationError(msg.into())
}

fn align8(v: u64) -> u64 {
    (v + 7) & !7
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Borrowing in-memory reader.
struct SliceReader<'a> {
    data: &'a [u8],
}

impl ByteRangeReader for SliceReader<'_> {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        let end = offset
            .checked_add(length)
            .filter(|&e| e <= self.data.len() as u64)
            .ok_or(MdfError::TooShortBuffer {
                actual: self.data.len(),
                expected: offset.saturating_add(length).min(usize::MAX as u64) as usize,
                file: file!(),
                line: line!(),
            })?;
        Ok(self.data[offset as usize..end as usize].to_vec())
    }
}

/// A raw block split into its parts.
struct RawBlock {
    id: [u8; 4],
    links: Vec<u64>,
    data: Vec<u8>,
}

fn read_header<R>(reader: &mut R, addr: u64) -> Result<BlockHeader, MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
{
    BlockHeader::from_bytes(&reader.read_range(addr, HDR)?)
}

fn read_block<R>(reader: &mut R, addr: u64) -> Result<RawBlock, MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
{
    let header = read_header(reader, addr)?;
    let links_len = header
        .links_nr
        .checked_mul(8)
        .and_then(|l| l.checked_add(HDR))
        .filter(|&l| l <= header.block_len)
        .ok_or_else(|| err(format!("block at {addr:#x} has an inconsistent link count")))?;
    if header.block_len > MAX_METADATA_BLOCK {
        return Err(err(format!(
            "block {} at {addr:#x} is implausibly large ({} bytes)",
            header.id, header.block_len
        )));
    }
    let bytes = reader.read_range(addr, header.block_len)?;
    let mut id = [0u8; 4];
    id.copy_from_slice(&bytes[0..4]);
    let links = (0..header.links_nr as usize).map(|i| u64_at(&bytes, 24 + i * 8)).collect();
    Ok(RawBlock { id, links, data: bytes[links_len as usize..].to_vec() })
}

fn assemble(id: &[u8; 4], links: &[u64], data: &[u8]) -> Vec<u8> {
    let len = HDR as usize + links.len() * 8 + data.len();
    let mut out = Vec::with_capacity(len);
    out.extend_from_slice(id);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(len as u64).to_le_bytes());
    out.extend_from_slice(&(links.len() as u64).to_le_bytes());
    for l in links {
        out.extend_from_slice(&l.to_le_bytes());
    }
    out.extend_from_slice(data);
    out
}

/// A `##TX`/`##MD` block holding `text` (NUL-terminated, 8-byte padded).
fn text_block(id: &[u8; 4], text: &str) -> Vec<u8> {
    let mut data = text.as_bytes().to_vec();
    data.push(0);
    data.resize(align8(HDR + data.len() as u64) as usize - HDR as usize, 0);
    assemble(id, &[], &data)
}

fn block_header_bytes(id: &[u8; 4], block_len: u64, links_nr: u64) -> [u8; 24] {
    let mut h = [0u8; 24];
    h[0..4].copy_from_slice(id);
    h[8..16].copy_from_slice(&block_len.to_le_bytes());
    h[16..24].copy_from_slice(&links_nr.to_le_bytes());
    h
}

// ---------------------------------------------------------------------------
// Source model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    /// `##DT`: records with record id and invalidation bytes.
    Dt,
    /// `##DV`: records with data bytes only.
    Dv,
}

#[derive(Clone)]
struct SrcBlock {
    data_offset: u64,
    len: u64,
    kind: BlockKind,
}

struct SrcChannel {
    name: Option<String>,
    links: Vec<u64>,
    /// Data section (after the links): 72 bytes for MDF 4.
    data: Vec<u8>,
    channel_type: u8,
    bit_offset: u8,
    byte_offset: u32,
    bit_count: u32,
    flags: u32,
    inval_pos: u32,
}

impl SrcChannel {
    fn is_master(&self) -> bool {
        self.channel_type == 2 || self.channel_type == 3
    }

    fn is_virtual(&self) -> bool {
        self.channel_type == 3 || self.channel_type == 6
    }

    /// Bytes this channel occupies in a record.
    fn byte_len(&self) -> u32 {
        if self.is_virtual() {
            0
        } else {
            (self.bit_offset as u32 + self.bit_count).div_ceil(8)
        }
    }
}

struct SrcGroup {
    dg: usize,
    cg_addr: u64,
    links: Vec<u64>,
    record_id: u64,
    flags: u16,
    path_separator: u16,
    samples: u32,
    inval_bytes: u32,
    name: Option<String>,
    channels: Vec<SrcChannel>,
    /// Records actually present in the data (filled by `count_records`).
    records: u64,
}

impl SrcGroup {
    fn is_vlsd_storage(&self) -> bool {
        self.flags & CG_FLAG_VLSD != 0
    }

    fn remote_master_link(&self) -> u64 {
        if self.flags & CG_FLAG_REMOTE_MASTER != 0 {
            self.links.get(6).copied().unwrap_or(0)
        } else {
            0
        }
    }

    fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| format!("group@{:#x}", self.cg_addr))
    }
}

struct SrcDg {
    rec_id_len: u8,
    comment: u64,
    blocks: Vec<SrcBlock>,
    groups: Vec<usize>,
}

struct Source {
    hd: RawBlock,
    dgs: Vec<SrcDg>,
    groups: Vec<SrcGroup>,
}

fn walk_source<R>(reader: &mut R) -> Result<Source, MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
{
    IdentificationBlock::from_bytes(&reader.read_range(0, 64)?)?;
    let hd = read_block(reader, 64)?;
    if &hd.id != b"##HD" || hd.links.len() < 6 {
        return Err(MdfError::BlockIDError {
            actual: String::from_utf8_lossy(&hd.id).into_owned(),
            expected: "##HD".into(),
        });
    }

    let mut dgs = Vec::new();
    let mut groups = Vec::new();
    let mut visited = HashSet::new();
    let mut dg_addr = hd.links[0];
    while dg_addr != 0 {
        if !visited.insert(dg_addr) {
            return Err(MdfError::BlockLinkError(format!("cycle in ##DG chain at {dg_addr:#x}")));
        }
        let dg = read_block(reader, dg_addr)?;
        if &dg.id != b"##DG" || dg.links.len() < 4 || dg.data.is_empty() {
            return Err(err(format!("expected a ##DG block at {dg_addr:#x}")));
        }
        let dg_index = dgs.len();
        let mut src_dg = SrcDg {
            rec_id_len: dg.data[0],
            comment: dg.links[3],
            blocks: collect_data_blocks(reader, dg.links[2])?,
            groups: Vec::new(),
        };
        if !matches!(src_dg.rec_id_len, 0 | 1 | 2 | 4 | 8) {
            return Err(err(format!("invalid record id size {} at {dg_addr:#x}", src_dg.rec_id_len)));
        }

        let mut cg_addr = dg.links[1];
        while cg_addr != 0 {
            if !visited.insert(cg_addr) {
                return Err(MdfError::BlockLinkError(format!("cycle in ##CG chain at {cg_addr:#x}")));
            }
            let cg = read_block(reader, cg_addr)?;
            if &cg.id != b"##CG" || cg.links.len() < 6 || cg.data.len() < 32 {
                return Err(err(format!("expected a ##CG block at {cg_addr:#x}")));
            }
            let name = read_string_block_via_reader(reader, cg.links[2])?;
            let next_cg = cg.links[0];
            let mut channels = Vec::new();
            let mut cn_addr = cg.links[1];
            while cn_addr != 0 {
                if !visited.insert(cn_addr) {
                    return Err(MdfError::BlockLinkError(format!("cycle in ##CN chain at {cn_addr:#x}")));
                }
                let cn = read_block(reader, cn_addr)?;
                if &cn.id != b"##CN" || cn.links.len() < 8 || cn.data.len() < 72 {
                    return Err(err(format!("expected a ##CN block at {cn_addr:#x}")));
                }
                let d = &cn.data;
                channels.push(SrcChannel {
                    name: read_string_block_via_reader(reader, cn.links[2])?,
                    channel_type: d[0],
                    bit_offset: d[3],
                    byte_offset: u32_at(d, 4),
                    bit_count: u32_at(d, 8),
                    flags: u32_at(d, 12),
                    inval_pos: u32_at(d, 16),
                    links: cn.links.clone(),
                    data: cn.data.clone(),
                });
                cn_addr = cn.links[0];
            }
            src_dg.groups.push(groups.len());
            groups.push(SrcGroup {
                dg: dg_index,
                cg_addr,
                record_id: u64_at(&cg.data, 0),
                flags: u16_at(&cg.data, 16),
                path_separator: u16_at(&cg.data, 18),
                samples: u32_at(&cg.data, 24),
                inval_bytes: u32_at(&cg.data, 28),
                links: cg.links,
                name,
                channels,
                records: 0,
            });
            cg_addr = next_cg;
        }
        dgs.push(src_dg);
        dg_addr = dg.links[0];
    }
    Ok(Source { hd, dgs, groups })
}

/// Follow a data group's `##DT`/`##DV`/`##DL`/`##HL` chain.
fn collect_data_blocks<R>(reader: &mut R, addr: u64) -> Result<Vec<SrcBlock>, MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
{
    let mut out = Vec::new();
    collect_chain(reader, addr, &mut out, |id| match id {
        b"##DT" => Some(BlockKind::Dt),
        b"##DV" => Some(BlockKind::Dv),
        _ => None,
    })?;
    Ok(out)
}

/// Follow a VLSD channel's `##SD`/`##DL`/`##HL` chain.
fn collect_sd_blocks<R>(reader: &mut R, addr: u64) -> Result<Vec<SrcBlock>, MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
{
    let mut out = Vec::new();
    collect_chain(reader, addr, &mut out, |id| (id == b"##SD").then_some(BlockKind::Dt))?;
    Ok(out)
}

fn collect_chain<R>(
    reader: &mut R,
    mut addr: u64,
    out: &mut Vec<SrcBlock>,
    leaf: impl Fn(&[u8]) -> Option<BlockKind>,
) -> Result<(), MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
{
    let mut visited = HashSet::new();
    while addr != 0 {
        if !visited.insert(addr) {
            return Err(MdfError::BlockLinkError(format!("cycle in data chain at {addr:#x}")));
        }
        let header = read_header(reader, addr)?;
        let id = header.id.as_bytes();
        if let Some(kind) = leaf(id) {
            out.push(SrcBlock { data_offset: addr + HDR, len: header.block_len.saturating_sub(HDR), kind });
            return Ok(());
        }
        match id {
            b"##DL" => {
                let dl = read_block(reader, addr)?;
                for &frag in dl.links.iter().skip(1) {
                    if frag == 0 {
                        continue;
                    }
                    let fh = read_header(reader, frag)?;
                    match leaf(fh.id.as_bytes()) {
                        Some(kind) => out.push(SrcBlock {
                            data_offset: frag + HDR,
                            len: fh.block_len.saturating_sub(HDR),
                            kind,
                        }),
                        None => return Err(unsupported_block(&fh.id, frag)),
                    }
                }
                addr = dl.links.first().copied().unwrap_or(0);
            }
            b"##HL" => {
                let hl = read_block(reader, addr)?;
                addr = hl.links.first().copied().unwrap_or(0);
            }
            _ => return Err(unsupported_block(&header.id, addr)),
        }
    }
    Ok(())
}

fn unsupported_block(id: &str, addr: u64) -> MdfError {
    if id == "##DZ" {
        err(format!(
            "compressed ##DZ data block at {addr:#x}: compressed MDF files are not supported"
        ))
    } else {
        MdfError::BlockIDError { actual: id.to_string(), expected: "a data block".into() }
    }
}

// ---------------------------------------------------------------------------
// Record streaming
// ---------------------------------------------------------------------------

/// Framing information for the records of one data group.
struct Framing {
    id_len: usize,
    /// `(record id, group position in dg.groups, record bytes after the id, is VLSD storage)`
    by_id: HashMap<u64, (usize, usize, bool)>,
    sorted: bool,
}

impl Framing {
    fn new(src: &Source, dg: usize) -> Result<Self, MdfError> {
        let d = &src.dgs[dg];
        let sorted = d.groups.len() == 1;
        let mut by_id = HashMap::new();
        for (pos, &g) in d.groups.iter().enumerate() {
            let grp = &src.groups[g];
            let size = grp.samples as usize + grp.inval_bytes as usize;
            if !sorted && by_id.insert(grp.record_id, (pos, size, grp.is_vlsd_storage())).is_some() {
                return Err(err(format!("duplicate record id {} in an unsorted data group", grp.record_id)));
            }
            if sorted {
                by_id.insert(0, (pos, size, grp.is_vlsd_storage()));
            }
        }
        if !sorted && d.rec_id_len == 0 {
            return Err(err("data group with several channel groups but no record ids"));
        }
        Ok(Self { id_len: d.rec_id_len as usize, by_id, sorted })
    }

    /// Length of the record at the start of `buf` and its group position, or
    /// `None` if `buf` does not hold a complete record yet.
    fn next(&self, buf: &[u8], kind: BlockKind, dv_size: usize) -> Result<Option<(usize, usize, usize, bool)>, MdfError> {
        if kind == BlockKind::Dv {
            return Ok((buf.len() >= dv_size).then_some((dv_size, 0, 0, false)));
        }
        if self.sorted {
            let (pos, size, _) = self.by_id[&0];
            let len = self.id_len + size;
            return Ok((buf.len() >= len).then_some((len, pos, self.id_len, false)));
        }
        if buf.len() < self.id_len {
            return Ok(None);
        }
        let id = match self.id_len {
            1 => buf[0] as u64,
            2 => u16_at(buf, 0) as u64,
            4 => u32_at(buf, 0) as u64,
            _ => u64_at(buf, 0),
        };
        let &(pos, size, vlsd) = self
            .by_id
            .get(&id)
            .ok_or_else(|| err(format!("unknown record id {id} in unsorted data group")))?;
        if vlsd {
            if buf.len() < self.id_len + 4 {
                return Ok(None);
            }
            let len = self.id_len + 4 + u32_at(buf, self.id_len) as usize;
            return Ok((buf.len() >= len).then_some((len, pos, self.id_len, true)));
        }
        let len = self.id_len + size;
        Ok((buf.len() >= len).then_some((len, pos, self.id_len, false)))
    }
}

/// Stream every record of data group `dg` once, calling `f(group_position,
/// record_without_id)` for each fixed-length record (VLSD storage records are
/// skipped). Records may straddle data-block boundaries. Returns the record
/// count per group position.
fn stream_records<R, F>(
    reader: &mut R,
    src: &Source,
    dg: usize,
    chunk: usize,
    mut f: F,
) -> Result<Vec<u64>, MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
    F: FnMut(usize, &[u8]) -> Result<(), MdfError>,
{
    let d = &src.dgs[dg];
    let mut counts = vec![0u64; d.groups.len()];
    if d.groups.is_empty() {
        return Ok(counts);
    }
    let framing = Framing::new(src, dg)?;
    let first = &src.groups[d.groups[0]];
    let dv_size = first.samples as usize;
    if d.blocks.iter().any(|b| b.kind == BlockKind::Dv) {
        if !framing.sorted || first.inval_bytes != 0 {
            return Err(err(
                "##DV data blocks are only supported for sorted groups without invalidation bytes",
            ));
        }
        if dv_size == 0 {
            return Ok(counts);
        }
    }
    if framing.sorted && framing.id_len + first.samples as usize + first.inval_bytes as usize == 0 {
        return Ok(counts);
    }

    // Sorted data: read whole records per chunk so records only need
    // re-assembly where they straddle a data-block boundary.
    let chunk = if framing.sorted {
        let dt_len = framing.id_len + first.samples as usize + first.inval_bytes as usize;
        let rec = dt_len.max(dv_size).max(1);
        (chunk / rec).max(1) * rec
    } else {
        chunk
    };
    let mut pending: Vec<u8> = Vec::new();
    let mut kind = BlockKind::Dt;
    for blk in &d.blocks {
        if blk.kind != kind && !pending.is_empty() {
            return Err(err("a record straddles a ##DT/##DV block boundary"));
        }
        kind = blk.kind;
        let mut off = 0u64;
        while off < blk.len {
            let n = (blk.len - off).min(chunk as u64);
            let bytes = reader.read_range(blk.data_offset + off, n)?;
            off += n;
            let buf: &[u8] = if pending.is_empty() {
                &bytes
            } else {
                pending.extend_from_slice(&bytes);
                &pending
            };
            let mut pos = 0usize;
            while let Some((len, gpos, id_len, vlsd)) = framing.next(&buf[pos..], kind, dv_size)? {
                if !vlsd {
                    f(gpos, &buf[pos + id_len..pos + len])?;
                }
                counts[gpos] += 1;
                pos += len;
            }
            pending = buf[pos..].to_vec();
        }
    }
    Ok(counts)
}

/// Determine the number of records of every group in data group `dg`.
///
/// Sorted groups are counted from the data-block sizes (no data is read);
/// unsorted groups need a pass over the records.
fn count_records<R>(reader: &mut R, src: &mut Source, dg: usize, chunk: usize) -> Result<(), MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
{
    let d = &src.dgs[dg];
    if d.groups.len() == 1 {
        let g = &src.groups[d.groups[0]];
        let dt_size = d.rec_id_len as u64 + g.samples as u64 + g.inval_bytes as u64;
        let (mut dt, mut dv) = (0u64, 0u64);
        for b in &d.blocks {
            match b.kind {
                BlockKind::Dt => dt += b.len,
                BlockKind::Dv => dv += b.len,
            }
        }
        let n = dt.checked_div(dt_size).unwrap_or(0) + dv.checked_div(g.samples as u64).unwrap_or(0);
        let gi = d.groups[0];
        src.groups[gi].records = n;
        return Ok(());
    }
    let counts = stream_records(reader, src, dg, chunk, |_, _| Ok(()))?;
    let groups = src.dgs[dg].groups.clone();
    for (pos, g) in groups.into_iter().enumerate() {
        src.groups[g].records = counts[pos];
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

/// One output data group (= one channel group = one channel).
struct OutGroup {
    src_group: usize,
    channel: usize,
    is_time: bool,
    /// Index (into the output groups) of the time group, for signal groups.
    master: Option<usize>,
    /// Bytes copied from each source record.
    data_bytes: u32,
    /// Whether an invalidation byte follows the data bytes.
    inval: bool,
    records: u64,
    /// VLSD `##SD` source fragments (empty = fixed-length channel).
    sd_blocks: Vec<SrcBlock>,
    vlsd: bool,
}

impl OutGroup {
    fn record_size(&self) -> u64 {
        self.data_bytes as u64 + u64::from(self.inval)
    }
}

/// Decide which output groups to write and which time group each signal uses.
fn plan_output<R>(
    reader: &mut R,
    src: &Source,
    options: &ColumnOptions,
    chunk: usize,
    report: &mut ColumnReport,
) -> Result<Vec<OutGroup>, MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
{
    // Local master channel of each source group.
    let master_of: Vec<Option<usize>> = src
        .groups
        .iter()
        .map(|g| {
            if g.is_vlsd_storage() {
                return None;
            }
            g.channels.iter().position(|c| c.is_master() && c.byte_offset + c.byte_len() <= g.samples)
        })
        .collect();

    // Group representing each source group's time axis (identical axes merge).
    let mut rep: Vec<usize> = (0..src.groups.len()).collect();
    if options.share_identical_time {
        merge_identical_time(reader, src, &master_of, chunk, &mut rep)?;
    }
    report.shared_time_groups =
        (0..src.groups.len()).filter(|&g| master_of[g].is_some() && rep[g] != g).count();

    let mut out: Vec<OutGroup> = Vec::new();
    let mut time_out: HashMap<usize, usize> = HashMap::new();
    let addr_to_group: HashMap<u64, usize> =
        src.groups.iter().enumerate().map(|(i, g)| (g.cg_addr, i)).collect();

    for (gi, g) in src.groups.iter().enumerate() {
        if g.is_vlsd_storage() {
            continue;
        }
        // Which source group provides this group's time axis?
        let time_src = match master_of[gi] {
            Some(_) => Some(rep[gi]),
            None => addr_to_group
                .get(&g.remote_master_link())
                .filter(|&&m| master_of[m].is_some())
                .map(|&m| rep[m]),
        };
        // The time group is written the first time any group needs it.
        let master = time_src.map(|t| {
            *time_out.entry(t).or_insert_with(|| {
                let tg = &src.groups[t];
                let ch = &tg.channels[master_of[t].unwrap()];
                out.push(OutGroup {
                    src_group: t,
                    channel: master_of[t].unwrap(),
                    is_time: true,
                    master: None,
                    data_bytes: ch.byte_len(),
                    inval: has_inval(tg, ch),
                    records: tg.records,
                    sd_blocks: Vec::new(),
                    vlsd: false,
                });
                out.len() - 1
            })
        });

        for (ci, ch) in g.channels.iter().enumerate() {
            if Some(ci) == master_of[gi] {
                continue;
            }
            let label = || format!("{}/{}", g.label(), ch.name.as_deref().unwrap_or("<unnamed>"));
            if ch.is_master() {
                report.skipped_channels.push(format!("{}: additional master channel", label()));
                continue;
            }
            if ch.byte_offset as u64 + ch.byte_len() as u64 > g.samples as u64 || ch.bit_offset > 7 {
                report.skipped_channels.push(format!("{}: lies outside the record", label()));
                continue;
            }
            if ch.links[1] != 0 && read_header(reader, ch.links[1])?.id == "##CA" {
                report.skipped_channels.push(format!("{}: array (##CA) channels are not supported", label()));
                continue;
            }
            let mut sd_blocks = Vec::new();
            let mut vlsd = false;
            if ch.channel_type == 1 && ch.links[5] != 0 {
                let target = read_header(reader, ch.links[5])?;
                if target.id == "##CG" {
                    report.skipped_channels.push(format!(
                        "{}: VLSD data stored in a VLSD channel group is not supported",
                        label()
                    ));
                    continue;
                }
                sd_blocks = collect_sd_blocks(reader, ch.links[5])?;
                vlsd = true;
            }
            out.push(OutGroup {
                src_group: gi,
                channel: ci,
                is_time: false,
                master,
                data_bytes: ch.byte_len(),
                inval: has_inval(g, ch),
                records: g.records,
                sd_blocks,
                vlsd,
            });
        }
    }
    report.time_groups = out.iter().filter(|o| o.is_time).count();
    report.signal_groups = out.len() - report.time_groups;
    Ok(out)
}

fn has_inval(g: &SrcGroup, ch: &SrcChannel) -> bool {
    ch.flags & CN_FLAG_ALL_INVALID == 0
        && ch.flags & CN_FLAG_INVAL_BIT_VALID != 0
        && (ch.inval_pos / 8) < g.inval_bytes
}

/// Point `rep[g]` at the first group whose time axis is byte-identical to
/// `g`'s. Only groups with the same record count and master layout are
/// compared, so files without shareable axes pay nothing.
fn merge_identical_time<R>(
    reader: &mut R,
    src: &Source,
    master_of: &[Option<usize>],
    chunk: usize,
    rep: &mut [usize],
) -> Result<(), MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
{
    type Key = (u64, u8, u8, u8, u32, u32, u32, u64, Vec<u8>);
    let mut by_key: HashMap<Key, Vec<usize>> = HashMap::new();
    for (g, m) in master_of.iter().enumerate() {
        let Some(m) = *m else { continue };
        let grp = &src.groups[g];
        let ch = &grp.channels[m];
        let key = (
            grp.records,
            ch.channel_type,
            ch.data[2],
            ch.bit_offset,
            ch.bit_count,
            ch.byte_len(),
            ch.flags,
            ch.links[4], // same conversion block
            ch.data[..2].to_vec(),
        );
        by_key.entry(key).or_default().push(g);
    }
    let candidates: Vec<Vec<usize>> = by_key.into_values().filter(|v| v.len() > 1).collect();
    if candidates.is_empty() {
        return Ok(());
    }

    // Hash the raw master bytes of every candidate, one pass per data group.
    let mut hashes: HashMap<usize, (u64, u64)> = HashMap::new();
    let wanted: HashSet<usize> = candidates.iter().flatten().copied().collect();
    for (dgi, d) in src.dgs.iter().enumerate() {
        let targets: Vec<(usize, usize)> = d
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| wanted.contains(g))
            .map(|(pos, &g)| (pos, g))
            .collect();
        if targets.is_empty() {
            continue;
        }
        let mut hashers: HashMap<usize, ChunkedHasher> =
            targets.iter().map(|&(pos, _)| (pos, ChunkedHasher::default())).collect();
        let spans: HashMap<usize, (usize, usize)> = targets
            .iter()
            .map(|&(pos, g)| {
                let ch = &src.groups[g].channels[master_of[g].unwrap()];
                (pos, (ch.byte_offset as usize, ch.byte_len() as usize))
            })
            .collect();
        stream_records(reader, src, dgi, chunk, |pos, rec| {
            if let (Some(h), Some(&(o, l))) = (hashers.get_mut(&pos), spans.get(&pos)) {
                h.write(&rec[o..o + l]);
            }
            Ok(())
        })?;
        for (pos, g) in targets {
            hashes.insert(g, hashers.remove(&pos).unwrap().finish());
        }
    }

    for group in candidates {
        let mut firsts: Vec<usize> = Vec::new();
        for g in group {
            match firsts.iter().find(|&&f| hashes[&f] == hashes[&g]) {
                Some(&f) => rep[g] = f,
                None => firsts.push(g),
            }
        }
    }
    Ok(())
}

/// 128-bit content hash that does not depend on how the input is split.
#[derive(Default)]
struct ChunkedHasher {
    buf: Vec<u8>,
    a: std::collections::hash_map::DefaultHasher,
    b: std::collections::hash_map::DefaultHasher,
    len: u64,
}

impl ChunkedHasher {
    const BLOCK: usize = 64 * 1024;

    fn write(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        self.len += bytes.len() as u64;
        if self.buf.len() >= Self::BLOCK {
            self.drain();
        }
    }

    fn drain(&mut self) {
        self.a.write(&self.buf);
        self.b.write_u64(0x9e37_79b9_7f4a_7c15);
        self.b.write(&self.buf);
        self.buf.clear();
    }

    fn finish(mut self) -> (u64, u64) {
        self.drain();
        self.a.write_u64(self.len);
        (self.a.finish(), self.b.finish())
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// Growable metadata region starting at absolute offset `base`.
struct MetaBuf {
    base: u64,
    buf: Vec<u8>,
}

impl MetaBuf {
    fn new(base: u64) -> Self {
        Self { base, buf: Vec::new() }
    }

    fn end(&self) -> u64 {
        self.base + self.buf.len() as u64
    }

    fn reserve(&mut self, len: u64) -> u64 {
        let addr = align8(self.end());
        self.buf.resize((addr + len - self.base) as usize, 0);
        addr
    }

    fn alloc(&mut self, bytes: &[u8]) -> u64 {
        let addr = self.reserve(bytes.len() as u64);
        self.put(addr, bytes);
        addr
    }

    fn put(&mut self, addr: u64, bytes: &[u8]) {
        let start = (addr - self.base) as usize;
        self.buf[start..start + bytes.len()].copy_from_slice(bytes);
    }
}

/// Deep-copies metadata blocks (`##TX`, `##MD`, `##CC`, `##SI`, `##FH`,
/// `##AT`), remapping their links; each source block is copied once.
#[derive(Default)]
struct Copier {
    memo: HashMap<u64, u64>,
    visiting: HashSet<u64>,
}

impl Copier {
    fn copy<R>(&mut self, reader: &mut R, buf: &mut MetaBuf, addr: u64) -> Result<u64, MdfError>
    where
        R: ByteRangeReader<Error = MdfError>,
    {
        self.copy_inner(reader, buf, addr, 0)
    }

    fn copy_inner<R>(&mut self, reader: &mut R, buf: &mut MetaBuf, addr: u64, depth: u32) -> Result<u64, MdfError>
    where
        R: ByteRangeReader<Error = MdfError>,
    {
        if addr == 0 {
            return Ok(0);
        }
        if let Some(&out) = self.memo.get(&addr) {
            return Ok(out);
        }
        if depth > 64 || self.visiting.contains(&addr) {
            return Err(MdfError::ConversionChainCycle { address: addr });
        }
        let mut blk = read_block(reader, addr)?;
        match &blk.id {
            b"##TX" | b"##MD" | b"##CC" | b"##SI" => {}
            // Chains (`next` links) are walked iteratively by the caller.
            b"##FH" | b"##AT" => {
                if let Some(next) = blk.links.first_mut() {
                    *next = 0;
                }
            }
            _ => return Ok(0),
        }
        self.visiting.insert(addr);
        for l in blk.links.iter_mut() {
            *l = self.copy_inner(reader, buf, *l, depth + 1)?;
        }
        self.visiting.remove(&addr);
        let out = buf.alloc(&assemble(&blk.id, &blk.links, &blk.data));
        self.memo.insert(addr, out);
        Ok(out)
    }

    /// Copy a `next`-linked chain (`##FH`, `##AT`); returns the first and last
    /// copied addresses.
    fn copy_chain<R>(&mut self, reader: &mut R, buf: &mut MetaBuf, mut addr: u64) -> Result<(u64, u64), MdfError>
    where
        R: ByteRangeReader<Error = MdfError>,
    {
        let (mut first, mut last) = (0u64, 0u64);
        let mut seen = HashSet::new();
        while addr != 0 && seen.insert(addr) {
            let next = read_block(reader, addr)?.links.first().copied().unwrap_or(0);
            let out = self.copy(reader, buf, addr)?;
            if out == 0 {
                break;
            }
            if last != 0 {
                buf.put(last + HDR, &out.to_le_bytes());
            } else {
                first = out;
            }
            last = out;
            addr = next;
        }
        Ok((first, last))
    }
}

/// Final placement of one output group.
struct PlacedGroup {
    out: OutGroup,
    /// Payload bytes per data block (a whole number of records, 8-byte aligned).
    block_payload: u64,
    n_blocks: u64,
    total_bytes: u64,
    data_addr: u64,
    block_id: [u8; 4],
    sd_addr: u64,
    sd_len: Option<u64>,
}

impl PlacedGroup {
    fn block_addr(&self, i: u64) -> u64 {
        self.data_addr + i * (HDR + self.block_payload)
    }

    fn block_len(&self, i: u64) -> u64 {
        if i + 1 < self.n_blocks {
            self.block_payload
        } else {
            self.total_bytes - i * self.block_payload
        }
    }
}

struct Layout {
    groups: Vec<PlacedGroup>,
    /// Pre-built metadata to write verbatim: `(address, bytes)`.
    fixed_writes: Vec<(u64, Vec<u8>)>,
    metadata_end: u64,
    file_size: u64,
}

fn build_layout<R>(
    reader: &mut R,
    src: &Source,
    plan: Vec<OutGroup>,
    options: &ColumnOptions,
) -> Result<Layout, MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
{
    const ID_LEN: u64 = 64;
    const HD_LEN: u64 = 104;
    let mut meta = MetaBuf::new(ID_LEN + HD_LEN);
    let mut copier = Copier::default();

    // File-level metadata: header comment, file history (+ our own entry).
    let hd_comment = copier.copy(reader, &mut meta, src.hd.links[5])?;
    let (mut fh_first, fh_last) = copier.copy_chain(reader, &mut meta, src.hd.links[1])?;
    let fh_md = meta.alloc(&text_block(
        b"##MD",
        &format!(
            "<FHcomment xmlns=\"http://www.asam.net/mdf/v4\"><TX>Converted to column-oriented MDF 4.20 \
             (one signal per channel group, shared time channels)</TX><tool_id>mf4-rs</tool_id>\
             <tool_vendor>mf4-rs</tool_vendor><tool_version>{}</tool_version></FHcomment>",
            env!("CARGO_PKG_VERSION")
        ),
    ));
    let mut fh_data = Vec::with_capacity(16);
    fh_data.extend_from_slice(&now_ns().to_le_bytes());
    fh_data.extend_from_slice(&[0u8; 8]); // tz/dst offsets, time flags (UTC), reserved
    let our_fh = meta.alloc(&assemble(b"##FH", &[0, fh_md], &fh_data));
    if fh_last != 0 {
        meta.put(fh_last + HDR, &our_fh.to_le_bytes());
    } else {
        fh_first = our_fh;
    }

    // Metadata referenced by each output group (shared blocks copied once).
    struct GroupMeta {
        dg_comment: u64,
        cg_links: [u64; 3], // acq name, acq source, comment
        cn_links: [u64; 5], // name, source, conversion, unit, comment
    }
    let mut metas = Vec::with_capacity(plan.len());
    for og in &plan {
        let g = &src.groups[og.src_group];
        let ch = &g.channels[og.channel];
        let mut cg_links = [0u64; 3];
        for (slot, &l) in cg_links.iter_mut().zip(&[g.links[2], g.links[3], g.links[5]]) {
            *slot = copier.copy(reader, &mut meta, l)?;
        }
        let mut cn_links = [0u64; 5];
        for (slot, &l) in cn_links.iter_mut().zip(&[ch.links[2], ch.links[3], ch.links[4], ch.links[6], ch.links[7]]) {
            *slot = copier.copy(reader, &mut meta, l)?;
        }
        metas.push(GroupMeta {
            dg_comment: copier.copy(reader, &mut meta, src.dgs[g.dg].comment)?,
            cg_links,
            cn_links,
        });
    }

    // Data placement: compute block geometry per group.
    let mut placed: Vec<PlacedGroup> = plan
        .into_iter()
        .map(|out| {
            let rs = out.record_size();
            let total = rs * out.records;
            let (payload, n) = if total == 0 {
                (0, 0)
            } else {
                // Whole records per block, payload a multiple of 8 so the
                // blocks of one signal sit back to back without padding.
                let step = 8 / gcd(rs, 8);
                let per_block = ((options.block_size / rs) / step * step).max(step);
                let payload = per_block * rs;
                if total <= payload { (total, 1) } else { (payload, total.div_ceil(payload)) }
            };
            PlacedGroup {
                block_id: if out.inval { *b"##DT" } else { *b"##DV" },
                out,
                block_payload: payload,
                n_blocks: n,
                total_bytes: total,
                data_addr: 0,
                sd_addr: 0,
                sd_len: None,
            }
        })
        .collect();

    // Structural blocks, in output order: DG, CG, CN (+ DL).
    struct Addrs {
        dg: u64,
        cg: u64,
        cn: u64,
        dl: u64,
    }
    let mut addrs = Vec::with_capacity(placed.len());
    for p in &placed {
        let dg = meta.reserve(64);
        let cg = meta.reserve(if p.out.master.is_some() { 112 } else { 104 });
        let cn = meta.reserve(160);
        // Data is always referenced through an equal-length ##DL (even a
        // single block), so readers learn every block size from the metadata
        // prefix and never have to read a header in the data region.
        let dl = if p.n_blocks > 0 { meta.reserve(HDR + (p.n_blocks + 1) * 8 + 16) } else { 0 };
        addrs.push(Addrs { dg, cg, cn, dl });
    }
    let metadata_end = align8(meta.end());

    let mut cursor = metadata_end;
    for p in placed.iter_mut() {
        if p.n_blocks > 0 {
            p.data_addr = cursor;
            cursor = align8(p.block_addr(p.n_blocks - 1) + HDR + p.block_len(p.n_blocks - 1));
        }
        if p.out.vlsd {
            let len: u64 = p.out.sd_blocks.iter().map(|b| b.len).sum();
            p.sd_addr = cursor;
            p.sd_len = Some(len);
            cursor = align8(cursor + HDR + len);
        }
    }

    // Attachments go after the data so they never bloat the metadata prefix.
    let mut tail = MetaBuf::new(cursor);
    let (at_first, _) = copier.copy_chain(reader, &mut tail, src.hd.links[3])?;
    let file_size = align8(tail.end());

    // Fill in the structural blocks.
    for (i, (p, a)) in placed.iter().zip(&addrs).enumerate() {
        let m = &metas[i];
        let g = &src.groups[p.out.src_group];
        let ch = &g.channels[p.out.channel];
        let next_dg = addrs.get(i + 1).map_or(0, |n| n.dg);
        let data_link = a.dl;

        let mut dg_data = [0u8; 8];
        dg_data[0] = 0; // record id size
        meta.put(a.dg, &assemble(b"##DG", &[next_dg, a.cg, data_link, m.dg_comment], &dg_data));

        let mut cg_links = vec![0, a.cn, m.cg_links[0], m.cg_links[1], 0, m.cg_links[2]];
        let mut flags = g.flags & !(CG_FLAG_VLSD | CG_FLAG_REMOTE_MASTER);
        if let Some(t) = p.out.master {
            cg_links.push(addrs[t].cg);
            flags |= CG_FLAG_REMOTE_MASTER;
        }
        let mut cg_data = Vec::with_capacity(32);
        cg_data.extend_from_slice(&0u64.to_le_bytes()); // record id
        cg_data.extend_from_slice(&p.out.records.to_le_bytes());
        cg_data.extend_from_slice(&flags.to_le_bytes());
        cg_data.extend_from_slice(&g.path_separator.to_le_bytes());
        cg_data.extend_from_slice(&0u32.to_le_bytes());
        cg_data.extend_from_slice(&p.out.data_bytes.to_le_bytes());
        cg_data.extend_from_slice(&u32::from(p.out.inval).to_le_bytes());
        meta.put(a.cg, &assemble(b"##CG", &cg_links, &cg_data));

        let mut cn_data = ch.data[..72].to_vec();
        let mut cn_type = ch.channel_type;
        if cn_type == 5 {
            cn_type = 0; // maximum-length channel: keep the full bytes, drop the size link
        }
        cn_data[0] = cn_type;
        cn_data[4..8].copy_from_slice(&0u32.to_le_bytes()); // byte offset
        let mut cn_flags = ch.flags & !CN_FLAG_DEFAULT_X;
        if !p.out.inval {
            cn_flags &= !CN_FLAG_INVAL_BIT_VALID;
        }
        cn_data[12..16].copy_from_slice(&cn_flags.to_le_bytes());
        cn_data[16..20].copy_from_slice(&0u32.to_le_bytes()); // invalidation bit position
        cn_data[22..24].copy_from_slice(&0u16.to_le_bytes()); // attachment count
        let sd_link = if p.sd_len.is_some() { p.sd_addr } else { 0 };
        let cn_links = [0, 0, m.cn_links[0], m.cn_links[1], m.cn_links[2], sd_link, m.cn_links[3], m.cn_links[4]];
        meta.put(a.cn, &assemble(b"##CN", &cn_links, &cn_data));

        if p.n_blocks > 0 {
            let links = (0..p.n_blocks).map(|b| p.block_addr(b)).collect();
            let dl = DataListBlock::new_equal(links, p.block_payload);
            meta.put(a.dl, &dl.to_bytes()?);
        }
    }

    // ##ID + ##HD.
    let id = IdentificationBlock {
        version_identifier: "4.20    ".into(),
        version_number: 420,
        ..Default::default()
    };
    let first_dg = addrs.first().map_or(0, |a| a.dg);
    let hd = assemble(b"##HD", &[first_dg, fh_first, 0, at_first, 0, hd_comment], &src.hd.data);

    let mut fixed_writes = vec![(0, id.to_bytes()?), (64, hd)];
    let MetaBuf { base, buf } = meta;
    fixed_writes.push((base, buf));
    if !tail.buf.is_empty() {
        fixed_writes.push((tail.base, tail.buf));
    }
    Ok(Layout { groups: placed, fixed_writes, metadata_end, file_size })
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Current time in ns since the Unix epoch (for the `##FH` entry).
fn now_ns() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
    #[cfg(all(target_arch = "wasm32", feature = "wasm"))]
    {
        (js_sys::Date::now() * 1e6) as u64
    }
    #[cfg(all(target_arch = "wasm32", not(feature = "wasm")))]
    {
        0
    }
}

// ---------------------------------------------------------------------------
// Data streaming
// ---------------------------------------------------------------------------

/// Buffered writer for one output group's logical data stream; inserts the
/// data-block headers at the pre-computed positions.
struct ColumnStream<'a> {
    group: &'a PlacedGroup,
    written: u64,
    buf: Vec<u8>,
    cap: usize,
}

impl ColumnStream<'_> {
    #[inline]
    fn push<S: ColumnSink + ?Sized>(&mut self, bytes: &[u8], sink: &mut S) -> Result<(), MdfError> {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() >= self.cap {
            self.flush(sink)?;
        }
        Ok(())
    }

    fn flush<S: ColumnSink + ?Sized>(&mut self, sink: &mut S) -> Result<(), MdfError> {
        let g = self.group;
        if self.written + self.buf.len() as u64 > g.total_bytes {
            return Err(err("more records than counted while streaming"));
        }
        let mut rest: &[u8] = &self.buf;
        while !rest.is_empty() {
            let block = self.written / g.block_payload;
            let pos = self.written % g.block_payload;
            let addr = g.block_addr(block);
            if pos == 0 {
                sink.write_at(addr, &block_header_bytes(&g.block_id, HDR + g.block_len(block), 0))?;
            }
            let n = ((g.block_len(block) - pos) as usize).min(rest.len());
            sink.write_at(addr + HDR + pos, &rest[..n])?;
            self.written += n as u64;
            rest = &rest[n..];
        }
        self.buf.clear();
        Ok(())
    }
}

/// How to extract one output column from a source record.
struct Extract {
    stream: usize,
    offset: usize,
    len: usize,
    /// `(byte index in the record after the id, bit mask)` of the
    /// invalidation bit.
    inval: Option<(usize, u8)>,
}

fn stream_data<R, S>(
    reader: &mut R,
    src: &Source,
    layout: &Layout,
    options: &ColumnOptions,
    chunk: usize,
    sink: &mut S,
) -> Result<(), MdfError>
where
    R: ByteRangeReader<Error = MdfError>,
    S: ColumnSink + ?Sized,
{
    for (dgi, d) in src.dgs.iter().enumerate() {
        // Output groups fed by this data group (time groups once, by their
        // representative source group).
        let mut streams: Vec<ColumnStream> = Vec::new();
        let mut extracts: Vec<Vec<Extract>> = (0..d.groups.len()).map(|_| Vec::new()).collect();
        for p in &layout.groups {
            let Some(pos) = d.groups.iter().position(|&g| g == p.out.src_group) else { continue };
            if p.total_bytes == 0 {
                continue;
            }
            let g = &src.groups[p.out.src_group];
            let ch = &g.channels[p.out.channel];
            let inval = p.out.inval.then(|| {
                let bit = ch.inval_pos as usize;
                (g.samples as usize + bit / 8, 1u8 << (bit % 8))
            });
            extracts[pos].push(Extract {
                stream: streams.len(),
                offset: ch.byte_offset as usize,
                len: p.out.data_bytes as usize,
                inval,
            });
            streams.push(ColumnStream { group: p, written: 0, buf: Vec::new(), cap: 0 });
        }

        if !streams.is_empty() {
            let cap = (options.memory_budget / streams.len() as u64)
                .clamp(MIN_STREAM_BUFFER as u64, (options.block_size.max(MIN_STREAM_BUFFER as u64)).min(1 << 30))
                as usize;
            for s in streams.iter_mut() {
                s.cap = cap;
                s.buf.reserve(cap.min(s.group.total_bytes as usize) + 64);
            }
            stream_records(reader, src, dgi, chunk, |pos, rec| {
                for e in &extracts[pos] {
                    let s = &mut streams[e.stream];
                    s.push(&rec[e.offset..e.offset + e.len], sink)?;
                    if let Some((byte, mask)) = e.inval {
                        s.push(&[u8::from(rec[byte] & mask != 0)], sink)?;
                    }
                }
                Ok(())
            })?;
            for s in streams.iter_mut() {
                s.flush(sink)?;
                if s.written != s.group.total_bytes {
                    return Err(err(format!(
                        "record count changed while streaming ({} of {} bytes)",
                        s.written, s.group.total_bytes
                    )));
                }
            }
        }

        // VLSD signal data: copy each channel's ##SD stream verbatim right
        // after its records, so the inline offsets stay valid.
        for p in &layout.groups {
            let (Some(sd_len), true) = (p.sd_len, d.groups.contains(&p.out.src_group)) else { continue };
            sink.write_at(p.sd_addr, &block_header_bytes(b"##SD", HDR + sd_len, 0))?;
            let mut at = p.sd_addr + HDR;
            for b in &p.out.sd_blocks {
                let mut off = 0;
                while off < b.len {
                    let n = (b.len - off).min(chunk as u64);
                    let bytes = reader.read_range(b.data_offset + off, n)?;
                    sink.write_at(at, &bytes)?;
                    at += n;
                    off += n;
                }
            }
        }
    }
    Ok(())
}
