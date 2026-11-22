//! MDF File Indexing System
//!
//! This module provides functionality to create lightweight indexes of MDF files
//! that can be serialized to JSON and used later to read specific channel data
//! without parsing the entire file structure.

use serde::{Deserialize, Serialize};
use crate::api::mdf::Mdf;
use crate::blocks::common::{DataType, BlockHeader, BlockParse};
use crate::blocks::conversion::{ConversionBlock, ConversionType};
use crate::error::MdfError;
#[cfg(feature = "http")]
use crate::fragments::{merge_ranges_bridging, FragmentStore, RecordingRangeReader};
use crate::parsing::decoder::{check_value_validity, decode_channel_value, decode_channel_value_with_validity, decode_f64_from_record, DecodedValue};
use crate::signal::Signal;
use crate::types::{
    logical_group_members, merge_group_infos, resolve_records, search_time_window, ChannelInfo, ConversionInfo,
    GroupInfo, ReadOptions, TimeRange, Window,
};
use std::borrow::Cow;
use std::collections::HashMap;

/// Largest single read issued when coalescing adjacent data blocks (64 MiB).
const MAX_COALESCED_READ: u64 = 64 * 1024 * 1024;

/// Represents the location and metadata of data blocks in the file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataBlockInfo {
    /// File offset where the data block starts
    pub file_offset: u64,
    /// Size of the data block in bytes
    pub size: u64,
    /// Whether this is a compressed block (DZ)
    pub is_compressed: bool,
}

/// Channel metadata needed for decoding values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedChannel {
    /// Channel name
    pub name: Option<String>,
    /// Physical unit
    pub unit: Option<String>,
    /// Channel comment (`##TX` text, or the raw `##MD` XML)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Data type of the channel
    pub data_type: DataType,
    /// Byte offset within each record
    pub byte_offset: u32,
    /// Bit offset within the byte
    pub bit_offset: u8,
    /// Number of bits for this channel
    pub bit_count: u32,
    /// Channel type (0=data, 1=VLSD, 2=master, etc.)
    pub channel_type: u8,
    /// Channel flags (includes invalidation bit flags)
    pub flags: u32,
    /// Position of invalidation bit within invalidation bytes
    pub pos_invalidation_bit: u32,
    /// Conversion block for unit conversion (if any).
    ///
    /// May be `None` even when the channel *has* a conversion: index builds
    /// that read a file over the network ([`MdfIndex::from_url`] /
    /// [`MdfIndex::from_range_reader`]) do **not** fetch and resolve the
    /// conversion block up front — they only record its file offset in
    /// [`conversion_addr`](Self::conversion_addr) and resolve it lazily on the
    /// first value read. Indexes built from a local file or an in-memory buffer
    /// still resolve eagerly, so this stays populated for them. Read paths
    /// prefer a resolved conversion here and otherwise fall back to
    /// `conversion_addr`.
    pub conversion: Option<ConversionBlock>,
    /// File offset of the channel's `##CC` conversion block (0 = none).
    ///
    /// Recorded so the conversion can be resolved lazily at read time when it
    /// was not resolved during index building (the remote / range-reader
    /// path). Defaults to 0 for older serialized indexes, which carry the fully
    /// resolved [`conversion`](Self::conversion) instead.
    #[serde(default)]
    pub conversion_addr: u64,
    /// For VLSD channels: address of signal data blocks
    pub vlsd_data_address: Option<u64>,
    /// For VLSD channels: locations of the ##SD fragments in chain order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vlsd_data_blocks: Vec<DataBlockInfo>,
}

impl IndexedChannel {
    /// `true` if this is the group's master channel (usually time): a
    /// regular (`cn_type` 2) or virtual (`cn_type` 3) master.
    pub fn is_master(&self) -> bool {
        self.channel_type == 2 || self.channel_type == 3
    }

    /// `true` for a virtual channel (`cn_type` 3 master or 6 data): it stores
    /// no bytes, its raw value is the record index.
    pub fn is_virtual(&self) -> bool {
        self.channel_type == 3 || self.channel_type == 6
    }

    /// This channel's metadata as a plain [`ChannelInfo`].
    pub fn info(&self) -> ChannelInfo {
        ChannelInfo {
            name: self.name.clone(),
            unit: self.unit.clone(),
            comment: self.comment.clone(),
            data_type: self.data_type.name().to_string(),
            bit_count: self.bit_count,
            is_master: self.is_master(),
            is_vlsd: self.is_vlsd(),
            has_conversion: self.conversion.is_some() || self.conversion_addr != 0,
        }
    }

    /// `true` if this is a variable-length (VLSD) channel.
    pub fn is_vlsd(&self) -> bool {
        self.channel_type == 1 && self.vlsd_data_address.is_some()
    }

    /// Create a lightweight `ChannelBlock` for the decoder functions (which
    /// only read the record layout): the name and conversion are not cloned.
    /// Build it once per read and reuse it across all records.
    fn to_decode_only_channel_block(&self) -> crate::blocks::channel_block::ChannelBlock {
        Self::build_channel_block(
            self.channel_type,
            self.data_type.clone(),
            self.bit_offset,
            self.byte_offset,
            self.bit_count,
            self.flags,
            self.pos_invalidation_bit,
            None,
            None,
        )
    }

    fn build_channel_block(
        channel_type: u8,
        data_type: DataType,
        bit_offset: u8,
        byte_offset: u32,
        bit_count: u32,
        flags: u32,
        pos_invalidation_bit: u32,
        name: Option<String>,
        conversion: Option<ConversionBlock>,
    ) -> crate::blocks::channel_block::ChannelBlock {
        crate::blocks::channel_block::ChannelBlock {
            header: crate::blocks::common::BlockHeader {
                id: "##CN".to_string(),
                reserved0: 0,
                block_len: 160,
                links_nr: 8,
            },
            next_ch_addr: 0,
            component_addr: 0,
            name_addr: 0,
            source_addr: 0,
            conversion_addr: 0,
            data: 0,
            unit_addr: 0,
            comment_addr: 0,
            channel_type,
            sync_type: 0,
            data_type,
            bit_offset,
            byte_offset,
            bit_count,
            flags,
            pos_invalidation_bit,
            precision: 0,
            reserved1: 0,
            attachment_nr: 0,
            min_raw_value: 0.0,
            max_raw_value: 0.0,
            lower_limit: 0.0,
            upper_limit: 0.0,
            lower_ext_limit: 0.0,
            upper_ext_limit: 0.0,
            name,
            conversion,
        }
    }
}

/// Channel group metadata and layout information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedChannelGroup {
    /// Group name
    pub name: Option<String>,
    /// Comment
    pub comment: Option<String>,
    /// Size of record ID in bytes
    pub record_id_len: u8,
    /// Total size of each record in bytes (excluding record ID and invalidation bytes)
    pub record_size: u32,
    /// Number of invalidation bytes per record
    pub invalidation_bytes: u32,
    /// Number of records in this group
    pub record_count: u64,
    /// Channels in this group
    pub channels: Vec<IndexedChannel>,
    /// Data block locations for this channel group
    pub data_blocks: Vec<DataBlockInfo>,
    /// Index (into [`MdfIndex::channel_groups`]) of the group holding this
    /// group's master channel when it is stored *remotely* — the MDF 4.2
    /// column-oriented layout, where each signal has its own channel group and
    /// points at a shared time group via `cg_cg_master`. `None` when the master
    /// is local (or there is none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_group: Option<usize>,
}

impl IndexedChannelGroup {
    /// Find a channel in this group by name (first match).
    pub fn channel(&self, name: &str) -> Option<&IndexedChannel> {
        self.channels
            .iter()
            .find(|c| c.name.as_deref() == Some(name))
    }

    /// Names of every named channel in this group, in record order.
    pub fn channel_names(&self) -> Vec<&str> {
        self.channels
            .iter()
            .filter_map(|c| c.name.as_deref())
            .collect()
    }

    /// The group's master channel (channel type 2), if any.
    pub fn master_channel(&self) -> Option<&IndexedChannel> {
        self.channels.iter().find(|c| c.is_master())
    }
}

/// Where an [`MdfIndex`] reads sample data from when asked to.
///
/// The source is *not* serialized with the index — after [`MdfIndex::load`]
/// re-attach one with [`MdfIndex::set_source`]. Building an
/// index never reads sample data; the actual byte-range reads happen lazily on
/// [`MdfIndex::read`].
#[derive(Debug, Clone)]
pub enum Source {
    /// A local file path, read via memory map.
    File(String),
    /// An HTTP/S3 URL, read via range requests.
    #[cfg(feature = "http")]
    Url(String),
}

impl Source {
    /// The file path or URL as a string.
    pub fn as_str(&self) -> &str {
        match self {
            Source::File(p) => p,
            #[cfg(feature = "http")]
            Source::Url(u) => u,
        }
    }
}

/// A string is a [`Source`]: `http://` / `https://` URLs become
/// [`Source::Url`] (with the `http` feature), anything else a file path.
/// The URL scheme of `value` (`"https"` for `https://…`), if it has one.
/// A Windows drive path (`C:\\…`, `C:/…`) has none.
fn url_scheme(value: &str) -> Option<&str> {
    let (scheme, _) = value.split_once("://")?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    (valid && scheme.len() > 1).then_some(scheme)
}

/// True for an `http://` / `https://` URL (scheme matched case-insensitively).
fn is_http_url(value: &str) -> bool {
    url_scheme(value).is_some_and(|s| s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https"))
}

impl From<&str> for Source {
    fn from(value: &str) -> Self {
        #[cfg(feature = "http")]
        if is_http_url(value) {
            return Source::Url(value.to_string());
        }
        Source::File(value.to_string())
    }
}

impl From<String> for Source {
    fn from(value: String) -> Self {
        Source::from(value.as_str())
    }
}

/// Ranges recorded in one batched round that lie at most this far apart are
/// fetched as one request (mirrors `WINDOW_BRIDGE_BYTES` in the TS driver).
#[cfg(feature = "http")]
const BATCH_BRIDGE_BYTES: u64 = 4 * 1024;

/// Upper bound on the rounds of [`MdfIndex::batched_reads`].
#[cfg(feature = "http")]
const MAX_BATCH_ROUNDS: usize = 1_000;

/// Batched reads fetch a range larger than this as several concurrent
/// requests of at most this size (8 MiB): one connection to an object store
/// is far slower than several in parallel.
#[cfg(feature = "http")]
const PARALLEL_CHUNK_BYTES: u64 = 8 * 1024 * 1024;

/// Whole-channel URL reads fetch the batch at once (concurrently) only while
/// it needs at most this many bytes (256 MiB); larger reads stream one data
/// section at a time.
#[cfg(feature = "http")]
const BATCH_MEMORY_LIMIT: u64 = 256 * 1024 * 1024;

/// URL reader for streamed whole-channel reads: serves bytes already fetched
/// (deferred conversion blocks) from `store`, and fetches a range larger than
/// [`PARALLEL_CHUNK_BYTES`] as concurrent chunks.
#[cfg(feature = "http")]
struct ChunkedUrlReader {
    http: HttpRangeReader,
    store: FragmentStore,
}

#[cfg(feature = "http")]
impl ByteRangeReader for ChunkedUrlReader {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        if let crate::fragments::Coverage::Full(bytes) = self.store.assemble(offset, length)? {
            return Ok(bytes);
        }
        if length <= PARALLEL_CHUNK_BYTES {
            return self.http.read_range(offset, length);
        }
        let parts = self.http.read_ranges_parallel(&split_spans(vec![(offset, length)], PARALLEL_CHUNK_BYTES))?;
        let mut out = Vec::with_capacity(usize::try_from(length).unwrap_or(0));
        for part in parts {
            out.extend_from_slice(&part);
        }
        Ok(out)
    }
}

/// Split every `(offset, length)` span longer than `chunk` into consecutive
/// pieces of at most `chunk` bytes.
#[cfg(feature = "http")]
fn split_spans(spans: Vec<(u64, u64)>, chunk: u64) -> Vec<(u64, u64)> {
    let mut out = Vec::with_capacity(spans.len());
    for (offset, length) in spans {
        let mut at = 0;
        while at < length {
            let take = chunk.min(length - at);
            out.push((offset + at, take));
            at += take;
        }
    }
    out
}

/// Reader over an index's attached [`Source`], for windowed reads.
enum SourceReader {
    #[cfg(not(target_arch = "wasm32"))]
    File(MmapRangeReader),
    #[cfg(feature = "http")]
    Url(CachingRangeReader<HttpRangeReader>),
}

impl ByteRangeReader for SourceReader {
    type Error = MdfError;

    #[allow(unused_variables)]
    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        match *self {
            #[cfg(not(target_arch = "wasm32"))]
            SourceReader::File(ref mut r) => r.read_range(offset, length),
            #[cfg(feature = "http")]
            SourceReader::Url(ref mut r) => r.read_range(offset, length),
        }
    }
}

/// Record windows resolved during one `read_many`, keyed by the time axis
/// searched (`None` for a record window) and the group's record count.
type WindowCache = HashMap<(Option<(usize, usize)>, u64), (u64, u64)>;

/// Time axes decoded during one `read_many`, keyed by the master's
/// `(group, channel)` and the record window `(start, count)`.
type AxisCache = HashMap<(usize, usize, u64, u64), Vec<f64>>;

/// Complete MDF file index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdfIndex {
    /// File size for validation
    pub file_size: u64,
    /// Start time of the measurement in nanoseconds since epoch (from MDF header)
    /// None if the start time is not set (0) in the file
    pub start_time_ns: Option<u64>,
    /// Channel groups in the file
    pub channel_groups: Vec<IndexedChannelGroup>,
    /// The data source for lazy value reads. Populated by `from_file` /
    /// `from_url`, re-attachable after load via `set_source`. Never
    /// serialized — an index file is portable; the source is environment-local.
    #[serde(skip)]
    pub source: Option<Source>,
}

/// Trait for reading byte ranges from different sources (files, HTTP, etc.)
pub trait ByteRangeReader {
    type Error;

    /// Read bytes from the specified range
    /// Returns the requested bytes or an error
    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, Self::Error>;

    /// Read an *optional* metadata range that the walk can proceed without.
    ///
    /// Behaves exactly like [`read_range`](Self::read_range) by default, so
    /// on-demand readers (file, HTTP) are unaffected. Incremental readers that
    /// *gather* the ranges they still need — rather than fetching one at a time
    /// — override this to record a gap and return `Ok(None)` instead of
    /// aborting. That lets a single metadata walk collect **every** still-missing
    /// leaf range (channel names, units, group comments) at once, turning an
    /// O(N²) fetch-restart loop into a handful of passes. Returning `None` means
    /// "these bytes are not available yet"; the caller treats the value as
    /// absent for this pass. Structural reads (block headers whose bytes yield
    /// the next link address) must keep using [`read_range`](Self::read_range) —
    /// the walk cannot continue past those without the real bytes.
    fn read_range_optional(
        &mut self,
        offset: u64,
        length: u64,
    ) -> Result<Option<Vec<u8>>, Self::Error> {
        self.read_range(offset, length).map(Some)
    }

    /// True when this reader is a gap-discovery probe: it cannot serve every
    /// range and records the misses instead of returning data. Metadata walks
    /// may then skip an unreadable subtree and keep discovering gaps elsewhere
    /// rather than aborting on the first miss.
    ///
    /// A walk driven by a probing reader produces a **partial** result: callers
    /// MUST discard it and retry once the recorded ranges have been supplied.
    fn is_probing(&self) -> bool {
        false
    }
}

/// Local file reader implementation.
///
/// Not available on `wasm32-unknown-unknown`; implement [`ByteRangeReader`] over
/// a `Cursor<Vec<u8>>` or a JS `Blob`-backed reader instead.
#[cfg(not(target_arch = "wasm32"))]
pub struct FileRangeReader {
    file: std::fs::File,
}

#[cfg(not(target_arch = "wasm32"))]
impl FileRangeReader {
    pub fn new(file_path: &str) -> Result<Self, MdfError> {
        let file = std::fs::File::open(file_path)
            .map_err(MdfError::IOError)?;
        Ok(Self { file })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ByteRangeReader for FileRangeReader {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, Self::Error> {
        use std::io::{Read, Seek, SeekFrom};

        // Validate against the real file length *before* allocating: a forged
        // or stale index could otherwise request an absurd length and abort
        // the process on allocation failure.
        let file_len = self.file.metadata().map_err(MdfError::IOError)?.len();
        match offset.checked_add(length) {
            Some(end) if end <= file_len => {}
            _ => {
                return Err(MdfError::TooShortBuffer {
                    actual: file_len as usize,
                    expected: offset.saturating_add(length) as usize,
                    file: file!(),
                    line: line!(),
                });
            }
        }

        self.file.seek(SeekFrom::Start(offset))
            .map_err(MdfError::IOError)?;

        let mut buffer = vec![0u8; length as usize];
        self.file.read_exact(&mut buffer)
            .map_err(MdfError::IOError)?;

        Ok(buffer)
    }
}

/// Memory-mapped file reader implementation.
///
/// Not available on `wasm32-unknown-unknown`.
#[cfg(not(target_arch = "wasm32"))]
pub struct MmapRangeReader {
    mmap: memmap2::Mmap,
}

#[cfg(not(target_arch = "wasm32"))]
impl MmapRangeReader {
    pub fn new(file_path: &str) -> Result<Self, MdfError> {
        let file = std::fs::File::open(file_path).map_err(MdfError::IOError)?;
        let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(MdfError::IOError)?;
        Ok(Self { mmap })
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ByteRangeReader for MmapRangeReader {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, Self::Error> {
        // Checked arithmetic: a forged offset/length must error, not wrap.
        let end = match offset
            .checked_add(length)
            .and_then(|e| usize::try_from(e).ok())
        {
            Some(end) if end <= self.mmap.len() => end,
            _ => {
                return Err(MdfError::TooShortBuffer {
                    actual: self.mmap.len(),
                    expected: offset.saturating_add(length).min(usize::MAX as u64) as usize,
                    file: file!(),
                    line: line!(),
                });
            }
        };
        Ok(self.mmap[offset as usize..end].to_vec())
    }
}

/// In-memory byte-slice reader — available on all targets including WASM.
///
/// Wraps an owned `Vec<u8>` and satisfies [`ByteRangeReader`] by slicing
/// directly into it.  Useful when the entire file has already been loaded
/// into memory (e.g. via `Blob.arrayBuffer()` in a browser Worker).
pub struct SliceRangeReader {
    data: Vec<u8>,
}

impl SliceRangeReader {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }
}

impl ByteRangeReader for SliceRangeReader {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, Self::Error> {
        // Checked arithmetic: a forged offset/length must error, not wrap.
        let end = match offset
            .checked_add(length)
            .and_then(|e| usize::try_from(e).ok())
        {
            Some(end) if end <= self.data.len() => end,
            _ => {
                return Err(MdfError::TooShortBuffer {
                    actual: self.data.len(),
                    expected: offset.saturating_add(length).min(usize::MAX as u64) as usize,
                    file: file!(),
                    line: line!(),
                });
            }
        };
        Ok(self.data[offset as usize..end].to_vec())
    }
}

/// Borrowing counterpart to [`SliceRangeReader`] — a [`ByteRangeReader`] over a
/// borrowed `&[u8]` (e.g. a memory map), so the mmap-based index paths can
/// reuse the generic reader logic without cloning the whole file into a `Vec`.
struct BorrowedSliceReader<'a> {
    data: &'a [u8],
}

impl<'a> ByteRangeReader for BorrowedSliceReader<'a> {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, Self::Error> {
        // Checked arithmetic: a forged offset/length must error, not wrap.
        let end = match offset
            .checked_add(length)
            .and_then(|e| usize::try_from(e).ok())
        {
            Some(end) if end <= self.data.len() => end,
            _ => {
                return Err(MdfError::TooShortBuffer {
                    actual: self.data.len(),
                    expected: offset.saturating_add(length).min(usize::MAX as u64) as usize,
                    file: file!(),
                    line: line!(),
                });
            }
        };
        Ok(self.data[offset as usize..end].to_vec())
    }
}

/// Caching wrapper around any [`ByteRangeReader`].
///
/// During the metadata phase of building an [`MdfIndex`], the parser issues
/// many small reads (block headers, channel metadata, text blocks). Without
/// caching, each becomes a round-trip on a remote backend. `CachingRangeReader`
/// fetches in larger aligned chunks (default 1 MiB) and serves overlapping
/// reads from memory, collapsing hundreds of small reads into a handful of
/// underlying requests.
///
/// For value reads — which span large slices of data blocks — call
/// [`CachingRangeReader::set_bypass`] to forward each read directly to the
/// underlying reader without populating the cache.
pub struct CachingRangeReader<R: ByteRangeReader<Error = MdfError>> {
    inner: R,
    chunks: std::collections::BTreeMap<u64, Vec<u8>>,
    chunk_size: u64,
    bypass: bool,
    underlying_requests: u64,
    cache_hits: u64,
    /// Total size of the underlying resource, once learned. Whole-chunk
    /// fetches are clamped against it so strict inner readers (which error on
    /// reads past EOF) still work when the resource size is not a multiple of
    /// `chunk_size`.
    known_size: Option<u64>,
}

impl<R: ByteRangeReader<Error = MdfError>> CachingRangeReader<R> {
    /// The wrapped reader.
    #[allow(dead_code)]
    pub(crate) fn inner_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// The wrapped reader and every cached chunk as `(offset, bytes)`.
    #[allow(dead_code)]
    pub(crate) fn into_parts(self) -> (R, Vec<(u64, Vec<u8>)>) {
        let chunk_size = self.chunk_size;
        let chunks = self
            .chunks
            .into_iter()
            .filter(|(_, bytes)| !bytes.is_empty())
            .map(|(i, bytes)| (i * chunk_size, bytes))
            .collect();
        (self.inner, chunks)
    }

    /// Wrap a reader with the default chunk size (1 MiB).
    pub fn new(inner: R) -> Self {
        Self::with_chunk_size(inner, 1 << 20)
    }

    /// Wrap a reader with a custom chunk size in bytes.
    pub fn with_chunk_size(inner: R, chunk_size: u64) -> Self {
        assert!(chunk_size > 0, "chunk_size must be > 0");
        Self {
            inner,
            chunks: std::collections::BTreeMap::new(),
            chunk_size,
            bypass: false,
            underlying_requests: 0,
            cache_hits: 0,
            known_size: None,
        }
    }

    /// Tell the cache the total size of the underlying resource (e.g. from an
    /// HTTP size probe), so the final chunk is requested at its true length
    /// instead of failing short and being re-requested.
    pub fn set_known_size(&mut self, size: u64) {
        self.known_size = Some(size);
    }

    /// When set, reads are served from already-cached chunks when fully
    /// covered, and otherwise forward directly to the underlying reader without
    /// populating the cache. Use during value-read phases so large data-block
    /// fetches do not pollute the cache, while small metadata reads that were
    /// already pulled in during the index walk (such as a conversion block
    /// resolved lazily on first read) remain free.
    pub fn set_bypass(&mut self, bypass: bool) {
        self.bypass = bypass;
    }

    /// Number of read calls forwarded to the underlying reader.
    pub fn underlying_requests(&self) -> u64 {
        self.underlying_requests
    }

    /// Number of read calls fully satisfied from cache.
    pub fn cache_hits(&self) -> u64 {
        self.cache_hits
    }

    /// Pre-fetch a contiguous range into the cache.
    pub fn prefetch(&mut self, offset: u64, length: u64) -> Result<(), MdfError> {
        if length == 0 {
            return Ok(());
        }
        self.read_range(offset, length).map(|_| ())
    }

    /// Try to satisfy a read entirely from chunks already in the cache,
    /// without issuing any underlying request. Returns `None` if any covering
    /// chunk is absent or too short (e.g. an EOF-clamped tail), leaving the
    /// caller to fetch. Used in bypass mode so small reads that land in
    /// already-fetched metadata chunks (e.g. a lazily-resolved conversion
    /// block) stay free, while large uncached data reads still forward straight
    /// through.
    fn try_serve_from_cache(&self, offset: u64, length: u64) -> Option<Vec<u8>> {
        let first = offset / self.chunk_size;
        let last = (offset + length - 1) / self.chunk_size;
        let read_end = offset + length;
        for i in first..=last {
            let chunk = self.chunks.get(&i)?;
            let chunk_start = i * self.chunk_size;
            let cached_end = chunk_start + chunk.len() as u64;
            let needed_end = read_end.min(chunk_start + self.chunk_size);
            if cached_end < needed_end {
                return None;
            }
        }
        let mut out = Vec::with_capacity(length.min(1 << 24) as usize);
        let mut remaining = length as usize;
        let mut cursor = offset;
        while remaining > 0 {
            let chunk_index = cursor / self.chunk_size;
            let chunk_offset = (cursor % self.chunk_size) as usize;
            let chunk = self.chunks.get(&chunk_index)?;
            if chunk_offset >= chunk.len() {
                return None;
            }
            let take = std::cmp::min(remaining, chunk.len() - chunk_offset);
            out.extend_from_slice(&chunk[chunk_offset..chunk_offset + take]);
            cursor += take as u64;
            remaining -= take;
            if take == 0 {
                return None;
            }
        }
        Some(out)
    }

    fn ensure_chunks(&mut self, first: u64, last: u64, requested_end: u64) -> Result<(), MdfError> {
        // Walk [first..=last], find each contiguous run of missing chunks,
        // issue one read per run.
        let mut idx = first;
        while idx <= last {
            if self.chunks.contains_key(&idx) {
                idx += 1;
                continue;
            }
            let run_start = idx;
            while idx <= last && !self.chunks.contains_key(&idx) {
                idx += 1;
            }
            let run_end = idx - 1;
            let read_offset = run_start * self.chunk_size;
            let mut read_len = (run_end - run_start + 1) * self.chunk_size;

            // Clamp against the known end of the resource: strict inner
            // readers reject reads past EOF, and the final chunk is allowed
            // to be short.
            if let Some(size) = self.known_size {
                if read_offset >= size {
                    for slot in run_start..=run_end {
                        self.chunks.insert(slot, Vec::new());
                    }
                    continue;
                }
                read_len = read_len.min(size - read_offset);
            }

            let bytes = match self.inner.read_range(read_offset, read_len) {
                Ok(b) => {
                    self.underlying_requests += 1;
                    b
                }
                Err(e) => {
                    self.underlying_requests += 1;
                    // The whole-chunk read may extend past EOF (the resource
                    // size need not be a multiple of chunk_size). Learn the
                    // true end from the error when it reports one, otherwise
                    // fall back to the span the caller actually asked for,
                    // and retry clamped.
                    let clamp_to = match &e {
                        MdfError::TooShortBuffer { actual, .. }
                            if (*actual as u64) > read_offset
                                && (*actual as u64) < read_offset + read_len =>
                        {
                            self.known_size = Some(*actual as u64);
                            *actual as u64
                        }
                        _ if requested_end > read_offset
                            && requested_end < read_offset + read_len =>
                        {
                            requested_end
                        }
                        _ => return Err(e),
                    };
                    let b = self.inner.read_range(read_offset, clamp_to - read_offset)?;
                    self.underlying_requests += 1;
                    b
                }
            };

            // Split the response into chunk-sized pieces. The last chunk
            // may be short if the file ends partway through it.
            for (i, slot) in (run_start..=run_end).enumerate() {
                let start = i * self.chunk_size as usize;
                if start >= bytes.len() {
                    self.chunks.insert(slot, Vec::new());
                    continue;
                }
                let end = std::cmp::min(start + self.chunk_size as usize, bytes.len());
                self.chunks.insert(slot, bytes[start..end].to_vec());
            }
        }
        Ok(())
    }
}

impl<R: ByteRangeReader<Error = MdfError>> ByteRangeReader for CachingRangeReader<R> {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        if length == 0 {
            return Ok(Vec::new());
        }
        if self.bypass {
            // Serve from already-cached chunks when possible (e.g. a small
            // conversion block lazily resolved at read time that lands in a
            // metadata chunk fetched during the index walk); otherwise forward
            // the (typically large data-block) read straight through.
            if let Some(bytes) = self.try_serve_from_cache(offset, length) {
                self.cache_hits += 1;
                return Ok(bytes);
            }
            let bytes = self.inner.read_range(offset, length)?;
            self.underlying_requests += 1;
            return Ok(bytes);
        }

        let first = offset / self.chunk_size;
        let last = (offset + length - 1) / self.chunk_size;
        let read_end = offset + length;

        // A chunk cached from an earlier EOF-clamped read may be shorter than
        // the span this read needs from it. Drop such chunks so ensure_chunks
        // refetches them (clamped to this read's span or the known size).
        for i in first..=last {
            if let Some(chunk) = self.chunks.get(&i) {
                let chunk_start = i * self.chunk_size;
                let cached_end = chunk_start + chunk.len() as u64;
                let mut needed_end = read_end.min(chunk_start + self.chunk_size);
                if let Some(size) = self.known_size {
                    needed_end = needed_end.min(size);
                }
                if cached_end < needed_end {
                    self.chunks.remove(&i);
                }
            }
        }

        // Track cache-hit metric only when no underlying read is required.
        let need_fetch = (first..=last).any(|i| !self.chunks.contains_key(&i));
        self.ensure_chunks(first, last, read_end)?;
        if !need_fetch {
            self.cache_hits += 1;
        }

        // Capacity capped so a forged huge length cannot abort on allocation.
        let mut out = Vec::with_capacity(length.min(1 << 24) as usize);
        let mut remaining = length as usize;
        let mut cursor = offset;
        while remaining > 0 {
            let chunk_index = cursor / self.chunk_size;
            let chunk_offset = (cursor % self.chunk_size) as usize;
            let chunk = self.chunks.get(&chunk_index).expect("chunk fetched above");
            if chunk_offset >= chunk.len() {
                return Err(MdfError::TooShortBuffer {
                    actual: chunk.len(),
                    expected: chunk_offset + 1,
                    file: file!(),
                    line: line!(),
                });
            }
            let take = std::cmp::min(remaining, chunk.len() - chunk_offset);
            out.extend_from_slice(&chunk[chunk_offset..chunk_offset + take]);
            cursor += take as u64;
            remaining -= take;
            if take == 0 {
                break;
            }
        }

        if out.len() != length as usize {
            return Err(MdfError::TooShortBuffer {
                actual: out.len(),
                expected: length as usize,
                file: file!(),
                line: line!(),
            });
        }

        Ok(out)
    }
}

/// HTTP range-request reader using the synchronous [`ureq`] client.
///
/// Each [`ByteRangeReader::read_range`] call issues a single
/// `Range: bytes=A-B` GET request and expects an HTTP 206 response. The
/// underlying `ureq::Agent` is reused across calls so TCP keep-alive applies.
///
/// Wrap this in [`CachingRangeReader`] when building an index, otherwise the
/// many small metadata reads will each become a separate round-trip.
#[cfg(feature = "http")]
pub struct HttpRangeReader {
    agent: ureq::Agent,
    url: String,
    request_count: u64,
    /// Size the resource must have (the size an index was built from). Every
    /// ranged response reports the resource's total size in `Content-Range`;
    /// a different total means the file behind the URL was replaced, and is
    /// refused instead of decoding bytes at offsets meant for another file.
    expected_size: Option<u64>,
}

#[cfg(feature = "http")]
impl HttpRangeReader {
    pub fn new(url: impl Into<String>) -> Result<Self, MdfError> {
        Ok(Self {
            agent: shared_http_agent()?,
            url: url.into(),
            request_count: 0,
            expected_size: None,
        })
    }

    /// Require every response to report a resource of `size` bytes (see
    /// [`MdfError::InvalidArgument`] for the mismatch error).
    pub fn expect_size(&mut self, size: u64) {
        self.expected_size = Some(size);
    }

    /// Fetch several byte ranges concurrently (up to
    /// [`MAX_PARALLEL_REQUESTS`] at a time) and return their bytes in order.
    ///
    /// Used by the batched read driver ([`MdfIndex`]'s URL reads): a round of
    /// a time-window search needs a few dozen small, independent ranges, and
    /// fetching them one after another would cost one round-trip each.
    pub(crate) fn read_ranges_parallel(&mut self, spans: &[(u64, u64)]) -> Result<Vec<Vec<u8>>, MdfError> {
        if spans.len() <= 1 {
            return spans.iter().map(|&(o, l)| self.read_range(o, l)).collect();
        }
        let (agent, url, expected) = (&self.agent, self.url.as_str(), self.expected_size);
        let mut out: Vec<Vec<u8>> = Vec::with_capacity(spans.len());
        for chunk in spans.chunks(MAX_PARALLEL_REQUESTS) {
            let results: Vec<Result<Vec<u8>, MdfError>> = std::thread::scope(|scope| {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|&(offset, length)| scope.spawn(move || http_get_range(agent, url, offset, length, expected)))
                    .collect();
                handles
                    .into_iter()
                    .map(|h| {
                        h.join().unwrap_or_else(|_| {
                            Err(MdfError::BlockSerializationError("HTTP fetch thread panicked".to_string()))
                        })
                    })
                    .collect()
            });
            self.request_count += chunk.len() as u64;
            for r in results {
                out.push(r?);
            }
        }
        Ok(out)
    }

    /// Download the whole resource with one plain `GET`.
    pub(crate) fn fetch_all(&mut self) -> Result<Vec<u8>, MdfError> {
        use std::io::Read;
        self.request_count += 1;
        let (agent, url) = (&self.agent, self.url.as_str());
        with_retries(|| {
            let response = agent.get(url).call().map_err(classify_call_error)?;
            let mut body = Vec::new();
            response
                .into_reader()
                .read_to_end(&mut body)
                .map_err(|e| HttpFailure::Transient(MdfError::IOError(e), None))?;
            Ok(body)
        })
    }

    /// Learn the resource's total size with a single-byte ranged GET.
    ///
    /// Uses `GET` with `Range: bytes=0-0` and parses the total length out of
    /// the `Content-Range: bytes 0-0/<total>` header, rather than issuing a
    /// `HEAD`. Presigned URLs (e.g. AWS S3) are signed for one specific HTTP
    /// method, so a `HEAD` against a GET-signed URL is rejected with 403; a
    /// ranged `GET` matches the method the data reads use.
    pub fn probe_size(&mut self) -> Result<u64, MdfError> {
        use std::io::Read;

        let (agent, url) = (&self.agent, self.url.as_str());
        let resp = with_retries(|| agent.get(url).set("Range", "bytes=0-0").call().map_err(classify_call_error))?;
        self.request_count += 1;

        // Preferred: total size from the Content-Range header of a 206 response.
        let total = resp
            .header("Content-Range")
            .and_then(|cr| cr.rsplit('/').next().map(|s| s.trim().to_string()))
            .filter(|s| s != "*")
            .and_then(|s| s.parse::<u64>().ok());

        // Fallback: a server that ignores the Range header replies 200 with the
        // full body, in which case Content-Length is the total size. Drain the
        // body so the connection can be reused for keep-alive.
        let content_length = resp
            .header("Content-Length")
            .and_then(|s| s.parse::<u64>().ok());
        let status = resp.status();
        let mut sink = Vec::new();
        let _ = resp.into_reader().take(1).read_to_end(&mut sink);

        if let Some(len) = total {
            Ok(len)
        } else if status == 200 {
            content_length.ok_or_else(|| {
                MdfError::BlockSerializationError(
                    "size probe: server ignored Range and sent no Content-Length".into(),
                )
            })
        } else {
            Err(MdfError::BlockSerializationError(
                "size probe: 206 response missing Content-Range total".into(),
            ))
        }
    }

    /// Total number of HTTP requests issued by this reader.
    pub fn request_count(&self) -> u64 {
        self.request_count
    }
}

#[cfg(feature = "http")]
impl ByteRangeReader for HttpRangeReader {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        let bytes = http_get_range(&self.agent, &self.url, offset, length, self.expected_size);
        if length > 0 {
            self.request_count += 1;
        }
        bytes
    }
}

/// Maximum concurrent range requests issued by
/// [`HttpRangeReader::read_ranges_parallel`].
#[cfg(feature = "http")]
pub(crate) const MAX_PARALLEL_REQUESTS: usize = 8;

/// The process-wide HTTP agent shared by every [`HttpRangeReader`].
///
/// Building a TLS connector loads the system trust store, and each agent has
/// its own connection pool, so a fresh agent per read meant a fresh TCP + TLS
/// handshake for every `read()` / `values()` call. One shared agent keeps
/// connections alive across calls (and across indexes reading the same host).
#[cfg(feature = "http")]
fn shared_http_agent() -> Result<ureq::Agent, MdfError> {
    // Keyed by process id: a forked child (multiprocessing, DataLoader
    // workers) must not reuse the parent's pooled keep-alive sockets, or
    // several processes would interleave requests on one TCP stream.
    static AGENT: std::sync::Mutex<Option<(u32, ureq::Agent)>> = std::sync::Mutex::new(None);
    let pid = std::process::id();
    let mut slot = AGENT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((owner, agent)) = slot.as_ref()
        && *owner == pid
    {
        return Ok(agent.clone());
    }
    // ureq's native-tls feature is not auto-wired (unlike rustls): the
    // connector must be constructed and attached explicitly, otherwise
    // HTTPS requests fail with "no TLS backend is configured".
    let connector = native_tls::TlsConnector::new().map_err(|e| {
        MdfError::BlockSerializationError(format!("failed to init TLS backend: {e}"))
    })?;
    let agent = ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(connector))
        .max_idle_connections_per_host(MAX_PARALLEL_REQUESTS)
        // Without a read timeout a stalled connection would block a read
        // (and the Python caller) forever; a timed-out attempt is retried.
        .timeout_connect(HTTP_CONNECT_TIMEOUT)
        .timeout_read(HTTP_READ_TIMEOUT)
        .timeout_write(HTTP_READ_TIMEOUT)
        .build();
    *slot = Some((pid, agent.clone()));
    Ok(agent)
}

/// Time allowed to establish a connection.
#[cfg(feature = "http")]
const HTTP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Longest wait for the next bytes of a response (a stall, not a slow
/// download: every socket read restarts it).
#[cfg(feature = "http")]
const HTTP_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Retries of a request that failed transiently (connection error, timeout,
/// `408` / `429` / `5xx`) before giving up.
#[cfg(feature = "http")]
const HTTP_MAX_RETRIES: u32 = 3;

/// Delay before the first retry; doubled for each further one.
#[cfg(feature = "http")]
const HTTP_RETRY_BASE_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

/// Upper bound on a single retry delay, including a server's `Retry-After`.
#[cfg(feature = "http")]
const HTTP_MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(10);

/// Outcome of one failed HTTP attempt.
#[cfg(feature = "http")]
enum HttpFailure {
    /// Worth retrying (connection error, timeout, throttling, server
    /// error), optionally after the delay the server asked for.
    Transient(MdfError, Option<std::time::Duration>),
    /// Retrying would not help (`404`, `403`, a wrong range, a replaced file).
    Fatal(MdfError),
}

/// Statuses that signal a temporary condition: request timeout, throttling
/// (S3 `503 SlowDown`, `429`) and server/gateway errors.
#[cfg(feature = "http")]
fn is_transient_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
}

/// A `Retry-After: <seconds>` header (the HTTP-date form is ignored).
#[cfg(feature = "http")]
fn retry_after(resp: &ureq::Response) -> Option<std::time::Duration> {
    resp.header("Retry-After")?.trim().parse::<u64>().ok().map(std::time::Duration::from_secs)
}

/// Classify a failed `call()`: connection/IO errors and transient statuses
/// are retried; every other status and transport error is final.
#[cfg(feature = "http")]
fn classify_call_error(error: ureq::Error) -> HttpFailure {
    match error {
        ureq::Error::Status(status, resp) if is_transient_status(status) => {
            let after = retry_after(&resp);
            HttpFailure::Transient(
                MdfError::BlockSerializationError(format!("HTTP request failed with status {status}")),
                after,
            )
        }
        ureq::Error::Status(status, _) => {
            HttpFailure::Fatal(MdfError::BlockSerializationError(format!("HTTP request failed with status {status}")))
        }
        ureq::Error::Transport(transport) => {
            let error = MdfError::BlockSerializationError(format!("HTTP request failed: {transport}"));
            // A dropped or refused connection is worth another try; a bad
            // URL, an unknown host or a proxy misconfiguration is not.
            match transport.kind() {
                ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::Io | ureq::ErrorKind::ProxyConnect => {
                    HttpFailure::Transient(error, None)
                }
                _ => HttpFailure::Fatal(error),
            }
        }
    }
}

/// Run `attempt` until it succeeds, fails for good, or has been retried
/// [`HTTP_MAX_RETRIES`] times, sleeping with exponential backoff (or the
/// server's `Retry-After`, capped) between attempts.
#[cfg(feature = "http")]
fn with_retries<T>(mut attempt: impl FnMut() -> Result<T, HttpFailure>) -> Result<T, MdfError> {
    let mut delay = HTTP_RETRY_BASE_DELAY;
    let mut retries = 0;
    loop {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(HttpFailure::Fatal(e)) => return Err(e),
            Err(HttpFailure::Transient(e, after)) => {
                if retries == HTTP_MAX_RETRIES {
                    return Err(match e {
                        MdfError::BlockSerializationError(msg) => MdfError::BlockSerializationError(format!(
                            "{msg} (gave up after {} attempts)",
                            HTTP_MAX_RETRIES + 1
                        )),
                        other => other,
                    });
                }
                std::thread::sleep(after.unwrap_or(delay).min(HTTP_MAX_RETRY_DELAY));
                delay *= 2;
                retries += 1;
            }
        }
    }
}

/// Total size from a `Content-Range: bytes A-B/<total>` (or `bytes */<total>`)
/// header, if it states one.
#[cfg(feature = "http")]
fn content_range_total(header: Option<&str>) -> Option<u64> {
    header?.rsplit_once('/')?.1.trim().parse().ok()
}

/// `(A, B)` from a `Content-Range: bytes A-B/<total>` header.
#[cfg(feature = "http")]
fn content_range_span(header: Option<&str>) -> Option<(u64, u64)> {
    let spec = header?.trim().strip_prefix("bytes")?.trim();
    let (a, b) = spec.split_once('/')?.0.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// Refuse a source whose size differs from the one the index was built from.
#[cfg(any(feature = "http", not(target_arch = "wasm32")))]
fn check_resource_size(source: &str, expected: u64, actual: u64) -> Result<(), MdfError> {
    if expected != 0 && actual != expected {
        return Err(MdfError::InvalidArgument(format!(
            "the source does not match the index: the index was built from a {expected}-byte \
             file but '{source}' is {actual} bytes — attach the file the index was built \
             from, or rebuild the index"
        )));
    }
    Ok(())
}

/// One `Range: bytes=A-B` GET of `[offset, offset + length)`, retried on
/// transient failures (see [`with_retries`]).
#[cfg(feature = "http")]
fn http_get_range(
    agent: &ureq::Agent,
    url: &str,
    offset: u64,
    length: u64,
    expected_size: Option<u64>,
) -> Result<Vec<u8>, MdfError> {
    if length == 0 {
        return Ok(Vec::new());
    }
    with_retries(|| http_get_range_once(agent, url, offset, length, expected_size))
}

/// A single attempt of [`http_get_range`].
#[cfg(feature = "http")]
fn http_get_range_once(
    agent: &ureq::Agent,
    url: &str,
    offset: u64,
    length: u64,
    expected_size: Option<u64>,
) -> Result<Vec<u8>, HttpFailure> {
    use std::io::Read;
    use HttpFailure::Fatal;

    let range_header = format!("bytes={}-{}", offset, offset + length - 1);
    let resp = match agent.get(url).set("Range", &range_header).call() {
        Ok(resp) => resp,
        // 416: the range starts past the end of the resource. Its
        // `Content-Range: bytes */<size>` tells whether the file shrank.
        Err(ureq::Error::Status(416, resp)) => {
            let total = content_range_total(resp.header("Content-Range"));
            if let (Some(expected), Some(actual)) = (expected_size, total) {
                check_resource_size(url, expected, actual).map_err(Fatal)?;
            }
            return Err(Fatal(MdfError::BlockSerializationError(format!(
                "HTTP range {offset}..{} is past the end of {url}{}",
                offset + length,
                total.map(|t| format!(" ({t} bytes)")).unwrap_or_default()
            ))));
        }
        Err(e) => return Err(classify_call_error(e)),
    };
    if let (Some(expected), Some(actual)) = (expected_size, content_range_total(resp.header("Content-Range"))) {
        check_resource_size(url, expected, actual).map_err(Fatal)?;
    }
    // A 206 must carry exactly the requested range: a proxy or a shared
    // connection that answers with other bytes of the right length would
    // otherwise be decoded silently.
    if resp.status() == 206 {
        if let Some((start, end)) = content_range_span(resp.header("Content-Range")) {
            let want_end = offset + length - 1;
            // (A shorter range at end-of-file is caught by the short-body check below.)
            if start != offset || end > want_end {
                return Err(Fatal(MdfError::BlockSerializationError(format!(
                    "HTTP server returned bytes {start}-{end} for requested range {offset}-{want_end}"
                ))));
            }
        }
    }

    // 206 Partial Content is the accepted path. A 200 means the server
    // ignored the Range header and sent the whole resource: that is only
    // usable when the requested range starts at offset 0 (the body can be
    // truncated to `length`); for any other offset the first `length`
    // bytes would silently come from the wrong position, so error out.
    let status = resp.status();
    match status {
        206 => {}
        200 => {
            if offset != 0 {
                return Err(Fatal(MdfError::BlockSerializationError(format!(
                    "HTTP server ignored Range request (status 200) for offset {}; \
                     refusing to return bytes from the wrong position",
                    offset
                ))));
            }
        }
        other => {
            return Err(Fatal(MdfError::BlockSerializationError(format!(
                "HTTP range request returned unexpected status {other}"
            ))));
        }
    }

    // Read at most `length` bytes (a 200 response carries the whole
    // resource and must be truncated), then insist we actually received
    // all of them: a short body must be an error, not silently fewer
    // records. Capacity is capped so a forged huge length cannot abort
    // on allocation.
    let mut buf = Vec::with_capacity(length.min(1 << 20) as usize);
    // A connection reset or read timeout mid-body is transient.
    resp.into_reader()
        .take(length)
        .read_to_end(&mut buf)
        .map_err(|e| HttpFailure::Transient(MdfError::IOError(e), None))?;
    if (buf.len() as u64) < length {
        // Report absolute positions so wrappers (e.g. CachingRangeReader)
        // can learn the resource's true end from `actual`.
        return Err(Fatal(MdfError::TooShortBuffer {
            actual: (offset + buf.len() as u64) as usize,
            expected: (offset + length) as usize,
            file: file!(),
            line: line!(),
        }));
    }
    Ok(buf)
}

impl MdfIndex {
    /// Create an index from an MDF file on disk.
    ///
    /// Not available on `wasm32-unknown-unknown`; use [`from_bytes`](Self::from_bytes) instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(file_path: &str) -> Result<Self, MdfError> {
        let mdf = Mdf::from_file(file_path)?;
        let file_size = std::fs::metadata(file_path)
            .map_err(MdfError::IOError)?
            .len();
        let mut index = Self::build_index(mdf, file_size)?;
        index.source = Some(Source::File(file_path.to_string()));
        Ok(index)
    }

    /// Build an index from an MDF file served over HTTP / S3 using range
    /// requests, remembering the URL as the index's [`Source`].
    ///
    /// Only metadata blocks are fetched while building; sample data is read
    /// lazily on [`MdfIndex::read`]. Requires the `http` feature.
    #[cfg(feature = "http")]
    pub fn from_url(url: &str) -> Result<Self, MdfError> {
        Self::from_url_with_chunk_size(url, 1 << 20)
    }

    /// [`MdfIndex::from_url`] with an explicit metadata read-ahead chunk size.
    #[cfg(feature = "http")]
    pub fn from_url_with_chunk_size(url: &str, chunk_size: u64) -> Result<Self, MdfError> {
        let mut http = HttpRangeReader::new(url)?;
        let file_size = http.probe_size()?;
        let mut cached = CachingRangeReader::with_chunk_size(http, chunk_size);
        cached.set_known_size(file_size);
        let mut index = Self::from_range_reader(&mut cached, file_size)?;
        index.source = Some(Source::Url(url.to_string()));
        Ok(index)
    }

    /// Shared index-building logic operating on an already-parsed [`MDF`].
    fn build_index(mdf: Mdf, file_size: u64) -> Result<Self, MdfError> {
        let start_time_ns = mdf.start_time_ns();
        let mut indexed_groups = Vec::new();
        // (own ##CG address, remote master ##CG address) per group.
        let mut cg_links: Vec<(u64, u64)> = Vec::new();

        for group in mdf.channel_groups() {
            let raw_cg = group.raw_channel_group();
            cg_links.push((raw_cg.addr, raw_cg.block.master_cg_addr));
            // Unsorted data groups (a record-ID prefix with several channel
            // groups sharing the same data blocks) would make every CG index
            // the same interleaved records as if they were all its own.
            let raw_dg = group.raw_data_group();
            if raw_dg.block.record_id_len > 0 && raw_dg.channel_groups.len() > 1 {
                return Err(MdfError::BlockSerializationError(
                    "unsorted data groups (record IDs with multiple channel groups) \
                     are not supported by the index"
                        .to_string(),
                ));
            }

            let mut indexed_channels = Vec::new();
            let mmap = group.mmap();

            for channel in group.channels() {
                let block = channel.block();

                let resolved_conversion = if let Some(mut conversion) = block.conversion.clone() {
                    if let Err(e) = conversion.resolve_all_dependencies(mmap) {
                        eprintln!("Warning: Failed to resolve conversion dependencies for channel '{}': {}",
                                 block.name.as_deref().unwrap_or("<unnamed>"), e);
                    }
                    Some(conversion)
                } else {
                    None
                };

                let vlsd_data_blocks = if block.channel_type == 1 && block.data != 0 {
                    let mut slice_reader = BorrowedSliceReader { data: mmap };
                    Self::extract_vlsd_data_blocks(&mut slice_reader, block.data)?
                } else {
                    Vec::new()
                };

                indexed_channels.push(IndexedChannel {
                    name: channel.name()?,
                    unit: channel.unit()?,
                    comment: channel.comment()?,
                    data_type: block.data_type.clone(),
                    byte_offset: block.byte_offset,
                    bit_offset: block.bit_offset,
                    bit_count: block.bit_count,
                    channel_type: block.channel_type,
                    flags: block.flags,
                    pos_invalidation_bit: block.pos_invalidation_bit,
                    conversion: resolved_conversion,
                    conversion_addr: block.conversion_addr,
                    vlsd_data_address: if block.channel_type == 1 && block.data != 0 {
                        Some(block.data)
                    } else {
                        None
                    },
                    vlsd_data_blocks,
                });
            }

            let data_blocks = Self::extract_data_blocks(&group)?;

            indexed_groups.push(IndexedChannelGroup {
                name: group.name()?,
                comment: group.comment()?,
                record_id_len: group.raw_data_group().block.record_id_len,
                record_size: group.raw_channel_group().block.samples_byte_nr,
                invalidation_bytes: group.raw_channel_group().block.invalidation_bytes_nr,
                record_count: group.raw_channel_group().block.cycles_nr,
                channels: indexed_channels,
                data_blocks,
                master_group: None,
            });
        }
        Self::link_remote_masters(&mut indexed_groups, &cg_links);

        Ok(MdfIndex { file_size, start_time_ns, channel_groups: indexed_groups, source: None })
    }

    /// Fill [`IndexedChannelGroup::master_group`] for groups without a local
    /// master whose `##CG` carries an MDF 4.2 remote-master link.
    fn link_remote_masters(groups: &mut [IndexedChannelGroup], cg_links: &[(u64, u64)]) {
        for g in 0..groups.len() {
            let target = cg_links[g].1;
            if target == 0 || groups[g].master_channel().is_some() {
                continue;
            }
            groups[g].master_group = cg_links
                .iter()
                .position(|&(addr, _)| addr == target)
                .filter(|&m| m != g && groups[m].master_channel().is_some());
        }
    }

    /// Extract data block information from a channel group.
    ///
    /// The same bounds-checked, cycle-checked walk as a range-reader build
    /// (over the in-memory file), reading every fragment header: a
    /// truncated file is an error, not a panic.
    fn extract_data_blocks(group: &crate::api::channel_group::ChannelGroup) -> Result<Vec<DataBlockInfo>, MdfError> {
        let mut reader = BorrowedSliceReader { data: group.mmap() };
        Self::extract_data_blocks_via_reader(&mut reader, group.raw_data_group().block.data_block_addr, None, 0)
    }

    /// Create an index from an in-memory MDF byte buffer.
    ///
    /// This is the primary constructor on `wasm32-unknown-unknown`.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, MdfError> {
        let file_size = data.len() as u64;
        let mdf = Mdf::from_bytes(data)?;
        Self::build_index(mdf, file_size)
    }

    /// Build an [`MdfIndex`] using only [`ByteRangeReader`] calls.
    ///
    /// Issues range reads for the file's metadata structures (identification,
    /// header, data groups, channel groups, channels, text and conversion
    /// blocks, and data-block headers) but never reads the sample data
    /// itself. Intended for remote sources such as HTTP-served files; wrap
    /// the underlying reader in [`CachingRangeReader`] to keep the number of
    /// underlying requests low.
    ///
    /// `file_size` is stored on the resulting index for use by later byte-range
    /// calculations. Callers should obtain it from a HEAD request (e.g.
    /// [`HttpRangeReader::probe_size`]) or other out-of-band metadata.
    pub fn from_range_reader<R>(
        reader: &mut R,
        file_size: u64,
    ) -> Result<Self, MdfError>
    where
        R: ByteRangeReader<Error = MdfError>,
    {
        use crate::parsing::reader_walk;

        let walk = reader_walk::walk(reader)?;

        let start_time_ns = if walk.header.abs_time == 0 {
            None
        } else {
            Some(walk.header.abs_time)
        };

        let mut indexed_groups = Vec::with_capacity(walk.groups.len());
        let cg_links: Vec<(u64, u64)> =
            walk.groups.iter().map(|g| (g.cg_addr, g.cg.master_cg_addr)).collect();
        for group in walk.groups {
            let mut indexed_channels = Vec::with_capacity(group.channels.len());
            for ch in group.channels {
                let block = ch.block;
                let vlsd_data_blocks = if block.channel_type == 1 && block.data != 0 {
                    Self::extract_vlsd_data_blocks(reader, block.data)?
                } else {
                    Vec::new()
                };
                indexed_channels.push(IndexedChannel {
                    name: ch.name,
                    unit: ch.unit,
                    comment: ch.comment,
                    data_type: block.data_type.clone(),
                    byte_offset: block.byte_offset,
                    bit_offset: block.bit_offset,
                    bit_count: block.bit_count,
                    channel_type: block.channel_type,
                    flags: block.flags,
                    pos_invalidation_bit: block.pos_invalidation_bit,
                    // Remote / range-reader builds defer conversion resolution:
                    // only the block's location is recorded here; it is fetched
                    // and resolved lazily on the first value read.
                    conversion: None,
                    conversion_addr: ch.conversion_addr,
                    vlsd_data_address: if block.channel_type == 1 && block.data != 0 {
                        Some(block.data)
                    } else {
                        None
                    },
                    vlsd_data_blocks,
                });
            }

            // Bytes of record data the group holds, so equal-length ##DL
            // lists can be sized without touching the data region.
            let expected_bytes = group.cg.cycles_nr.checked_mul(
                group.record_id_len as u64
                    + group.cg.samples_byte_nr as u64
                    + group.cg.invalidation_bytes_nr as u64,
            );
            let data_blocks = Self::extract_data_blocks_via_reader(
                reader,
                group.data_block_addr,
                expected_bytes,
                file_size,
            )?;

            indexed_groups.push(IndexedChannelGroup {
                name: group.cg_name,
                comment: group.cg_comment,
                record_id_len: group.record_id_len,
                record_size: group.cg.samples_byte_nr,
                invalidation_bytes: group.cg.invalidation_bytes_nr,
                record_count: group.cg.cycles_nr,
                channels: indexed_channels,
                data_blocks,
                master_group: None,
            });
        }
        Self::link_remote_masters(&mut indexed_groups, &cg_links);

        Ok(MdfIndex {
            file_size,
            start_time_ns,
            channel_groups: indexed_groups,
            source: None,
        })
    }

    /// Mirror of [`Self::extract_data_blocks`] that fetches headers via a
    /// [`ByteRangeReader`] instead of slicing into a memory map.
    ///
    /// When `reader.is_probing()` (an incremental gap-discovery pass — see
    /// [`ByteRangeReader::is_probing`]), a failed read stops walking this
    /// group's data-block chain and returns what was collected so far instead
    /// of propagating the error: the reader has already recorded the miss, so
    /// the caller can fetch it and retry. The resulting index is **partial**
    /// and must be discarded while any reader miss remains — this is exactly
    /// the batched gap-discovery contract `MdfIndex::from_range_reader`
    /// callers (`buildIndexStep`/`IndexBuilder::step`) already enforce by only
    /// reporting `done` once a pass records zero misses. On-demand readers
    /// (the default `is_probing() == false`) are unaffected: a failed read is
    /// still fatal, exactly as before.
    ///
    /// For an **equal-length** `##DL` list (`dl_flags` bit 0) reached directly
    /// from the data group, the fragment sizes follow from the list itself:
    /// every fragment but the last holds `dl_equal_length` bytes and the last
    /// holds the rest of `expected_bytes` (cycle count × record size). When
    /// that arithmetic is consistent with the number of fragments and the file
    /// size, the fragment headers are **not read** — on a column-oriented file
    /// whose metadata sits at the front this keeps the whole index build inside
    /// the metadata prefix (one or two requests, independent of the number of
    /// data blocks). Reads still verify each fragment header they fetch. Any
    /// inconsistency falls back to reading the headers.
    fn extract_data_blocks_via_reader<R>(
        reader: &mut R,
        data_block_addr: u64,
        expected_bytes: Option<u64>,
        file_size: u64,
    ) -> Result<Vec<DataBlockInfo>, MdfError>
    where
        R: ByteRangeReader<Error = MdfError>,
    {
        let mut data_blocks = Vec::new();

        // Read a range, but when probing and the read fails, stop walking
        // this chain and hand back what's been collected so far rather than
        // aborting the whole build.
        macro_rules! probe_read {
            ($offset:expr, $len:expr) => {
                match reader.read_range($offset, $len) {
                    Ok(b) => b,
                    Err(e) => {
                        if reader.is_probing() {
                            return Ok(data_blocks);
                        }
                        return Err(e);
                    }
                }
            };
        }

        let mut current_block_address = data_block_addr;
        let mut visited = std::collections::HashSet::new();

        while current_block_address != 0 {
            if !visited.insert(current_block_address) {
                return Err(MdfError::BlockLinkError(format!(
                    "cycle detected in data block chain at address {:#x}",
                    current_block_address
                )));
            }
            let header_bytes = probe_read!(current_block_address, 24);
            let block_header =
                crate::blocks::common::BlockHeader::from_bytes(&header_bytes)?;

            match block_header.id.as_str() {
                "##DT" | "##DV" => {
                    data_blocks.push(DataBlockInfo {
                        file_offset: current_block_address,
                        size: block_header.block_len,
                        is_compressed: false,
                    });
                    current_block_address = 0;
                }
                "##DZ" => {
                    data_blocks.push(DataBlockInfo {
                        file_offset: current_block_address,
                        size: block_header.block_len,
                        is_compressed: true,
                    });
                    current_block_address = 0;
                }
                "##DL" => {
                    let dl_bytes = probe_read!(current_block_address, block_header.block_len);
                    let data_list_block =
                        crate::blocks::data_list_block::DataListBlock::from_bytes(&dl_bytes)?;

                    if current_block_address == data_block_addr && data_list_block.next == 0
                        && let Some(derived) = Self::derive_equal_length_fragments(
                            &data_list_block,
                            expected_bytes,
                            file_size,
                        ) {
                            data_blocks.extend(derived);
                            break;
                        }

                    for &fragment_address in &data_list_block.data_links {
                        if fragment_address == 0 {
                            continue; // NIL link (skipped by the in-memory reader too)
                        }
                        let frag_header_bytes = probe_read!(fragment_address, 24);
                        let fragment_header = crate::blocks::common::BlockHeader::from_bytes(
                            &frag_header_bytes,
                        )?;
                        let is_compressed = fragment_header.id == "##DZ";
                        data_blocks.push(DataBlockInfo {
                            file_offset: fragment_address,
                            size: fragment_header.block_len,
                            is_compressed,
                        });
                    }

                    current_block_address = data_list_block.next;
                }
                unexpected_id => {
                    return Err(MdfError::BlockIDError {
                        actual: unexpected_id.to_string(),
                        expected: "##DT / ##DV / ##DL / ##DZ".to_string(),
                    });
                }
            }
        }

        Ok(data_blocks)
    }

    /// Fragment locations of an equal-length `##DL` derived without reading
    /// the fragments (see [`Self::extract_data_blocks_via_reader`]); `None`
    /// when the list is not equal-length or the numbers do not add up.
    fn derive_equal_length_fragments(
        dl: &crate::blocks::data_list_block::DataListBlock,
        expected_bytes: Option<u64>,
        file_size: u64,
    ) -> Option<Vec<DataBlockInfo>> {
        let equal = dl.data_block_len.filter(|_| dl.flags & 1 != 0)?;
        let total = expected_bytes?;
        let n = dl.data_links.len() as u64;
        if equal == 0 || n == 0 || total == 0 || dl.data_links.contains(&0) {
            return None;
        }
        // (n-1) full fragments plus a non-empty last one must hold exactly `total`.
        let full = (n - 1).checked_mul(equal)?;
        if total <= full || total > full.checked_add(equal)? {
            return None;
        }
        let last = total - full;
        let mut out: Vec<DataBlockInfo> = Vec::with_capacity(n as usize);
        for (i, &addr) in dl.data_links.iter().enumerate() {
            let data = if i as u64 + 1 == n { last } else { equal };
            let size = data.checked_add(24)?;
            if file_size > 0 && addr.checked_add(size)? > file_size {
                return None;
            }
            // Fragments must not overlap (ascending, each starting after the
            // previous one ends); anything else is left to the headers.
            if let Some(prev) = out.last()
                && addr < prev.file_offset.checked_add(prev.size)?
            {
                return None;
            }
            out.push(DataBlockInfo { file_offset: addr, size, is_compressed: false });
        }
        Some(out)
    }

    /// Read the data sections of `blocks` in order, coalescing blocks that sit
    /// back to back in the file (at most 7 bytes of alignment padding apart)
    /// into a single read of up to [`MAX_COALESCED_READ`] bytes, and call `f`
    /// with each block's data section.
    ///
    /// A column-oriented file stores each signal's blocks contiguously, so a
    /// whole signal arrives in one range request instead of one per block.
    /// Every read starts at a block header, and each block's header is
    /// verified (`##DT`/`##DV`) before its data is decoded. The header's own
    /// length is authoritative: when it disagrees with the indexed size (e.g.
    /// fragment sizes derived from an equal-length `##DL` and a stale cycle
    /// count), the block's real data section is used — re-read if it extends
    /// beyond the coalesced read — so bytes beyond a block are never decoded.
    fn for_each_data_section<R, F>(
        reader: &mut R,
        blocks: &[DataBlockInfo],
        mut f: F,
    ) -> Result<(), MdfError>
    where
        R: ByteRangeReader<Error = MdfError>,
        F: FnMut(&[u8]) -> Result<(), MdfError>,
    {
        // With a probing reader every missing section is recorded in one
        // pass (sections after a miss are requested but not decoded), so a
        // batched read needs one round, not one per section.
        let mut first_err = None;
        let mut i = 0;
        while i < blocks.len() {
            let start = blocks[i].file_offset;
            Self::data_section_size(&blocks[i])?;
            let mut end = start.checked_add(blocks[i].size).ok_or_else(|| {
                MdfError::BlockSerializationError("data block offset overflows".to_string())
            })?;
            let mut j = i + 1;
            while j < blocks.len() {
                let b = &blocks[j];
                let b_end = b.file_offset.saturating_add(b.size);
                if b.file_offset < end
                    || b.file_offset - end > 7
                    || b.size < 24
                    || b_end - start > MAX_COALESCED_READ
                {
                    break;
                }
                end = b_end;
                j += 1;
            }
            let buf = match reader.read_range(start, end - start) {
                Ok(buf) if first_err.is_none() => buf,
                Ok(_) => {
                    i = j;
                    continue;
                }
                Err(e) if reader.is_probing() => {
                    first_err.get_or_insert(e);
                    i = j;
                    continue;
                }
                Err(e) => return Err(e),
            };
            for b in &blocks[i..j] {
                let header_start = (b.file_offset - start) as usize;
                let data_start = header_start + 24;
                let len = Self::verified_data_len(buf.get(header_start..data_start), b)?;
                match usize::try_from(len).ok().and_then(|l| buf.get(data_start..data_start.checked_add(l)?)) {
                    Some(data) => f(data)?,
                    None => f(&reader.read_range(b.file_offset + 24, len)?)?,
                }
            }
            i = j;
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Check that `header` (the 24 header bytes of `block`, if available) is an
    /// uncompressed `##DT`/`##DV` block and return the length of its data
    /// section according to the header (which wins over the indexed size).
    fn verified_data_len(header: Option<&[u8]>, block: &DataBlockInfo) -> Result<u64, MdfError> {
        let h = BlockHeader::from_bytes(header.unwrap_or(&[]))?;
        if (h.id == "##DT" || h.id == "##DV") && h.block_len >= 24 {
            return Ok(h.block_len - 24);
        }
        Err(MdfError::BlockSerializationError(format!(
            "data block at offset {} is {} with length {} but the index expects an \
             uncompressed ##DT/##DV block of {} bytes (compressed or inconsistent \
             file; rebuild the index)",
            block.file_offset, h.id, h.block_len, block.size
        )))
    }

    /// Walk a VLSD channel's `data` chain (a single `##SD`, or a `##DL` chain
    /// of `##SD`/`##DZ` fragments) and record each fragment's location in chain
    /// order.
    ///
    /// Generic over [`ByteRangeReader`] so the mmap path (via
    /// [`BorrowedSliceReader`]) and the remote path share one implementation.
    /// Cycle-detects the `##DL` `next` chain, rejects unexpected block IDs, and
    /// validates that a variable-length `##DL`'s per-fragment virtual offsets
    /// reproduce the running sum of prior fragments' data-section sizes — the
    /// invariant the offset-based reader relies on.
    ///
    /// When `reader.is_probing()`, a failed read stops walking this VLSD
    /// chain and returns what was collected so far instead of propagating the
    /// error — see the identical rationale on
    /// [`Self::extract_data_blocks_via_reader`]. Non-probing readers keep
    /// today's fatal-on-first-miss behavior.
    fn extract_vlsd_data_blocks<R>(
        reader: &mut R,
        data_addr: u64,
    ) -> Result<Vec<DataBlockInfo>, MdfError>
    where
        R: ByteRangeReader<Error = MdfError>,
    {
        let mut data_blocks: Vec<DataBlockInfo> = Vec::new();

        // See `extract_data_blocks_via_reader`'s `probe_read!` for the
        // rationale: batched gap discovery skips an unreadable subtree
        // instead of aborting the whole pass, but only when probing.
        macro_rules! probe_read {
            ($offset:expr, $len:expr) => {
                match reader.read_range($offset, $len) {
                    Ok(b) => b,
                    Err(e) => {
                        if reader.is_probing() {
                            return Ok(data_blocks);
                        }
                        return Err(e);
                    }
                }
            };
        }

        let mut visited: std::collections::HashSet<u64> = std::collections::HashSet::new();
        // Running sum of fragment data-section sizes (size - 24), i.e. the
        // virtual start offset of the next fragment in the concatenated stream.
        let mut running_offset: u64 = 0;
        let mut current = data_addr;

        while current != 0 {
            let header_bytes = probe_read!(current, 24);
            let header = crate::blocks::common::BlockHeader::from_bytes(&header_bytes)?;

            match header.id.as_str() {
                "##SD" => {
                    data_blocks.push(DataBlockInfo {
                        file_offset: current,
                        size: header.block_len,
                        is_compressed: false,
                    });
                    current = 0;
                }
                "##DZ" => {
                    data_blocks.push(DataBlockInfo {
                        file_offset: current,
                        size: header.block_len,
                        is_compressed: true,
                    });
                    current = 0;
                }
                "##DL" => {
                    // Cycle detection on the DL chain: a chain revisiting an
                    // address would otherwise loop forever.
                    if !visited.insert(current) {
                        return Err(MdfError::BlockLinkError(format!(
                            "cycle detected in VLSD data list chain at address {:#x}",
                            current
                        )));
                    }
                    let dl_bytes = probe_read!(current, header.block_len);
                    let dl = crate::blocks::data_list_block::DataListBlock::from_bytes(&dl_bytes)?;

                    for (i, &fragment_address) in dl.data_links.iter().enumerate() {
                        if fragment_address == 0 {
                            continue;
                        }
                        let frag_header_bytes = probe_read!(fragment_address, 24);
                        let frag_header =
                            crate::blocks::common::BlockHeader::from_bytes(&frag_header_bytes)?;
                        let is_compressed = match frag_header.id.as_str() {
                            "##SD" => false,
                            "##DZ" => true,
                            other => {
                                return Err(MdfError::BlockIDError {
                                    actual: other.to_string(),
                                    expected: "##SD / ##DZ".to_string(),
                                });
                            }
                        };

                        // A variable-length DL carries an explicit virtual
                        // offset per fragment. The offset-based read path maps
                        // an inline offset to a fragment via cumulative
                        // data-section sizes, so a non-contiguous chain would
                        // silently mislocate entries — reject it here.
                        if let Some(offsets) = &dl.offsets
                            && let Some(&declared) = offsets.get(i)
                                && declared != running_offset {
                                    return Err(MdfError::BlockSerializationError(format!(
                                        "non-contiguous VLSD ##SD chain is unsupported: \
                                         fragment at offset {} declares virtual start {} \
                                         but the running data-section sum is {}",
                                        fragment_address, declared, running_offset
                                    )));
                                }

                        let data_section = frag_header.block_len.checked_sub(24).ok_or_else(|| {
                            MdfError::BlockSerializationError(format!(
                                "VLSD fragment at offset {} has size {} \
                                 (smaller than the 24-byte block header)",
                                fragment_address, frag_header.block_len
                            ))
                        })?;
                        running_offset = running_offset.checked_add(data_section).ok_or_else(|| {
                            MdfError::BlockSerializationError(
                                "VLSD ##SD chain total size overflows the address space"
                                    .to_string(),
                            )
                        })?;

                        data_blocks.push(DataBlockInfo {
                            file_offset: fragment_address,
                            size: frag_header.block_len,
                            is_compressed,
                        });
                    }

                    current = dl.next;
                }
                other => {
                    return Err(MdfError::BlockIDError {
                        actual: other.to_string(),
                        expected: "##SD / ##DL / ##DZ".to_string(),
                    });
                }
            }
        }

        Ok(data_blocks)
    }

    /// Save the index to a JSON file.
    ///
    /// Not available on `wasm32-unknown-unknown`; use [`to_json`](Self::to_json) instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save(&self, index_path: &str) -> Result<(), MdfError> {
        let json = self.to_json()?;
        std::fs::write(index_path, json)
            .map_err(MdfError::IOError)?;
        Ok(())
    }

    /// Load an index from a JSON file.
    ///
    /// Not available on `wasm32-unknown-unknown`; use [`from_json`](Self::from_json) instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load(index_path: &str) -> Result<Self, MdfError> {
        let json = std::fs::read_to_string(index_path)
            .map_err(MdfError::IOError)?;
        Self::from_json(&json)
    }

    /// Serialize the index to a JSON string (available on all targets).
    pub fn to_json(&self) -> Result<String, MdfError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| MdfError::BlockSerializationError(format!("JSON serialization failed: {}", e)))
    }

    /// Deserialize an index from a JSON string (available on all targets).
    ///
    /// The deserialized index is [validated](Self::validate) so hostile or
    /// stale index data errors here instead of panicking later during reads.
    pub fn from_json(json: &str) -> Result<Self, MdfError> {
        let index: Self = serde_json::from_str(json)
            .map_err(|e| MdfError::BlockSerializationError(format!("JSON deserialization failed: {}", e)))?;
        index.validate()?;
        Ok(index)
    }

    /// Validate structural invariants of the index.
    ///
    /// Called from [`Self::from_json`] / [`Self::load`] so that
    /// malformed (hostile or stale) index data returns an error instead of
    /// causing panics, integer underflow, or huge allocations during reads.
    pub fn validate(&self) -> Result<(), MdfError> {
        for group in &self.channel_groups {
            let group_name = group.name.as_deref().unwrap_or("<unnamed>");
            for db in &group.data_blocks {
                if db.size < 24 {
                    return Err(MdfError::BlockSerializationError(format!(
                        "invalid index: group '{}' data block at offset {} has size {} \
                         (smaller than the 24-byte block header)",
                        group_name, db.file_offset, db.size
                    )));
                }
                if db.file_offset.checked_add(db.size).is_none() {
                    return Err(MdfError::BlockSerializationError(format!(
                        "invalid index: group '{}' data block at offset {} with size {} \
                         overflows the file address space",
                        group_name, db.file_offset, db.size
                    )));
                }
            }
            if !group.data_blocks.is_empty() {
                Self::checked_record_size(group)?;
            }
            for channel in &group.channels {
                for db in &channel.vlsd_data_blocks {
                    if db.size < 24 {
                        return Err(MdfError::BlockSerializationError(format!(
                            "invalid index: group '{}' VLSD data block at offset {} has size {} \
                             (smaller than the 24-byte block header)",
                            group_name, db.file_offset, db.size
                        )));
                    }
                    if db.file_offset.checked_add(db.size).is_none() {
                        return Err(MdfError::BlockSerializationError(format!(
                            "invalid index: group '{}' VLSD data block at offset {} with size {} \
                             overflows the file address space",
                            group_name, db.file_offset, db.size
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Total on-disk record size (record id + data + invalidation bytes) for
    /// a group, validated non-zero so callers can safely divide by it.
    fn checked_record_size(group: &IndexedChannelGroup) -> Result<usize, MdfError> {
        let size = group.record_id_len as usize
            + group.record_size as usize
            + group.invalidation_bytes as usize;
        if size == 0 {
            return Err(MdfError::BlockSerializationError(format!(
                "invalid index: channel group '{}' has record size 0",
                group.name.as_deref().unwrap_or("<unnamed>")
            )));
        }
        Ok(size)
    }

    /// Size of a data block's data section (block size minus the 24-byte
    /// header), validated so a corrupt/forged block size cannot underflow.
    fn data_section_size(data_block: &DataBlockInfo) -> Result<u64, MdfError> {
        data_block.size.checked_sub(24).ok_or_else(|| {
            MdfError::BlockSerializationError(format!(
                "invalid index: data block at offset {} has size {} \
                 (smaller than the 24-byte block header)",
                data_block.file_offset, data_block.size
            ))
        })
    }

    /// Number of whole records in a data block's data section.
    ///
    /// Errors if the block size underflows the header or if the data section
    /// is not a multiple of the record size — a record spanning data-block
    /// fragments is unsupported and silently flooring would decode garbage
    /// for every record after the split.
    fn records_in_block(data_block: &DataBlockInfo, record_size: usize) -> Result<u64, MdfError> {
        let data_len = Self::data_section_size(data_block)?;
        if data_len % record_size as u64 != 0 {
            return Err(MdfError::BlockSerializationError(format!(
                "data block at offset {}: data section of {} bytes is not a multiple of \
                 the {}-byte record — record spans data block fragments, which is unsupported",
                data_block.file_offset, data_len, record_size
            )));
        }
        Ok(data_len / record_size as u64)
    }

    /// Error for VLSD channels on the byte-range calculation paths.
    ///
    /// A VLSD channel's fixed record field holds an 8-byte offset into a
    /// separate `##SD` stream, not the value bytes, so a static
    /// `(offset, length)` range cannot describe where a sample's payload
    /// lives. Value reads (`read` / `values`) resolve the indirection and are
    /// fully supported; only byte-range calculation refuses VLSD channels.
    fn ensure_not_vlsd(channel: &IndexedChannel) -> Result<(), MdfError> {
        if channel.channel_type == 1 {
            return Err(MdfError::BlockSerializationError(format!(
                "byte ranges cannot represent offset-indirected VLSD channel '{}'; \
                 use read()/values() instead",
                channel.name.as_deref().unwrap_or("<unnamed>")
            )));
        }
        Ok(())
    }

    /// Read channel values using the index and a byte range reader.
    ///
    /// Internal positional helper — the public entry point is
    /// [`MdfReader::values`], which resolves channels by name.
    ///
    /// # Returns
    /// A vector of `Option<DecodedValue>` where:
    /// - `Some(value)` represents a valid decoded value
    /// - `None` represents an invalid value (invalidation bit set or decoding failed)
    pub(crate) fn read_channel_values<R: ByteRangeReader<Error = MdfError>>(
        &self,
        group_index: usize,
        channel_index: usize,
        raw: bool,
        reader: &mut R
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        let conversion = Self::read_conversion(channel, reader, raw)?;

        if channel.is_virtual() {
            return self.virtual_values(group, 0, group.record_count, conversion.as_deref());
        }
        if channel.channel_type == 1 {
            return self.read_vlsd_channel_values(group, channel, conversion.as_deref(), reader);
        }

        // For regular channels, read from data blocks
        self.read_regular_channel_values(group, channel, conversion.as_deref(), reader)
    }

    /// Values of a virtual channel (`cn_type` 3/6) for records
    /// `[start, start + count)`: the raw value is the record index, converted
    /// through the channel's conversion (MDF 4.1 §6.6.3).
    fn virtual_values(
        &self,
        group: &IndexedChannelGroup,
        start: u64,
        count: u64,
        conversion: Option<&ConversionBlock>,
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        // Nothing is read for a virtual channel, so bound the record count by
        // stored data instead of trusting a possibly forged `record_count`:
        // the group's own data blocks, or — for a byte-less time group (the
        // virtual master `to_column_oriented` writes) — the largest
        // data-backed group using it as its remote master.
        let available = if !group.data_blocks.is_empty() {
            Self::stored_records(group)?
        } else {
            let me = self.channel_groups.iter().position(|g| std::ptr::eq(g, group));
            self.channel_groups
                .iter()
                .filter(|g| me.is_some() && g.master_group == me && !g.data_blocks.is_empty())
                .filter_map(|g| Self::stored_records(g).ok())
                .max()
                // Nothing stored to check against: trust the group.
                .unwrap_or(group.record_count)
        };
        match start.checked_add(count) {
            Some(end) if end <= available => {}
            _ => {
                return Err(MdfError::BlockSerializationError(format!(
                    "virtual channel read of {} records from {} exceeds the {} records the \
                     group's data holds (rebuild the index)",
                    count, start, available
                )))
            }
        }
        let mut values = Vec::with_capacity((count as usize).min(1 << 24));
        for i in start..start + count {
            let raw = DecodedValue::UnsignedInteger(i);
            values.push(Some(match conversion {
                Some(conv) => conv.apply_decoded(raw, &[])?,
                None => raw,
            }));
        }
        Ok(values)
    }

    /// Read the samples of the channel at `(g, c)` for the record window
    /// `[start, start + count)` only.
    ///
    /// For a regular channel only the bytes of the window's records are read
    /// (one range per data block the window touches, plus that block's
    /// 24-byte header, which is verified). A VLSD channel is read in full and
    /// sliced, since its payloads sit in a separate stream.
    pub(crate) fn read_channel_values_window<R: ByteRangeReader<Error = MdfError>>(
        &self,
        g: usize,
        c: usize,
        start: u64,
        count: u64,
        raw: bool,
        reader: &mut R,
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        let (group, channel) = self.group_and_channel(g, c)?;
        Self::check_window(group, start, count)?;
        let conversion = Self::read_conversion(channel, reader, raw)?;
        self.window_values(group, channel, conversion.as_deref(), start, count, reader)
    }

    fn window_values<R: ByteRangeReader<Error = MdfError>>(
        &self,
        group: &IndexedChannelGroup,
        channel: &IndexedChannel,
        conversion: Option<&ConversionBlock>,
        start: u64,
        count: u64,
        reader: &mut R,
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        if channel.is_virtual() {
            return self.virtual_values(group, start, count, conversion);
        }
        if channel.channel_type == 1 {
            return self.read_vlsd_window_values(group, channel, conversion, start, count, reader);
        }
        let record_size = Self::checked_record_size(group)?;
        let temp_cb = channel.to_decode_only_channel_block();
        let mut values = Vec::with_capacity((count as usize).min(1 << 24));
        Self::for_each_window_section(reader, group, record_size, start, count, |data| {
            Self::decode_records_to_values(data, record_size, group, conversion, &temp_cb, &mut values)
        })?;
        Ok(values)
    }

    /// `f64` variant of [`window_values`](Self::window_values) (invalid or
    /// non-numeric samples are NaN).
    fn window_values_f64<R: ByteRangeReader<Error = MdfError>>(
        &self,
        group: &IndexedChannelGroup,
        channel: &IndexedChannel,
        conversion: Option<&ConversionBlock>,
        start: u64,
        count: u64,
        reader: &mut R,
    ) -> Result<Vec<f64>, MdfError> {
        if channel.is_virtual() || channel.channel_type == 1 {
            let vals = self.window_values(group, channel, conversion, start, count, reader)?;
            return Ok(Self::vlsd_values_to_f64(vals));
        }
        let record_size = Self::checked_record_size(group)?;
        let temp_cb = channel.to_decode_only_channel_block();
        let linear_coeffs = Self::get_linear_coeffs(conversion);
        let mut values = Vec::with_capacity((count as usize).min(1 << 24));
        Self::for_each_window_section(reader, group, record_size, start, count, |data| {
            Self::decode_records_to_f64(data, record_size, group, channel, conversion, &temp_cb, linear_coeffs, conversion.is_some(), &mut values)
        })?;
        Ok(values)
    }

    /// Call `f` with the bytes of the records `[start, start + count)` of
    /// `group`, one slice per data block the window touches.
    fn for_each_window_section<R, F>(
        reader: &mut R,
        group: &IndexedChannelGroup,
        record_size: usize,
        start: u64,
        count: u64,
        mut f: F,
    ) -> Result<(), MdfError>
    where
        R: ByteRangeReader<Error = MdfError>,
        F: FnMut(&[u8]) -> Result<(), MdfError>,
    {
        let end = start + count;
        let mut block_start = 0u64;
        // A probing reader (the wasm fragment driver) records every missing
        // block of the window in one pass instead of stopping at the first.
        let mut first_err = None;
        for block in &group.data_blocks {
            if block_start >= end {
                break;
            }
            if block.is_compressed {
                return Err(MdfError::BlockSerializationError(
                    "Compressed blocks not yet supported in index reader".to_string(),
                ));
            }
            let block_end = block_start + Self::records_in_block(block, record_size)?;
            let (from, to) = (start.max(block_start), end.min(block_end));
            if from < to {
                let rel = (from - block_start) * record_size as u64;
                let len = (to - from) * record_size as u64;
                // Verify the block header like the full read path does (the
                // header wins over the index): a window starting at the
                // block's first record reads header and data in one request.
                let read = if rel == 0 {
                    reader.read_range(block.file_offset, 24 + len).map(|mut bytes| {
                        let data = bytes.split_off(24.min(bytes.len()));
                        (bytes, data)
                    })
                } else {
                    let header = reader.read_range(block.file_offset, 24);
                    let data = reader.read_range(block.file_offset + 24 + rel, len);
                    header.and_then(|h| data.map(|d| (h, d)))
                };
                match read {
                    Ok((header, data)) if first_err.is_none() => {
                        let real_len = Self::verified_data_len(Some(&header), block)?;
                        if rel + len > real_len {
                            return Err(MdfError::BlockSerializationError(format!(
                                "data block at offset {} holds {} data bytes but the index \
                                 expects at least {} (inconsistent file; rebuild the index)",
                                block.file_offset, real_len, rel + len
                            )));
                        }
                        f(&data)?
                    }
                    Ok(_) => {}
                    Err(e) if reader.is_probing() => {
                        first_err.get_or_insert(e);
                    }
                    Err(e) => return Err(e),
                }
            }
            block_start = block_end;
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        if block_start < end {
            return Err(MdfError::BlockSerializationError(format!(
                "record window ending at {} exceeds the {} records held by the data blocks",
                end, block_start
            )));
        }
        Ok(())
    }

    /// Records held by a group's data blocks.
    fn stored_records(group: &IndexedChannelGroup) -> Result<u64, MdfError> {
        let record_size = Self::checked_record_size(group)?;
        let mut total = 0u64;
        for block in &group.data_blocks {
            total = total.saturating_add(Self::records_in_block(block, record_size)?);
        }
        Ok(total)
    }

    fn group_and_channel(&self, g: usize, c: usize) -> Result<(&IndexedChannelGroup, &IndexedChannel), MdfError> {
        let group = self.channel_groups.get(g)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        let channel = group.channels.get(c)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;
        Ok((group, channel))
    }

    fn check_window(group: &IndexedChannelGroup, start: u64, count: u64) -> Result<(), MdfError> {
        match start.checked_add(count) {
            Some(end) if end <= group.record_count => Ok(()),
            _ => Err(MdfError::BlockSerializationError(format!(
                "record window starting at {} for {} records exceeds the group's {} records",
                start, count, group.record_count
            ))),
        }
    }

    /// `(group, channel)` of the time axis for `(g, c)`: its master, or the
    /// channel itself when it *is* the master.
    fn time_axis_location(&self, g: usize, c: usize) -> Option<(usize, usize)> {
        match self.master_location(g, c) {
            Some(m) => Some(m),
            None => self.channel_groups.get(g)?.channels.get(c)?.is_master().then_some((g, c)),
        }
    }

    /// Record window `(start, count)` of the group owning `(g, c)` whose
    /// master (time) values lie in `[t_start, t_end]` (either bound optional).
    ///
    /// The master must be non-decreasing, as MDF requires for a time master.
    /// It is searched rather than read (see [`search_time_window`], shared
    /// with the in-memory reader so both select the same records): each step reads 16
    /// single records, so a window is found in a handful of small reads
    /// (`log16(records)` rounds per bound). With a probing reader (the wasm
    /// fragment driver) both bounds' probes of a round are attempted before the
    /// first miss is returned, so every gap of the round is recorded at once.
    pub(crate) fn record_window_for_time<R: ByteRangeReader<Error = MdfError>>(
        &self,
        g: usize,
        c: usize,
        t_start: Option<f64>,
        t_end: Option<f64>,
        reader: &mut R,
    ) -> Result<(u64, u64), MdfError> {
        let (group, channel) = self.group_and_channel(g, c)?;
        let Some((mg, m)) = self.time_axis_location(g, c) else {
            // Same rule as the in-memory reader (`Mdf::record_window`): an
            // empty group selects nothing, otherwise a master is required.
            if group.record_count == 0 {
                return Ok((0, 0));
            }
            return Err(MdfError::InvalidArgument(format!(
                "channel '{}' has no master (time) channel; cannot select by time",
                channel.name.as_deref().unwrap_or_default()
            )));
        };
        let (mgroup, mchannel) = self.group_and_channel(mg, m)?;
        let n = group.record_count.min(mgroup.record_count);
        let conversion = Self::resolve_conversion(mchannel, reader)?;
        search_time_window(n, t_start, t_end, |i| {
            Ok(self.window_values_f64(mgroup, mchannel, conversion.as_deref(), i, 1, reader)?[0])
        })
    }

    /// Resolve a channel's conversion for reading.
    ///
    /// Prefers a conversion already resolved into the index (local-file /
    /// in-memory builds, and older serialized indexes). Otherwise — the remote
    /// / range-reader build path, which records only
    /// [`IndexedChannel::conversion_addr`] — it fetches the `##CC` block and
    /// resolves its dependency chain through `reader`, exactly when the caller
    /// asks to read values. Returns `None` when the channel has no conversion.
    fn resolve_conversion<'c, R: ByteRangeReader<Error = MdfError>>(
        channel: &'c IndexedChannel,
        reader: &mut R,
    ) -> Result<Option<Cow<'c, ConversionBlock>>, MdfError> {
        if let Some(conv) = &channel.conversion {
            return Ok(Some(Cow::Borrowed(conv)));
        }
        if channel.conversion_addr == 0 {
            return Ok(None);
        }
        let addr = channel.conversion_addr;
        let header_bytes = reader.read_range(addr, 24)?;
        let header = BlockHeader::from_bytes(&header_bytes)?;
        if header.id != "##CC" {
            return Err(MdfError::BlockIDError {
                actual: header.id,
                expected: "##CC".to_string(),
            });
        }
        let full = reader.read_range(addr, header.block_len)?;
        let mut cc = ConversionBlock::from_bytes(&full)?;
        cc.resolve_all_dependencies_via_reader(reader, addr)?;
        Ok(Some(Cow::Owned(cc)))
    }

    /// The conversion a read applies: none for a `raw` read (the stored
    /// values are wanted, so a deferred conversion is not even fetched),
    /// else [`resolve_conversion`](Self::resolve_conversion).
    fn read_conversion<'c, R: ByteRangeReader<Error = MdfError>>(
        channel: &'c IndexedChannel,
        reader: &mut R,
        raw: bool,
    ) -> Result<Option<Cow<'c, ConversionBlock>>, MdfError> {
        if raw {
            return Ok(None);
        }
        Self::resolve_conversion(channel, reader)
    }

    /// Resolve the conversion of the channel at `(g, c)` using `reader`,
    /// following the same lazy rules as the read paths. Crate-visible so the
    /// wasm bindings can drive fragment-based conversion resolution: pointing a
    /// gap-reporting reader at this lets the JS side discover exactly which
    /// conversion-block byte ranges it must fetch before a fragment read.
    #[allow(dead_code)] // used by the wasm bindings (wasm feature)
    pub(crate) fn resolve_channel_conversion<R: ByteRangeReader<Error = MdfError>>(
        &self,
        g: usize,
        c: usize,
        reader: &mut R,
    ) -> Result<Option<ConversionBlock>, MdfError> {
        let channel = self
            .channel_groups
            .get(g)
            .and_then(|grp| grp.channels.get(c))
            .ok_or_else(|| {
                MdfError::BlockSerializationError("Invalid channel index".to_string())
            })?;
        Ok(Self::resolve_conversion(channel, reader)?.map(Cow::into_owned))
    }

    /// Extract linear conversion coefficients (a, b) for inline application.
    fn get_linear_coeffs(conversion: Option<&ConversionBlock>) -> Option<(f64, f64)> {
        conversion.and_then(|conv| {
            if conv.cc_type == ConversionType::Linear && conv.cc_val.len() >= 2 {
                Some((conv.cc_val[0], conv.cc_val[1]))
            } else {
                None
            }
        })
    }

    /// Read values for a regular (non-VLSD) channel using byte range reader
    fn read_regular_channel_values<R: ByteRangeReader<Error = MdfError>>(
        &self,
        group: &IndexedChannelGroup,
        channel: &IndexedChannel,
        conversion: Option<&ConversionBlock>,
        reader: &mut R,
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        let record_size = Self::checked_record_size(group)?;
        let mut total_records: usize = 0;
        for data_block in &group.data_blocks {
            if data_block.is_compressed {
                return Err(MdfError::BlockSerializationError(
                    "Compressed blocks not yet supported in index reader".to_string()
                ));
            }
            total_records += Self::records_in_block(data_block, record_size)? as usize;
        }
        // Cap the pre-allocation: `total_records` derives from index-supplied
        // block sizes, and a forged value must not abort on allocation. The
        // vector still grows as needed; real reads fail earlier at the reader.
        let mut values = Vec::with_capacity(total_records.min(1 << 24));
        let temp_cb = channel.to_decode_only_channel_block();

        Self::for_each_data_section(reader, &group.data_blocks, |block_data| {
            Self::decode_records_to_values(block_data, record_size, group, conversion, &temp_cb, &mut values)
        })?;

        Ok(values)
    }

    /// Read values for a VLSD channel (channel type 1) via a byte-range reader.
    ///
    /// The channel's fixed record field holds an 8-byte little-endian virtual
    /// offset into the concatenation of the `##SD` fragments' data sections;
    /// at that offset lives one `[u32 length][payload]` entry. This resolves
    /// each record's offset into the captured fragment chain and decodes the
    /// payload, preserving one value per record so [`Signal`] timestamps stay
    /// aligned and per-record invalidation can be honoured.
    fn read_vlsd_channel_values<R: ByteRangeReader<Error = MdfError>>(
        &self,
        group: &IndexedChannelGroup,
        channel: &IndexedChannel,
        conversion: Option<&ConversionBlock>,
        reader: &mut R,
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        Self::check_vlsd_readable(group, channel)?;

        // Fetch each SD fragment's data section, tracking each fragment's
        // virtual start offset in the concatenated stream.
        let mut fragments: Vec<Vec<u8>> = Vec::with_capacity(channel.vlsd_data_blocks.len());
        let mut frag_starts: Vec<u64> = Vec::with_capacity(channel.vlsd_data_blocks.len());
        let mut running: u64 = 0;
        for db in &channel.vlsd_data_blocks {
            let data_len = Self::data_section_size(db)?;
            let bytes = reader.read_range(db.file_offset + 24, data_len)?;
            frag_starts.push(running);
            running = running.checked_add(bytes.len() as u64).ok_or_else(|| {
                MdfError::BlockSerializationError(
                    "VLSD ##SD chain total size overflows the address space".to_string(),
                )
            })?;
            fragments.push(bytes);
        }
        let stream_len = running;

        let record_size = Self::checked_record_size(group)?;
        let record_id_len = group.record_id_len as usize;
        let cg_data_bytes = group.record_size;
        let has_invalidation = group.invalidation_bytes > 0;
        let all_invalid = channel.flags & 0x1 != 0;
        let offset_pos = record_id_len + channel.byte_offset as usize;

        // A fixed-record channel block (data == 0) drives the validity check;
        // a separate VLSD block (data != 0) drives payload decoding.
        let validity_cb = channel.to_decode_only_channel_block();
        let mut payload_cb = channel.to_decode_only_channel_block();
        payload_cb.data = 1;

        let mut total_records: usize = 0;
        for data_block in &group.data_blocks {
            total_records += Self::records_in_block(data_block, record_size)? as usize;
        }
        let mut values = Vec::with_capacity(total_records.min(1 << 24));

        Self::for_each_data_section(reader, &group.data_blocks, |block_data| {
            if block_data.len() % record_size != 0 {
                return Err(MdfError::BlockSerializationError(format!(
                    "data block length {} is not a multiple of the {}-byte record — \
                     record spans data block fragments, which is unsupported",
                    block_data.len(),
                    record_size
                )));
            }
            let record_count = block_data.len() / record_size;
            for i in 0..record_count {
                let record = &block_data[i * record_size..(i + 1) * record_size];

                if all_invalid
                    || (has_invalidation
                        && !check_value_validity(record, record_id_len, cg_data_bytes, &validity_cb))
                {
                    values.push(None);
                    continue;
                }

                // Read the inline 8-byte virtual offset.
                if offset_pos + 8 > record.len() {
                    return Err(MdfError::TooShortBuffer {
                        actual: record.len(),
                        expected: offset_pos + 8,
                        file: file!(),
                        line: line!(),
                    });
                }
                let virtual_offset =
                    u64::from_le_bytes(record[offset_pos..offset_pos + 8].try_into().unwrap());

                if virtual_offset >= stream_len {
                    return Err(MdfError::BlockSerializationError(format!(
                        "VLSD inline offset {} is past the end of the {}-byte signal-data stream",
                        virtual_offset, stream_len
                    )));
                }

                // Locate the fragment whose virtual span contains the offset.
                let frag_idx = frag_starts.partition_point(|&s| s <= virtual_offset) - 1;
                let frag = &fragments[frag_idx];
                let pos = (virtual_offset - frag_starts[frag_idx]) as usize;

                if pos + 4 > frag.len() {
                    return Err(MdfError::BlockSerializationError(format!(
                        "VLSD entry at virtual offset {} is truncated: no room for its \
                         length prefix in fragment {}",
                        virtual_offset, frag_idx
                    )));
                }
                let len = u32::from_le_bytes(frag[pos..pos + 4].try_into().unwrap()) as usize;
                let start = pos + 4;
                let end = match start.checked_add(len) {
                    Some(e) if e <= frag.len() => e,
                    _ => {
                        return Err(MdfError::BlockSerializationError(format!(
                            "VLSD entry at virtual offset {} declares length {} which \
                             exceeds fragment {} (size {})",
                            virtual_offset, len, frag_idx, frag.len()
                        )));
                    }
                };
                let payload = &frag[start..end];

                if let Some(value) = decode_channel_value(payload, 0, &payload_cb) {
                    let final_value = if let Some(conversion) = conversion {
                        conversion.apply_decoded(value, &[])?
                    } else {
                        value
                    };
                    values.push(Some(final_value));
                } else {
                    values.push(None);
                }
            }
            Ok(())
        })?;

        Ok(values)
    }

    /// Refuse VLSD channels the index reader cannot decode (compressed
    /// fragments, missing signal data, a malformed offset field).
    fn check_vlsd_readable(group: &IndexedChannelGroup, channel: &IndexedChannel) -> Result<(), MdfError> {
        // Compressed fragments (either the fixed records or the SD stream) are
        // not decodable yet.
        if group.data_blocks.iter().any(|b| b.is_compressed)
            || channel.vlsd_data_blocks.iter().any(|b| b.is_compressed)
        {
            return Err(MdfError::BlockSerializationError(
                "Compressed blocks not yet supported in index reader".to_string(),
            ));
        }

        // A VLSD channel without a data link has no signal data at all —
        // rebuilding the index cannot help, so distinguish it from an index
        // built before VLSD chains were captured.
        if channel.vlsd_data_blocks.is_empty() {
            if channel.vlsd_data_address.is_none() {
                return Err(MdfError::BlockSerializationError(format!(
                    "VLSD channel '{}' has no signal data (its ##SD data link is 0)",
                    channel.name.as_deref().unwrap_or("<unnamed>")
                )));
            }
            return Err(MdfError::BlockSerializationError(format!(
                "VLSD channel '{}' has no signal-data fragments recorded in the index; \
                 rebuild the index with this version to read it",
                channel.name.as_deref().unwrap_or("<unnamed>")
            )));
        }

        // The VLSD offset field is fixed at 64 bits by the spec and is
        // byte-aligned; a nonzero bit offset would silently shift every
        // resolved stream offset.
        if channel.bit_count != 64 || channel.bit_offset != 0 {
            return Err(MdfError::BlockSerializationError(format!(
                "VLSD channel '{}' has a {}-bit record field at bit offset {}; the \
                 inline VLSD offset must be a byte-aligned 64-bit field",
                channel.name.as_deref().unwrap_or("<unnamed>"),
                channel.bit_count,
                channel.bit_offset
            )));
        }
        Ok(())
    }

    /// Window read of a VLSD channel: decode only records
    /// `[start, start + count)`, fetching their inline offsets and then just
    /// the slice of the signal-data stream those offsets point into (one
    /// read, plus one more if the last entry runs past it) — not the whole
    /// stream.
    fn read_vlsd_window_values<R: ByteRangeReader<Error = MdfError>>(
        &self,
        group: &IndexedChannelGroup,
        channel: &IndexedChannel,
        conversion: Option<&ConversionBlock>,
        start: u64,
        count: u64,
        reader: &mut R,
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        Self::check_vlsd_readable(group, channel)?;
        let record_size = Self::checked_record_size(group)?;
        let record_id_len = group.record_id_len as usize;
        let has_invalidation = group.invalidation_bytes > 0;
        let all_invalid = channel.flags & 0x1 != 0;
        let offset_pos = record_id_len + channel.byte_offset as usize;
        let validity_cb = channel.to_decode_only_channel_block();
        let mut payload_cb = channel.to_decode_only_channel_block();
        payload_cb.data = 1;

        // Inline offsets of the window's records (None = invalid sample).
        let mut offsets: Vec<Option<u64>> = Vec::with_capacity((count as usize).min(1 << 24));
        Self::for_each_window_section(reader, group, record_size, start, count, |data| {
            for record in data.chunks_exact(record_size) {
                if all_invalid
                    || (has_invalidation
                        && !check_value_validity(record, record_id_len, group.record_size, &validity_cb))
                {
                    offsets.push(None);
                    continue;
                }
                let field = record.get(offset_pos..offset_pos + 8).ok_or(MdfError::TooShortBuffer {
                    actual: record.len(),
                    expected: offset_pos + 8,
                    file: file!(),
                    line: line!(),
                })?;
                offsets.push(Some(u64::from_le_bytes(field.try_into().unwrap())));
            }
            Ok(())
        })?;

        // Virtual start of each ##SD fragment in the concatenated stream.
        let mut frag_starts = Vec::with_capacity(channel.vlsd_data_blocks.len());
        let mut stream_len = 0u64;
        for db in &channel.vlsd_data_blocks {
            frag_starts.push(stream_len);
            stream_len = stream_len.checked_add(Self::data_section_size(db)?).ok_or_else(|| {
                MdfError::BlockSerializationError("VLSD ##SD chain total size overflows the address space".to_string())
            })?;
        }
        let present = || offsets.iter().flatten().copied();
        let (Some(lo), Some(hi)) = (present().min(), present().max()) else {
            return Ok(vec![None; offsets.len()]);
        };
        let prefix_end = hi.checked_add(4).filter(|&e| e <= stream_len).ok_or_else(|| {
            MdfError::BlockSerializationError(format!(
                "VLSD inline offset {hi} leaves no room for a length prefix in the {stream_len}-byte signal-data stream"
            ))
        })?;
        let mut buf = Self::read_vlsd_stream(reader, channel, &frag_starts, lo, prefix_end - lo)?;
        let entry_len = |buf: &[u8], off: u64| -> usize {
            let p = (off - lo) as usize;
            u32::from_le_bytes(buf[p..p + 4].try_into().unwrap()) as usize
        };
        let end = present().map(|off| off + 4 + entry_len(&buf, off) as u64).max().unwrap_or(prefix_end);
        if end > stream_len {
            return Err(MdfError::BlockSerializationError(format!(
                "VLSD entry ending at virtual offset {end} is past the end of the {stream_len}-byte signal-data stream"
            )));
        }
        if end > prefix_end {
            let more = Self::read_vlsd_stream(reader, channel, &frag_starts, prefix_end, end - prefix_end)?;
            buf.extend_from_slice(&more);
        }

        let mut values = Vec::with_capacity(offsets.len());
        for off in &offsets {
            let Some(off) = *off else {
                values.push(None);
                continue;
            };
            // Like the full read, refuse an entry that does not fit in the
            // ##SD fragment it starts in.
            let frag_idx = frag_starts.partition_point(|&s| s <= off) - 1;
            let frag_end = frag_starts.get(frag_idx + 1).copied().unwrap_or(stream_len);
            let len = entry_len(&buf, off);
            if off + 4 + len as u64 > frag_end {
                return Err(MdfError::BlockSerializationError(format!(
                    "VLSD entry at virtual offset {} declares length {} which exceeds fragment {} (size {})",
                    off,
                    len,
                    frag_idx,
                    frag_end - frag_starts[frag_idx]
                )));
            }
            let p = (off - lo) as usize + 4;
            let payload = &buf[p..p + entry_len(&buf, off)];
            values.push(match decode_channel_value(payload, 0, &payload_cb) {
                Some(value) => Some(match conversion {
                    Some(conversion) => conversion.apply_decoded(value, &[])?,
                    None => value,
                }),
                None => None,
            });
        }
        Ok(values)
    }

    /// Bytes `[from, from + len)` of a VLSD channel's concatenated
    /// signal-data stream, read from the ##SD fragments they fall in.
    fn read_vlsd_stream<R: ByteRangeReader<Error = MdfError>>(
        reader: &mut R,
        channel: &IndexedChannel,
        frag_starts: &[u64],
        from: u64,
        len: u64,
    ) -> Result<Vec<u8>, MdfError> {
        let to = from + len;
        let mut out = Vec::with_capacity(len as usize);
        // Attempt every fragment's read before failing, so a probing reader
        // records all gaps in one pass.
        let mut first_err = None;
        for (db, &fs) in channel.vlsd_data_blocks.iter().zip(frag_starts) {
            let fe = fs + Self::data_section_size(db)?;
            let (a, b) = (from.max(fs), to.min(fe));
            if a >= b {
                continue;
            }
            match reader.read_range(db.file_offset + 24 + (a - fs), b - a) {
                // A custom reader that returns fewer bytes (e.g. clamped at
                // EOF) must surface as an error, not an out-of-bounds slice.
                Ok(bytes) if bytes.len() as u64 != b - a => {
                    return Err(MdfError::TooShortBuffer {
                        actual: bytes.len(),
                        expected: (b - a) as usize,
                        file: file!(),
                        line: line!(),
                    });
                }
                Ok(bytes) => out.extend_from_slice(&bytes),
                Err(e) if reader.is_probing() => {
                    first_err.get_or_insert(e);
                }
                Err(e) => return Err(e),
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        Ok(out)
    }

    /// Map VLSD decoded values to `f64`, matching `Channel::values`:
    /// numeric variants convert, everything else (strings, bytes, invalid) is
    /// NaN.
    fn vlsd_values_to_f64(values: Vec<Option<DecodedValue>>) -> Vec<f64> {
        values
            .into_iter()
            .map(|v| match v {
                Some(DecodedValue::Float(f)) => f,
                Some(DecodedValue::UnsignedInteger(u)) => u as f64,
                Some(DecodedValue::SignedInteger(i)) => i as f64,
                _ => f64::NAN,
            })
            .collect()
    }

    /// Decode records from a data block slice into values vec.
    /// Shared by both the reader-based and slice-based paths.
    fn decode_records_to_values(
        block_data: &[u8],
        record_size: usize,
        group: &IndexedChannelGroup,
        conversion: Option<&ConversionBlock>,
        temp_cb: &crate::blocks::channel_block::ChannelBlock,
        values: &mut Vec<Option<DecodedValue>>,
    ) -> Result<(), MdfError> {
        if record_size == 0 {
            return Err(MdfError::BlockSerializationError(
                "invalid index: record size is 0".to_string(),
            ));
        }
        if !block_data.len().is_multiple_of(record_size) {
            return Err(MdfError::BlockSerializationError(format!(
                "data block length {} is not a multiple of the {}-byte record — \
                 record spans data block fragments, which is unsupported",
                block_data.len(),
                record_size
            )));
        }
        let record_count = block_data.len() / record_size;
        let record_id_len = group.record_id_len as usize;
        let cg_data_bytes = group.record_size;

        for i in 0..record_count {
            let record = &block_data[i * record_size..(i + 1) * record_size];
            if let Some(decoded) = decode_channel_value_with_validity(
                record, record_id_len, cg_data_bytes, temp_cb,
            ) {
                if decoded.is_valid {
                    let final_value = if let Some(conversion) = conversion {
                        conversion.apply_decoded(decoded.value, &[])?
                    } else {
                        decoded.value
                    };
                    values.push(Some(final_value));
                } else {
                    values.push(None);
                }
            } else {
                values.push(None);
            }
        }
        Ok(())
    }

    /// Decode records from a data block as f64 values.
    /// Uses the fast decode_f64_from_record path and applies conversions inline.
    /// For channels without invalidation bytes, skips validity checking entirely.
    fn decode_records_to_f64(
        block_data: &[u8],
        record_size: usize,
        group: &IndexedChannelGroup,
        channel: &IndexedChannel,
        conversion: Option<&ConversionBlock>,
        temp_cb: &crate::blocks::channel_block::ChannelBlock,
        linear_coeffs: Option<(f64, f64)>,
        has_conversion: bool,
        values: &mut Vec<f64>,
    ) -> Result<(), MdfError> {
        if record_size == 0 {
            return Err(MdfError::BlockSerializationError(
                "invalid index: record size is 0".to_string(),
            ));
        }
        if !block_data.len().is_multiple_of(record_size) {
            return Err(MdfError::BlockSerializationError(format!(
                "data block length {} is not a multiple of the {}-byte record — \
                 record spans data block fragments, which is unsupported",
                block_data.len(),
                record_size
            )));
        }
        let record_count = block_data.len() / record_size;
        let record_id_len = group.record_id_len as usize;
        let cg_data_bytes = group.record_size;
        let has_invalidation = group.invalidation_bytes > 0;

        // MDF 4.1: cn_flags bit 0 ("all values invalid") applies regardless
        // of whether the group carries invalidation bytes. Short-circuit to
        // NaN for every sample, matching decode_records_to_values.
        if channel.flags & 0x1 != 0 {
            values.extend(std::iter::repeat_n(f64::NAN, record_count));
            return Ok(());
        }

        // Each record is decoded once: straight to f64 (with a linear
        // conversion applied inline), or through `DecodedValue` when a
        // non-linear conversion needs it.
        let valid = |record: &[u8]| {
            !has_invalidation || check_value_validity(record, record_id_len, cg_data_bytes, temp_cb)
        };
        let records = block_data.chunks_exact(record_size);
        match (linear_coeffs, conversion.filter(|_| has_conversion)) {
            (Some((a, b)), _) => {
                for record in records {
                    values.push(match valid(record) {
                        true => a + b * decode_f64_from_record(record, record_id_len, temp_cb),
                        false => f64::NAN,
                    });
                }
            }
            (None, Some(conversion)) => {
                for record in records {
                    let value = match valid(record) {
                        true => decode_channel_value(record, record_id_len, temp_cb),
                        false => None,
                    };
                    values.push(match value {
                        Some(value) => match conversion.apply_decoded(value, &[])? {
                            DecodedValue::Float(v) => v,
                            DecodedValue::UnsignedInteger(v) => v as f64,
                            DecodedValue::SignedInteger(v) => v as f64,
                            _ => f64::NAN,
                        },
                        None => f64::NAN,
                    });
                }
            }
            (None, None) => {
                for record in records {
                    values.push(match valid(record) {
                        true => decode_f64_from_record(record, record_id_len, temp_cb),
                        false => f64::NAN,
                    });
                }
            }
        }
        Ok(())
    }

    // ----- metadata (cross-language contract) -----------------------------

    /// Metadata of every channel group, in file order.
    ///
    /// The raw indexed structures (record layout, data-block locations) are
    /// available through the public [`channel_groups`](Self::channel_groups)
    /// field.
    pub fn groups(&self) -> Result<Vec<GroupInfo>, MdfError> {
        Ok((0..self.channel_groups.len()).map(|g| self.group_info(g)).collect())
    }

    /// Metadata of the first channel group named `name`, or `None`.
    ///
    /// The groups of a column-oriented (MDF 4.2) file are merged back into
    /// their source group, as [`Mdf::group`] does.
    pub fn group(&self, name: &str) -> Result<Option<GroupInfo>, MdfError> {
        let names = self.group_names();
        let Some(first) = names.iter().position(|n| n.as_deref() == Some(name)) else {
            return Ok(None);
        };
        let channel_counts: Vec<usize> = self.channel_groups.iter().map(|g| g.channels.len()).collect();
        let members = logical_group_members(&names, &self.time_groups(), &channel_counts, first);
        Ok(Some(merge_group_infos(
            self.group_info(first),
            members.iter().map(|&m| self.group_info(m)),
        )))
    }

    fn group_names(&self) -> Vec<Option<String>> {
        self.channel_groups.iter().map(|g| g.name.clone()).collect()
    }

    /// For each group, the group holding its time axis: itself for a local
    /// master, the remote master group of an MDF 4.2 column-oriented group,
    /// `None` when untimed.
    fn time_groups(&self) -> Vec<Option<usize>> {
        (0..self.channel_groups.len()).map(|g| self.time_group(g)).collect()
    }

    fn time_group(&self, g: usize) -> Option<usize> {
        let group = self.channel_groups.get(g)?;
        if group.master_channel().is_some() {
            return Some(g);
        }
        group
            .master_group
            .filter(|&m| m != g && self.channel_groups.get(m).is_some_and(|mg| mg.master_channel().is_some()))
    }

    /// Metadata of the first channel named `name`, or `None`. Pass `group` to
    /// look only in the group(s) with that name.
    pub fn channel(&self, name: &str, group: Option<&str>) -> Result<Option<ChannelInfo>, MdfError> {
        Ok(self.indexed_channel(name, group).map(IndexedChannel::info))
    }

    /// Names of every named channel across all groups (duplicates kept).
    pub fn channel_names(&self) -> Result<Vec<String>, MdfError> {
        Ok(self
            .channel_groups
            .iter()
            .flat_map(|g| g.channels.iter())
            .filter_map(|c| c.name.clone())
            .collect())
    }

    /// Measurement start time in nanoseconds since the Unix epoch, or `None`
    /// if the file header does not record one.
    pub fn start_time_ns(&self) -> Option<u64> {
        self.start_time_ns
    }

    /// Size in bytes of the indexed MDF file.
    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    /// The raw [`IndexedChannel`] of the first channel named `name` (in the
    /// group(s) named `group`, when given). Rust-only access to the record
    /// layout behind [`channel`](Self::channel).
    pub fn indexed_channel(&self, name: &str, group: Option<&str>) -> Option<&IndexedChannel> {
        let (g, c) = self.locate_opt(name, group)?;
        self.channel_groups.get(g)?.channels.get(c)
    }

    fn group_info(&self, g: usize) -> GroupInfo {
        let group = &self.channel_groups[g];
        GroupInfo {
            name: group.name.clone(),
            comment: group.comment.clone(),
            record_count: group.record_count,
            channel_names: group.channels.iter().filter_map(|c| c.name.clone()).collect(),
            // A column-oriented (MDF 4.2) signal group reports the shared
            // time channel it references.
            master_channel: group
                .master_channel()
                .or_else(|| {
                    group
                        .master_group
                        .and_then(|mg| self.channel_groups.get(mg))
                        .and_then(|mg| mg.master_channel())
                })
                .and_then(|c| c.name.clone()),
            channels: group.channels.iter().map(IndexedChannel::info).collect(),
        }
    }

    /// Position of a channel by name, optionally restricted to a group name.
    pub(crate) fn locate_opt(&self, name: &str, group: Option<&str>) -> Option<(usize, usize)> {
        match group {
            Some(group) => self.locate_in(group, name),
            None => self.locate(name),
        }
    }

    /// [`locate_opt`](Self::locate_opt), erroring with `ChannelNotFound`.
    pub(crate) fn locate_checked(&self, name: &str, group: Option<&str>) -> Result<(usize, usize), MdfError> {
        self.locate_opt(name, group)
            .ok_or_else(|| MdfError::channel_not_found(name, group))
    }

    /// Resolve a channel name to its `(group_index, channel_index)` position.
    ///
    /// Returns the first match across all groups. Internal helper backing the
    /// name-based public methods.
    pub(crate) fn locate(&self, name: &str) -> Option<(usize, usize)> {
        for (g, group) in self.channel_groups.iter().enumerate() {
            for (c, channel) in group.channels.iter().enumerate() {
                if channel.name.as_deref() == Some(name) {
                    return Some((g, c));
                }
            }
        }
        None
    }

    /// Resolve a `(group name, channel name)` pair to indices.
    ///
    /// Every group carrying that name is searched (a column-oriented file has
    /// one channel group per signal, all named after the source group), then
    /// the time groups they reference under another name — a time axis shared
    /// by several source groups carries only one of their names.
    pub(crate) fn locate_in(&self, group: &str, name: &str) -> Option<(usize, usize)> {
        let named: Vec<usize> = (0..self.channel_groups.len())
            .filter(|&g| self.channel_groups[g].name.as_deref() == Some(group))
            .collect();
        let remote = named
            .iter()
            .filter_map(|&g| self.time_group(g))
            .filter(|&t| self.channel_groups[t].name.as_deref() != Some(group));
        named.iter().copied().chain(remote).find_map(|g| {
            self.channel_groups[g]
                .channels
                .iter()
                .position(|ch| ch.name.as_deref() == Some(name))
                .map(|c| (g, c))
        })
    }

    /// `(group, channel)` of the master (time) channel for the channel at
    /// `(g, c)`: the group's own master, or the remote master of an MDF 4.2
    /// column-oriented group. `None` when there is none or `(g, c)` is itself
    /// the master.
    pub(crate) fn master_location(&self, g: usize, c: usize) -> Option<(usize, usize)> {
        let grp = self.channel_groups.get(g)?;
        if let Some(m) = grp.channels.iter().position(|ch| ch.is_master()) {
            return (m != c).then_some((g, m));
        }
        let mg = grp.master_group?;
        let m = self.channel_groups.get(mg)?.channels.iter().position(|ch| ch.is_master())?;
        Some((mg, m))
    }

    /// Bind this index to a byte-range source for reading sample data.
    ///
    /// The returned [`MdfReader`] borrows the index and owns `reader`; read
    /// values by channel name without re-supplying the source each time.
    pub fn open<R: ByteRangeReader<Error = MdfError>>(&self, reader: R) -> MdfReader<'_, R> {
        MdfReader { index: self, reader }
    }

    /// Bind this index to a local file (via memory map) for reading.
    ///
    /// Convenience wrapper around [`MdfIndex::open`] using [`MmapRangeReader`].
    /// Not available on `wasm32-unknown-unknown`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_file(&self, path: &str) -> Result<MdfReader<'_, MmapRangeReader>, MdfError> {
        Ok(self.open(MmapRangeReader { mmap: self.map_source_file(path)? }))
    }

    /// The data source `read` / `values` fetch sample bytes from, if any.
    pub fn source(&self) -> Option<&Source> {
        self.source.as_ref()
    }

    /// Attach (or replace) the data source used by `read` / `values`.
    ///
    /// Accepts a [`Source`] or a string: `http(s)://` URLs are read with range
    /// requests (with the `http` feature), anything else is a file path.
    pub fn set_source(&mut self, source: impl Into<Source>) {
        self.source = Some(source.into());
    }

    /// Detach the data source.
    pub fn clear_source(&mut self) {
        self.source = None;
    }

    // ----- data (cross-language contract) ---------------------------------

    /// Read a channel as a [`Signal`]: its values (conversions applied,
    /// `None` = invalid sample) paired with the group's time axis. For the
    /// master channel itself `timestamps` is empty.
    ///
    /// This is the lazy read path: bytes are fetched from the attached
    /// [`Source`] now, not at index build time. Errors with
    /// [`MdfError::ChannelNotFound`] if no such channel exists, or if no
    /// source is attached (see [`set_source`](Self::set_source)).
    ///
    /// `options` is `None` for the whole channel, or a [`ReadOptions`] with a
    /// group filter and/or a time or record window. A window fetches only the
    /// selected records: a time window is located by searching the master
    /// channel (a few single-record reads), not by downloading it.
    pub fn read<'o>(&self, name: &str, options: impl Into<Option<&'o ReadOptions>>) -> Result<Signal, MdfError> {
        let default = ReadOptions::default();
        let options = options.into().unwrap_or(&default);
        let window = options.window()?;
        let (g, c) = self.locate_checked(name, options.group_name())?;
        // Over a URL the channel and its time axis are fetched concurrently.
        #[cfg(feature = "http")]
        if let Source::Url(url) = self.require_source()? {
            let mut signals = self.read_many_url(url, &[(g, c)], window, options.raw)?;
            return signals.pop().ok_or_else(|| MdfError::BlockSerializationError("no signal read".to_string()));
        }
        if window == Window::All {
            return self.read_signal(g, c, options.raw);
        }
        let mut reader = self.source_reader(g, c, options.raw, true)?;
        let (start, count) = self.resolve_window_via(g, c, window, &mut reader)?;
        read_signal_window(self, g, c, start, count, options.raw, &mut reader)
    }

    /// Read several channels at once, with the same `options` for all of
    /// them: one [`Signal`] per name, in the order given.
    ///
    /// Equivalent to calling [`read`](Self::read) for each name, but each
    /// time axis shared by several of the channels is fetched and decoded
    /// once (in a column-oriented file every signal of a source group shares
    /// one), a time window is searched once per time axis, and over a URL
    /// every byte range of the whole batch is fetched concurrently. Errors
    /// with [`MdfError::ChannelNotFound`] — before fetching anything — if any
    /// name is unknown.
    pub fn read_many<'o, S: AsRef<str>>(
        &self,
        names: &[S],
        options: impl Into<Option<&'o ReadOptions>>,
    ) -> Result<Vec<Signal>, MdfError> {
        let default = ReadOptions::default();
        let options = options.into().unwrap_or(&default);
        let window = options.window()?;
        let targets = self.locate_all(names, options.group_name())?;
        if targets.is_empty() {
            return Ok(Vec::new());
        }
        #[cfg(all(target_arch = "wasm32", not(feature = "http")))]
        let _ = window;
        match self.require_source()? {
            #[cfg(not(target_arch = "wasm32"))]
            Source::File(path) => {
                let mmap = self.map_source_file(path)?;
                if window == Window::All {
                    return self.read_many_from_slice(&targets, options.raw, &mmap);
                }
                self.read_many_at(&targets, window, options.raw, &mut MmapRangeReader { mmap })
            }
            #[cfg(target_arch = "wasm32")]
            Source::File(_) => Err(MdfError::BlockSerializationError(
                "file sources are not available on wasm32".to_string(),
            )),
            #[cfg(feature = "http")]
            Source::Url(url) => self.read_many_url(url, &targets, window, options.raw),
        }
    }

    /// Read a numeric channel as plain `f64` values (no timestamps).
    /// Conversions are applied; invalid and non-numeric samples are `NaN`.
    /// `options` as for [`read`](Self::read).
    pub fn values<'o>(&self, name: &str, options: impl Into<Option<&'o ReadOptions>>) -> Result<Vec<f64>, MdfError> {
        let default = ReadOptions::default();
        let options = options.into().unwrap_or(&default);
        let window = options.window()?;
        let (g, c) = self.locate_checked(name, options.group_name())?;
        if window == Window::All {
            return self.read_values_f64_via_source(g, c, options.raw);
        }
        let mut reader = self.source_reader(g, c, options.raw, false)?;
        let (start, count) = self.resolve_window_via(g, c, window, &mut reader)?;
        self.values_f64_window(g, c, start, count, options.raw, &mut reader)
    }

    /// First and last master (time) value of the group holding `name`, or
    /// `None` for an empty group. Reads just two records. Errors if the group
    /// has no master channel.
    pub fn time_range(&self, name: &str, group: Option<&str>) -> Result<Option<TimeRange>, MdfError> {
        let (g, c) = self.locate_checked(name, group)?;
        // Over a URL only the master is read (in batched rounds), so skip
        // `source_reader`, which would also fetch channel `c`'s conversion.
        #[cfg(feature = "http")]
        if let Source::Url(url) = self.require_source()? {
            let mut http = self.http_source(url)?;
            return self.batched_reads(&mut http, |r| self.time_range_at(g, c, r));
        }
        // Only a file source is left here; `raw` would only matter for a URL.
        let mut reader = self.source_reader(g, c, false, true)?;
        self.time_range_at(g, c, &mut reader)
    }

    /// [`resolve_window`](Self::resolve_window) over a [`SourceReader`]: a
    /// time window over a URL is searched in batched rounds (see
    /// [`batched_reads`](Self::batched_reads)) instead of one blocking
    /// request per probe.
    fn resolve_window_via(
        &self,
        g: usize,
        c: usize,
        window: Window,
        reader: &mut SourceReader,
    ) -> Result<(u64, u64), MdfError> {
        match (window, reader) {
            #[cfg(feature = "http")]
            (Window::Time(..), SourceReader::Url(cached)) => {
                self.batched_reads(cached.inner_mut(), |r| self.resolve_window(g, c, window, r))
            }
            (_, reader) => self.resolve_window(g, c, window, reader),
        }
    }

    /// Run `op` against the bytes fetched so far, recording every range it
    /// could not read; fetch all of them **concurrently**, and run it again
    /// until it needs nothing more — the native twin of the wasm gap loop
    /// (`js/src/mdf-index.ts`). A time-window search then costs
    /// ~`log16(records)` rounds of parallel requests instead of one blocking
    /// round-trip per probed record (and per data-block header).
    #[cfg(feature = "http")]
    fn batched_reads<T>(
        &self,
        http: &mut HttpRangeReader,
        op: impl FnMut(&mut RecordingRangeReader<'_>) -> Result<T, MdfError>,
    ) -> Result<T, MdfError> {
        self.batched_reads_seeded(http, &mut FragmentStore::new(), op)
    }

    /// [`batched_reads`](Self::batched_reads) starting from bytes already
    /// fetched (`store`). Large gaps are split into
    /// [`PARALLEL_CHUNK_BYTES`] pieces so one big signal is also fetched over
    /// several connections at once.
    #[cfg(feature = "http")]
    fn batched_reads_seeded<T>(
        &self,
        http: &mut HttpRangeReader,
        store: &mut FragmentStore,
        mut op: impl FnMut(&mut RecordingRangeReader<'_>) -> Result<T, MdfError>,
    ) -> Result<T, MdfError> {
        for _ in 0..MAX_BATCH_ROUNDS {
            let mut recorder = RecordingRangeReader { store: &*store, misses: Vec::new(), whole_request: true };
            let result = op(&mut recorder);
            let misses = std::mem::take(&mut recorder.misses);
            if misses.is_empty() {
                return result;
            }
            let spans = split_spans(merge_ranges_bridging(misses, BATCH_BRIDGE_BYTES), PARALLEL_CHUNK_BYTES);
            let fetched = http.read_ranges_parallel(&spans)?;
            for ((offset, _), bytes) in spans.iter().zip(fetched) {
                store.push(*offset, bytes)?;
            }
        }
        Err(MdfError::BlockSerializationError(
            "batched read did not converge; the index or source may be inconsistent".to_string(),
        ))
    }

    /// The conversion `read` / `values` apply to channel `name`, fully
    /// resolved, or `None` when the channel has none.
    ///
    /// Indexes built over the network record only a conversion's location;
    /// it is then fetched through the attached source on demand.
    pub fn conversion(&self, name: &str, group: Option<&str>) -> Result<Option<ConversionInfo>, MdfError> {
        Ok(self.conversion_block(name, group)?.as_ref().map(ConversionInfo::from))
    }

    /// Locate every name (in the group(s) named `group`, when given), failing
    /// with `ChannelNotFound` on the first unknown one.
    pub(crate) fn locate_all<S: AsRef<str>>(&self, names: &[S], group: Option<&str>) -> Result<Vec<(usize, usize)>, MdfError> {
        names.iter().map(|name| self.locate_checked(name.as_ref(), group)).collect()
    }

    /// A [`Signal`] for channel `(g, c)` from its decoded parts.
    fn make_signal(&self, g: usize, c: usize, timestamps: Vec<f64>, values: Vec<Option<DecodedValue>>) -> Signal {
        let channel = &self.channel_groups[g].channels[c];
        Signal { name: channel.name.clone().unwrap_or_default(), unit: channel.unit.clone(), timestamps, values }
    }

    /// `read_many` of whole channels from an in-memory file (a mapped file
    /// source): the same decoding as [`read`](Self::read), each time axis once.
    #[cfg(not(target_arch = "wasm32"))]
    fn read_many_from_slice(&self, targets: &[(usize, usize)], raw: bool, data: &[u8]) -> Result<Vec<Signal>, MdfError> {
        let mut axes: HashMap<(usize, usize), Vec<f64>> = HashMap::new();
        let mut out = Vec::with_capacity(targets.len());
        for &(g, c) in targets {
            let values = self.read_channel_values_from_slice(g, c, raw, data)?;
            let timestamps = match self.master_location(g, c) {
                Some(m) => match axes.get(&m) {
                    Some(axis) => axis.clone(),
                    None => {
                        let axis = self.read_channel_values_from_slice_as_f64(m.0, m.1, false, data)?;
                        axes.insert(m, axis.clone());
                        axis
                    }
                },
                None => Vec::new(),
            };
            out.push(self.make_signal(g, c, timestamps, values));
        }
        Ok(out)
    }

    /// `read_many` over any byte-range reader — the shared core of the URL,
    /// wasm-fragment and [`MdfReader`] paths.
    ///
    /// With a probing reader
    /// ([`RecordingRangeReader`](crate::fragments::RecordingRangeReader))
    /// every channel is attempted even after one fails, so the gaps of the
    /// whole batch are recorded in one pass, and the first error is
    /// returned; any other reader stops at the first error. Within the call, a window is resolved once per time axis and
    /// each time axis (or its window) is decoded once.
    pub(crate) fn read_many_at<R: ByteRangeReader<Error = MdfError>>(
        &self,
        targets: &[(usize, usize)],
        window: Window,
        raw: bool,
        reader: &mut R,
    ) -> Result<Vec<Signal>, MdfError> {
        let mut windows = WindowCache::new();
        let mut axes = AxisCache::new();
        let mut out = Vec::with_capacity(targets.len());
        let mut first_error = None;
        for &(g, c) in targets {
            match self.read_one_at(g, c, window, raw, reader, &mut windows, &mut axes) {
                Ok(signal) => out.push(signal),
                // Only a probing reader gains from going on (it records the
                // remaining channels' gaps); any other reader stops here, as a
                // loop of `read` would.
                Err(e) if reader.is_probing() => {
                    first_error.get_or_insert(e);
                }
                Err(e) => return Err(e),
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }

    /// One channel of [`read_many_at`](Self::read_many_at): the same reads
    /// as [`read`](Self::read) (`read_signal` / [`read_signal_window`]), with
    /// resolved windows and time axes shared through the two caches.
    #[allow(clippy::too_many_arguments)]
    fn read_one_at<R: ByteRangeReader<Error = MdfError>>(
        &self,
        g: usize,
        c: usize,
        window: Window,
        raw: bool,
        reader: &mut R,
        windows: &mut WindowCache,
        axes: &mut AxisCache,
    ) -> Result<Signal, MdfError> {
        let master = self.master_location(g, c);
        if window == Window::All {
            let values = self.read_channel_values(g, c, raw, reader);
            // Read the time axis even when the values failed, so a probing
            // reader records both gaps in one pass.
            let timestamps = match master {
                Some((mg, m)) => Self::cached_axis(axes, (mg, m, 0, u64::MAX), || {
                    self.read_channel_values_as_f64(mg, m, false, reader)
                }),
                None => Ok(Vec::new()),
            };
            // The channel's own error first, as `read` reports it.
            let values = values?;
            return Ok(self.make_signal(g, c, timestamps?, values));
        }
        let (start, count) = self.cached_window(g, c, window, reader, windows)?;
        let values = self.read_channel_values_window(g, c, start, count, raw, reader);
        let timestamps = match master {
            Some((mg, m)) => Self::cached_axis(axes, (mg, m, start, count), || {
                self.values_f64_window(mg, m, start, count, false, reader)
            }),
            None => Ok(Vec::new()),
        };
        let values = values?;
        Ok(self.make_signal(g, c, timestamps?, values))
    }

    /// [`resolve_window`](Self::resolve_window) through `cache`: a time window
    /// depends on the time axis searched, a record window only on the
    /// group's record count.
    fn cached_window<R: ByteRangeReader<Error = MdfError>>(
        &self,
        g: usize,
        c: usize,
        window: Window,
        reader: &mut R,
        cache: &mut WindowCache,
    ) -> Result<(u64, u64), MdfError> {
        let axis = match window {
            Window::Time(..) => self.time_axis_location(g, c),
            _ => None,
        };
        let key = (axis, self.channel_groups[g].record_count);
        if let Some(&resolved) = cache.get(&key) {
            return Ok(resolved);
        }
        let resolved = self.resolve_window(g, c, window, reader)?;
        cache.insert(key, resolved);
        Ok(resolved)
    }

    fn cached_axis(
        axes: &mut AxisCache,
        key: (usize, usize, u64, u64),
        decode: impl FnOnce() -> Result<Vec<f64>, MdfError>,
    ) -> Result<Vec<f64>, MdfError> {
        if let Some(axis) = axes.get(&key) {
            return Ok(axis.clone());
        }
        let axis = decode()?;
        axes.insert(key, axis.clone());
        Ok(axis)
    }

    /// `read_many` over a URL: deferred conversions are fetched first through
    /// a small read-ahead cache (a `##CC` block and the blocks it references
    /// usually sit together, so this costs one request instead of one round
    /// per chain level), then every data range of the batch — channels and
    /// time axes — is fetched concurrently in [`batched_reads_seeded`]
    /// rounds (one round for whole channels; a few more to search a time
    /// window).
    ///
    /// [`batched_reads_seeded`]: Self::batched_reads_seeded
    #[cfg(feature = "http")]
    fn read_many_url(
        &self,
        url: &str,
        targets: &[(usize, usize)],
        window: Window,
        raw: bool,
    ) -> Result<Vec<Signal>, MdfError> {
        self.read_many_url_limited(url, targets, window, raw, BATCH_MEMORY_LIMIT)
    }

    /// [`read_many_url`](Self::read_many_url) with the size above which
    /// whole channels are streamed instead of fetched as one batch.
    #[cfg(feature = "http")]
    fn read_many_url_limited(
        &self,
        url: &str,
        targets: &[(usize, usize)],
        window: Window,
        raw: bool,
        batch_limit: u64,
    ) -> Result<Vec<Signal>, MdfError> {
        let mut cached = CachingRangeReader::with_chunk_size(self.http_source(url)?, 64 * 1024);
        if self.file_size > 0 {
            cached.set_known_size(self.file_size);
        }
        let mut primed = std::collections::HashSet::new();
        for &(g, c) in targets {
            let mut conversions = Vec::with_capacity(3);
            if !raw {
                conversions.push((g, c));
            }
            conversions.extend(self.master_location(g, c));
            if matches!(window, Window::Time(..)) {
                conversions.extend(self.time_axis_location(g, c));
            }
            for (tg, tc) in conversions {
                let channel = &self.channel_groups[tg].channels[tc];
                if channel.conversion.is_none() && channel.conversion_addr != 0 && primed.insert(channel.conversion_addr) {
                    self.resolve_channel_conversion(tg, tc, &mut cached)?;
                }
            }
        }
        let (mut http, fetched) = cached.into_parts();
        let mut store = FragmentStore::from_pairs(fetched)?;
        let bytes = if window == Window::All {
            self.whole_read_bytes(targets)
        } else {
            // Locate every window first (a few rounds of single-record
            // probes), to know how much the read itself will fetch.
            // A failure here is left to the read below — the streaming
            // path, a plain sequential read that keeps memory bounded and
            // reports the channels' errors in the same order as a loop of
            // `read`.
            match self.batched_reads_seeded(&mut http, &mut store, |r| self.resolve_windows(targets, window, r)) {
                Ok(resolved) => self.window_read_bytes(targets, &resolved),
                Err(_) => u64::MAX,
            }
        };
        if bytes > batch_limit {
            // Too much to hold at once: stream channel by channel (one
            // section in memory at a time), each large section still fetched
            // as concurrent chunks; time axes are still read once, and the
            // window probes are served from the bytes already fetched.
            let mut reader = ChunkedUrlReader { http, store };
            return self.read_many_at(targets, window, raw, &mut reader);
        }
        self.batched_reads_seeded(&mut http, &mut store, |r| self.read_many_at(targets, window, raw, r))
    }

    /// The record window `(start, count)` of every target, resolved as
    /// [`read_many_at`](Self::read_many_at) does (once per time axis). Every
    /// target is attempted, so a probing reader records all probes of a round.
    #[cfg(feature = "http")]
    fn resolve_windows<R: ByteRangeReader<Error = MdfError>>(
        &self,
        targets: &[(usize, usize)],
        window: Window,
        reader: &mut R,
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        let mut cache = WindowCache::new();
        let mut out = Vec::with_capacity(targets.len());
        let mut first_error = None;
        for &(g, c) in targets {
            match self.cached_window(g, c, window, reader, &mut cache) {
                Ok(resolved) => out.push(resolved),
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        match first_error {
            Some(e) => Err(e),
            None => Ok(out),
        }
    }

    /// Upper bound of the bytes a windowed `read_many` fetches once its
    /// windows are resolved: the selected records of every group involved
    /// (each group once, at its widest window) plus the `##SD` streams of
    /// VLSD channels.
    #[cfg(feature = "http")]
    fn window_read_bytes(&self, targets: &[(usize, usize)], resolved: &[(u64, u64)]) -> u64 {
        let mut records: std::collections::BTreeMap<usize, u64> = std::collections::BTreeMap::new();
        let mut total = 0u64;
        for (&(g, c), &(_, count)) in targets.iter().zip(resolved) {
            for group in std::iter::once(g).chain(self.master_location(g, c).map(|(mg, _)| mg)) {
                let widest = records.entry(group).or_default();
                *widest = (*widest).max(count);
            }
            let channel = &self.channel_groups[g].channels[c];
            total = total.saturating_add(channel.vlsd_data_blocks.iter().map(|b| b.size).sum());
        }
        records.into_iter().fold(total, |sum, (g, count)| {
            let record = Self::checked_record_size(&self.channel_groups[g]).unwrap_or(0) as u64;
            sum.saturating_add(count.saturating_mul(record))
        })
    }

    /// Bytes a whole-channel `read_many` of `targets` fetches: every data
    /// block of their groups and of their time groups (each group once),
    /// plus the `##SD` streams of VLSD channels.
    #[cfg(feature = "http")]
    fn whole_read_bytes(&self, targets: &[(usize, usize)]) -> u64 {
        let mut groups = std::collections::BTreeSet::new();
        let mut total = 0u64;
        for &(g, c) in targets {
            groups.insert(g);
            if let Some((mg, _)) = self.master_location(g, c) {
                groups.insert(mg);
            }
            let channel = &self.channel_groups[g].channels[c];
            total = total.saturating_add(channel.vlsd_data_blocks.iter().map(|b| b.size).sum());
        }
        groups
            .iter()
            .flat_map(|&g| self.channel_groups[g].data_blocks.iter())
            .fold(total, |sum, b| sum.saturating_add(b.size))
    }

    /// Records `[start, start + count)` a window selects in the group of `(g, c)`.
    pub(crate) fn resolve_window<R: ByteRangeReader<Error = MdfError>>(
        &self,
        g: usize,
        c: usize,
        window: Window,
        reader: &mut R,
    ) -> Result<(u64, u64), MdfError> {
        let total = self.channel_groups[g].record_count;
        match window {
            Window::All => Ok((0, total)),
            Window::Records(start, count) => resolve_records(start, count, total),
            Window::Time(t_start, t_end) => self.record_window_for_time(g, c, t_start, t_end, reader),
        }
    }

    /// `f64` values of records `[start, start + count)` of `(g, c)`.
    pub(crate) fn values_f64_window<R: ByteRangeReader<Error = MdfError>>(
        &self,
        g: usize,
        c: usize,
        start: u64,
        count: u64,
        raw: bool,
        reader: &mut R,
    ) -> Result<Vec<f64>, MdfError> {
        // Straight to f64: the same values as `read_channel_values_window`
        // mapped through `decoded_opt_to_f64`, without the `DecodedValue`s.
        let (group, channel) = self.group_and_channel(g, c)?;
        Self::check_window(group, start, count)?;
        let conversion = Self::read_conversion(channel, reader, raw)?;
        self.window_values_f64(group, channel, conversion.as_deref(), start, count, reader)
    }

    /// First and last value of the time axis of `(g, c)` (two single-record reads).
    pub(crate) fn time_range_at<R: ByteRangeReader<Error = MdfError>>(
        &self,
        g: usize,
        c: usize,
        reader: &mut R,
    ) -> Result<Option<TimeRange>, MdfError> {
        let total = self.channel_groups[g].record_count;
        let Some((mg, m)) = self.time_axis_location(g, c) else {
            if total == 0 {
                return Ok(None);
            }
            let name = self.channel_groups[g].channels[c].name.clone().unwrap_or_default();
            return Err(MdfError::InvalidArgument(format!(
                "channel '{name}' has no master (time) channel"
            )));
        };
        let n = total.min(self.channel_groups[mg].record_count);
        if n == 0 {
            return Ok(None);
        }
        let first = self.values_f64_window(mg, m, 0, 1, false, reader);
        // Attempt the second read even when the first failed, so a probing
        // reader records both gaps in one pass.
        let last = self.values_f64_window(mg, m, n - 1, 1, false, reader)?;
        Ok(Some(TimeRange { start: first?[0], end: last[0] }))
    }

    /// Memory-map a file source, naming it in the error when it cannot be
    /// opened and refusing it when its size differs from the indexed file.
    #[cfg(not(target_arch = "wasm32"))]
    fn map_source_file(&self, path: &str) -> Result<memmap2::Mmap, MdfError> {
        if is_http_url(path) {
            #[cfg(not(feature = "http"))]
            return Err(MdfError::InvalidArgument(format!(
                "'{path}' is a URL, but mf4-rs was built without the `http` feature"
            )));
            #[cfg(feature = "http")]
            return Err(MdfError::InvalidArgument(format!(
                "'{path}' is a URL, not a local file: attach URLs with `set_source` or build the \
                 index with `from_url`"
            )));
        }
        if let Some(scheme) = url_scheme(path) {
            return Err(MdfError::InvalidArgument(format!(
                "unsupported source '{path}': {scheme}:// URLs cannot be read directly — use a \
                 local file path or an http(s):// URL (e.g. a presigned S3 / GCS / Azure URL)"
            )));
        }
        let file = std::fs::File::open(path).map_err(|e| {
            MdfError::IOError(std::io::Error::new(e.kind(), format!("cannot open source '{path}': {e}")))
        })?;
        let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(MdfError::IOError)?;
        check_resource_size(path, self.file_size, mmap.len() as u64)?;
        Ok(mmap)
    }

    /// An HTTP reader for a URL source that checks every response against
    /// the indexed file size.
    #[cfg(feature = "http")]
    fn http_source(&self, url: &str) -> Result<HttpRangeReader, MdfError> {
        let mut http = HttpRangeReader::new(url)?;
        if self.file_size > 0 {
            http.expect_size(self.file_size);
        }
        Ok(http)
    }

    /// A byte-range reader over the attached source, with the conversion of
    /// `(g, c)` resolved up front for URL sources (unless `raw`).
    /// `with_master`: the read decodes the time axis through this reader (a
    /// signal), so the master's conversion is primed too. A URL time-window
    /// search runs through `batched_reads`, which bypasses this cache.
    fn source_reader(&self, g: usize, c: usize, raw: bool, with_master: bool) -> Result<SourceReader, MdfError> {
        let _ = (g, c, raw, with_master);
        match self.require_source()? {
            #[cfg(not(target_arch = "wasm32"))]
            Source::File(path) => Ok(SourceReader::File(MmapRangeReader { mmap: self.map_source_file(path)? })),
            #[cfg(target_arch = "wasm32")]
            Source::File(_) => Err(MdfError::BlockSerializationError(
                "file sources are not available on wasm32".to_string(),
            )),
            #[cfg(feature = "http")]
            Source::Url(url) => {
                let http = self.http_source(url)?;
                Ok(SourceReader::Url(self.url_reader_with_conversion(http, g, c, raw, with_master)?))
            }
        }
    }

    /// Decode a channel + its group master from the attached source.
    fn read_signal(&self, g: usize, c: usize, raw: bool) -> Result<Signal, MdfError> {
        let channel = &self.channel_groups[g].channels[c];
        let (name, unit) = (channel.name.clone().unwrap_or_default(), channel.unit.clone());

        // A file source is mapped once for the values and the time axis.
        #[cfg(not(target_arch = "wasm32"))]
        #[allow(irrefutable_let_patterns)] // `File` is the only source without `http`
        if let Source::File(path) = self.require_source()? {
            let mmap = self.map_source_file(path)?;
            let values = self.read_channel_values_from_slice(g, c, raw, &mmap)?;
            let timestamps = match self.master_location(g, c) {
                Some((mg, m)) => self.read_channel_values_from_slice_as_f64(mg, m, false, &mmap)?,
                None => Vec::new(),
            };
            return Ok(Signal { name, unit, timestamps, values });
        }

        let values = self.read_values_via_source(g, c, raw)?;
        let timestamps = match self.master_location(g, c) {
            Some((mg, m)) => self.read_values_f64_via_source(mg, m, false)?,
            None => Vec::new(),
        };

        Ok(Signal { name, unit, timestamps, values })
    }

    /// Wrap `http` for a value read of `(g, c)`: a deferred conversion is
    /// fetched first through a small (64 KiB) read-ahead cache — the `##CC`
    /// block and the text/conversion blocks it references usually sit next to
    /// each other, so this costs one request instead of one per block — then
    /// the cache switches to bypass mode so the bulk data is streamed directly
    /// (cached conversion bytes are still served from memory).
    ///
    /// A `raw` read skips the channel's conversion. `with_master` also primes
    /// the group master's, for reads that decode the time axis through this
    /// reader (windows, signals): otherwise a raw read would resolve it later
    /// in bypass mode, one sequential request per block.
    #[cfg(feature = "http")]
    fn url_reader_with_conversion(
        &self,
        http: HttpRangeReader,
        g: usize,
        c: usize,
        raw: bool,
        with_master: bool,
    ) -> Result<CachingRangeReader<HttpRangeReader>, MdfError> {
        let mut cached = CachingRangeReader::with_chunk_size(http, 64 * 1024);
        if self.file_size > 0 {
            cached.set_known_size(self.file_size);
        }
        let mut targets = Vec::with_capacity(2);
        if !raw {
            targets.push((g, c));
        }
        if with_master {
            targets.extend(self.master_location(g, c));
        }
        for (tg, tc) in targets {
            let channel = &self.channel_groups[tg].channels[tc];
            if channel.conversion.is_none() && channel.conversion_addr != 0 {
                self.resolve_channel_conversion(tg, tc, &mut cached)?;
            }
        }
        cached.set_bypass(true);
        Ok(cached)
    }

    /// Resolve the attached [`Source`], erroring with a helpful message if none.
    fn require_source(&self) -> Result<&Source, MdfError> {
        let source = self.source.as_ref().ok_or_else(|| {
            MdfError::BlockSerializationError(
                "no data source attached to index; build with from_file/from_url or call set_source".to_string(),
            )
        })?;
        #[cfg(not(feature = "http"))]
        {
            let Source::File(path) = source;
            if is_http_url(path) {
                return Err(MdfError::InvalidArgument(format!(
                    "'{path}' is a URL, but mf4-rs was built without the `http` feature"
                )));
            }
        }
        Ok(source)
    }

    /// Read one channel's decoded values lazily through the attached source.
    pub(crate) fn read_values_via_source(
        &self,
        g: usize,
        c: usize,
        raw: bool,
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        #[cfg(target_arch = "wasm32")]
        let _ = (g, c, raw);
        match self.require_source()? {
            #[cfg(not(target_arch = "wasm32"))]
            Source::File(path) => {
                let mmap = self.map_source_file(path)?;
                self.read_channel_values_from_slice(g, c, raw, &mmap)
            }
            #[cfg(target_arch = "wasm32")]
            Source::File(_) => Err(MdfError::BlockSerializationError(
                "file sources are not available on wasm32".to_string(),
            )),
            #[cfg(feature = "http")]
            Source::Url(url) => {
                let http = self.http_source(url)?;
                let mut cached = self.url_reader_with_conversion(http, g, c, raw, false)?;
                self.read_channel_values(g, c, raw, &mut cached)
            }
        }
    }

    /// Resolve a channel's conversion, fetching it through the attached source
    /// if it was not resolved at index-build time (the remote / range-reader
    /// path records only its location — see [`IndexedChannel::conversion_addr`]).
    ///
    /// Returns `None` when the channel has no conversion. Errors if resolution
    /// needs the source (a deferred conversion) but none is attached — attach
    /// one with [`set_source`](Self::set_source).
    ///
    /// This returns the raw [`ConversionBlock`]; [`conversion`](Self::conversion)
    /// is the contract method returning a plain [`ConversionInfo`].
    pub fn conversion_block(&self, name: &str, group: Option<&str>) -> Result<Option<ConversionBlock>, MdfError> {
        let (g, c) = self.locate_checked(name, group)?;
        let channel = &self.channel_groups[g].channels[c];
        if let Some(conv) = &channel.conversion {
            return Ok(Some(conv.clone()));
        }
        if channel.conversion_addr == 0 {
            return Ok(None);
        }
        match self.require_source()? {
            #[cfg(not(target_arch = "wasm32"))]
            Source::File(path) => {
                let mmap = self.map_source_file(path)?;
                let mut slice_reader = BorrowedSliceReader { data: &mmap };
                Ok(Self::resolve_conversion(channel, &mut slice_reader)?.map(Cow::into_owned))
            }
            #[cfg(target_arch = "wasm32")]
            Source::File(_) => Err(MdfError::BlockSerializationError(
                "file sources are not available on wasm32".to_string(),
            )),
            #[cfg(feature = "http")]
            Source::Url(url) => {
                let http = self.http_source(url)?;
                let mut cached = CachingRangeReader::new(http);
                Ok(Self::resolve_conversion(channel, &mut cached)?.map(Cow::into_owned))
            }
        }
    }

    /// Read one channel's values as `f64` lazily through the attached source.
    pub(crate) fn read_values_f64_via_source(
        &self,
        g: usize,
        c: usize,
        raw: bool,
    ) -> Result<Vec<f64>, MdfError> {
        #[cfg(target_arch = "wasm32")]
        let _ = (g, c, raw);
        match self.require_source()? {
            #[cfg(not(target_arch = "wasm32"))]
            Source::File(path) => {
                let mmap = self.map_source_file(path)?;
                self.read_channel_values_from_slice_as_f64(g, c, raw, &mmap)
            }
            #[cfg(target_arch = "wasm32")]
            Source::File(_) => Err(MdfError::BlockSerializationError(
                "file sources are not available on wasm32".to_string(),
            )),
            #[cfg(feature = "http")]
            Source::Url(url) => {
                let http = self.http_source(url)?;
                let mut cached = self.url_reader_with_conversion(http, g, c, raw, false)?;
                self.read_channel_values_as_f64(g, c, raw, &mut cached)
            }
        }
    }

    /// Get the exact byte ranges needed to read all data for a specific channel
    /// 
    /// Returns a vector of (file_offset, length) tuples representing the byte ranges
    /// that need to be read from the file to get all data for the specified channel.
    /// 
    /// # Arguments
    /// * `group_index` - Index of the channel group
    /// * `channel_index` - Index of the channel within the group
    /// 
    /// # Returns
    /// * `Ok(Vec<(u64, u64)>)` - Vector of (offset, length) byte ranges
    /// * `Err(MdfError)` - If indices are invalid or channel type not supported
    pub(crate) fn get_channel_byte_ranges(
        &self,
        group_index: usize,
        channel_index: usize,
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        // VLSD channels not supported for byte range calculation
        Self::ensure_not_vlsd(channel)?;

        // For regular channels, calculate byte ranges from data blocks
        self.calculate_regular_channel_byte_ranges(group, channel)
    }

    /// Get the exact byte ranges for a specific record range of a channel
    /// 
    /// This is useful when you only want to read a subset of records rather than all data.
    /// 
    /// # Arguments
    /// * `group_index` - Index of the channel group
    /// * `channel_index` - Index of the channel within the group
    /// * `start_record` - Starting record index (0-based)
    /// * `record_count` - Number of records to read
    /// 
    /// # Returns
    /// * `Ok(Vec<(u64, u64)>)` - Vector of (offset, length) byte ranges
    /// * `Err(MdfError)` - If indices are invalid, range is out of bounds, or channel type not supported
    pub(crate) fn get_channel_byte_ranges_for_records(
        &self,
        group_index: usize,
        channel_index: usize,
        start_record: u64,
        record_count: u64,
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        // Validate record range with checked arithmetic (forged inputs must
        // error, not wrap), and without underflowing when record_count == 0.
        let end_record = start_record.checked_add(record_count).ok_or_else(|| {
            MdfError::BlockSerializationError(format!(
                "Record range overflow: start {} + count {}",
                start_record, record_count
            ))
        })?;
        if end_record > group.record_count {
            return Err(MdfError::BlockSerializationError(
                format!("Record range starting at {} for {} records exceeds total records {}",
                    start_record, record_count, group.record_count)
            ));
        }

        // VLSD channels not supported for byte range calculation
        Self::ensure_not_vlsd(channel)?;

        self.calculate_channel_byte_ranges_for_records(group, channel, start_record, record_count)
    }

    /// Calculate byte ranges for a regular (non-VLSD) channel for all records
    fn calculate_regular_channel_byte_ranges(
        &self,
        group: &IndexedChannelGroup,
        channel: &IndexedChannel,
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        self.calculate_channel_byte_ranges_for_records(group, channel, 0, group.record_count)
    }

    /// Calculate byte ranges for a regular channel for a specific record range
    fn calculate_channel_byte_ranges_for_records(
        &self,
        group: &IndexedChannelGroup,
        channel: &IndexedChannel,
        start_record: u64,
        record_count: u64,
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        // Record structure: record_id + data_bytes + invalidation_bytes
        let record_size = Self::checked_record_size(group)?;
        let channel_offset_in_record = group.record_id_len as usize + channel.byte_offset as usize;
        
        // Calculate how many bytes this channel needs per record
        let channel_bytes_per_record = if matches!(channel.data_type,
            DataType::StringLatin1 | DataType::StringUtf8 | DataType::StringUtf16LE | 
            DataType::StringUtf16BE | DataType::ByteArray | DataType::MimeSample | DataType::MimeStream)
        {
            channel.bit_count as usize / 8
        } else {
            (channel.bit_offset as usize + channel.bit_count as usize).div_ceil(8).max(1)
        };

        let mut byte_ranges = Vec::new();
        let mut records_processed = 0u64;
        
        for data_block in &group.data_blocks {
            if data_block.is_compressed {
                return Err(MdfError::BlockSerializationError(
                    "Compressed blocks not supported for byte range calculation".to_string()
                ));
            }

            let block_data_start = data_block.file_offset + 24; // Skip block header
            let records_in_block = Self::records_in_block(data_block, record_size)?;
            
            // Determine which records from this block we need
            let block_start_record = records_processed;
            let block_end_record = records_processed + records_in_block;
            
            let need_start = start_record.max(block_start_record);
            let need_end = (start_record + record_count).min(block_end_record);
            
            if need_start < need_end {
                // We need some records from this block
                let first_record_in_block = need_start - block_start_record;
                let last_record_in_block = need_end - block_start_record - 1;
                
                // Calculate byte range for the channel data in these records
                let first_channel_byte = block_data_start + 
                    first_record_in_block * record_size as u64 + 
                    channel_offset_in_record as u64;
                
                let last_channel_byte = block_data_start + 
                    last_record_in_block * record_size as u64 + 
                    channel_offset_in_record as u64 + 
                    channel_bytes_per_record as u64 - 1;
                
                let range_length = last_channel_byte - first_channel_byte + 1;
                byte_ranges.push((first_channel_byte, range_length));
            }
            
            records_processed = block_end_record;
            
            // Early exit if we've processed all needed records
            if records_processed >= start_record + record_count {
                break;
            }
        }
        
        Ok(byte_ranges)
    }

    /// Byte ranges occupied by a channel across the whole file, by name.
    ///
    /// Each tuple is `(offset, length)`, accounting for the channel's position
    /// in the record layout and any data-block splitting. Resolves the first
    /// channel matching `name`. Power-user entry point for issuing HTTP-range
    /// or S3 partial reads yourself.
    ///
    /// Note: each returned range is one *coalesced* span per data-block
    /// fragment, running from the first byte of the channel in the fragment's
    /// first record to its last byte in the fragment's last record. Because
    /// records interleave all channels of the group, the span **includes the
    /// bytes of the other channels** lying between this channel's samples.
    ///
    /// Pass `group` to resolve the channel in the group(s) with that name.
    /// Refused for VLSD channels (use `read` / `values`).
    pub fn byte_ranges(&self, name: &str, group: Option<&str>) -> Result<Vec<(u64, u64)>, MdfError> {
        let (g, c) = self.locate_checked(name, group)?;
        self.get_channel_byte_ranges(g, c)
    }

    /// Byte ranges for a record window of a channel, by name.
    ///
    /// `start_record` is 0-based. Errors if `start_record + record_count`
    /// exceeds the records available. Useful for paging through a large
    /// channel. See [`MdfIndex::byte_ranges`] for what each returned span
    /// covers.
    pub fn byte_ranges_for_records(
        &self,
        name: &str,
        start_record: u64,
        record_count: u64,
        group: Option<&str>,
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        let (g, c) = self.locate_checked(name, group)?;
        self.get_channel_byte_ranges_for_records(g, c, start_record, record_count)
    }

    /// Fast path: read channel values as `Vec<f64>` using a byte range reader.
    ///
    /// This avoids boxing `DecodedValue` enums and applies linear conversions inline.
    /// For channels without invalidation bytes (the common case), validity checking
    /// is skipped entirely. Invalid samples are represented as `f64::NAN`.
    pub(crate) fn read_channel_values_as_f64<R: ByteRangeReader<Error = MdfError>>(
        &self,
        group_index: usize,
        channel_index: usize,
        raw: bool,
        reader: &mut R,
    ) -> Result<Vec<f64>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        let conversion = Self::read_conversion(channel, reader, raw)?;

        if channel.is_virtual() {
            let vals = self.virtual_values(group, 0, group.record_count, conversion.as_deref())?;
            return Ok(Self::vlsd_values_to_f64(vals));
        }
        if channel.channel_type == 1 {
            let vals = self.read_vlsd_channel_values(group, channel, conversion.as_deref(), reader)?;
            return Ok(Self::vlsd_values_to_f64(vals));
        }

        let record_size = Self::checked_record_size(group)?;

        let mut total_records: usize = 0;
        for data_block in &group.data_blocks {
            if data_block.is_compressed {
                return Err(MdfError::BlockSerializationError(
                    "Compressed blocks not yet supported in index reader".to_string()
                ));
            }
            total_records += Self::records_in_block(data_block, record_size)? as usize;
        }
        // Cap the pre-allocation: `total_records` derives from index-supplied
        // block sizes, and a forged value must not abort on allocation. The
        // vector still grows as needed; real reads fail earlier at the reader.
        let mut values = Vec::with_capacity(total_records.min(1 << 24));

        let temp_cb = channel.to_decode_only_channel_block();
        let linear_coeffs = Self::get_linear_coeffs(conversion.as_deref());
        let has_conversion = conversion.is_some();

        Self::for_each_data_section(reader, &group.data_blocks, |block_data| {
            Self::decode_records_to_f64(block_data, record_size, group, channel, conversion.as_deref(), &temp_cb, linear_coeffs, has_conversion, &mut values)
        })?;

        Ok(values)
    }

    /// Zero-copy fast path: read channel values directly from an `&[u8]` mmap slice.
    ///
    /// Avoids all per-block heap allocation by slicing directly into the provided
    /// memory-mapped region. This is the fastest `DecodedValue` read path when the
    /// entire file is already mapped into memory.
    #[allow(dead_code)] // used by the Python bindings (pyo3 feature)
    pub(crate) fn read_channel_values_from_slice(
        &self,
        group_index: usize,
        channel_index: usize,
        raw: bool,
        file_data: &[u8],
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        let mut slice_reader = BorrowedSliceReader { data: file_data };
        let conversion = Self::read_conversion(channel, &mut slice_reader, raw)?;

        if channel.is_virtual() {
            return self.virtual_values(group, 0, group.record_count, conversion.as_deref());
        }
        if channel.channel_type == 1 {
            return self.read_vlsd_channel_values(group, channel, conversion.as_deref(), &mut slice_reader);
        }

        let record_size = Self::checked_record_size(group)?;
        let mut total_records: usize = 0;
        for data_block in &group.data_blocks {
            if data_block.is_compressed {
                return Err(MdfError::BlockSerializationError(
                    "Compressed blocks not yet supported in index reader".to_string()
                ));
            }
            total_records += Self::records_in_block(data_block, record_size)? as usize;
        }
        // Cap the pre-allocation: `total_records` derives from index-supplied
        // block sizes, and a forged value must not abort on allocation. The
        // vector still grows as needed; real reads fail earlier at the reader.
        let mut values = Vec::with_capacity(total_records.min(1 << 24));
        let temp_cb = channel.to_decode_only_channel_block();

        for data_block in &group.data_blocks {
            let block_data = Self::slice_data_block(file_data, data_block)?;
            Self::decode_records_to_values(block_data, record_size, group, conversion.as_deref(), &temp_cb, &mut values)?;
        }

        Ok(values)
    }

    /// Zero-copy fast path: read channel values as `Vec<f64>` directly from an `&[u8]` mmap slice.
    ///
    /// Combines zero-copy slice access with the f64 fast decode path. No per-block
    /// allocation, no `DecodedValue` enum boxing. This is the fastest possible read
    /// path. Invalid or undecodable samples are `f64::NAN`.
    #[allow(dead_code)] // used by the Python bindings (pyo3 feature)
    pub(crate) fn read_channel_values_from_slice_as_f64(
        &self,
        group_index: usize,
        channel_index: usize,
        raw: bool,
        file_data: &[u8],
    ) -> Result<Vec<f64>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        let mut slice_reader = BorrowedSliceReader { data: file_data };
        let conversion = Self::read_conversion(channel, &mut slice_reader, raw)?;

        if channel.is_virtual() {
            let vals = self.virtual_values(group, 0, group.record_count, conversion.as_deref())?;
            return Ok(Self::vlsd_values_to_f64(vals));
        }
        if channel.channel_type == 1 {
            let vals = self.read_vlsd_channel_values(group, channel, conversion.as_deref(), &mut slice_reader)?;
            return Ok(Self::vlsd_values_to_f64(vals));
        }

        let record_size = Self::checked_record_size(group)?;
        let mut total_records: usize = 0;
        for data_block in &group.data_blocks {
            if data_block.is_compressed {
                return Err(MdfError::BlockSerializationError(
                    "Compressed blocks not yet supported in index reader".to_string()
                ));
            }
            total_records += Self::records_in_block(data_block, record_size)? as usize;
        }
        // Cap the pre-allocation: `total_records` derives from index-supplied
        // block sizes, and a forged value must not abort on allocation. The
        // vector still grows as needed; real reads fail earlier at the reader.
        let mut values = Vec::with_capacity(total_records.min(1 << 24));
        let temp_cb = channel.to_decode_only_channel_block();
        let linear_coeffs = Self::get_linear_coeffs(conversion.as_deref());
        let has_conversion = conversion.is_some();

        for data_block in &group.data_blocks {
            let block_data = Self::slice_data_block(file_data, data_block)?;
            Self::decode_records_to_f64(block_data, record_size, group, channel, conversion.as_deref(), &temp_cb, linear_coeffs, has_conversion, &mut values)?;
        }

        Ok(values)
    }

    /// Slice a data block from file_data, skipping the 24-byte block header.
    /// Uses checked arithmetic so a forged offset/size errors instead of
    /// wrapping or panicking.
    #[allow(dead_code)] // used by the Python bindings (pyo3 feature)
    fn slice_data_block<'a>(file_data: &'a [u8], data_block: &DataBlockInfo) -> Result<&'a [u8], MdfError> {
        Self::data_section_size(data_block)?;
        let too_short = |expected: usize| MdfError::TooShortBuffer {
            actual: file_data.len(),
            expected,
            file: file!(),
            line: line!(),
        };
        let data_start = usize::try_from(data_block.file_offset)
            .ok()
            .and_then(|o| o.checked_add(24))
            .ok_or_else(|| too_short(usize::MAX))?;
        // The block header's own length wins over the indexed size.
        let data_len = Self::verified_data_len(file_data.get(data_start - 24..data_start), data_block)?;
        let data_end = usize::try_from(data_len)
            .ok()
            .and_then(|l| data_start.checked_add(l))
            .ok_or_else(|| too_short(usize::MAX))?;
        file_data.get(data_start..data_end).ok_or_else(|| too_short(data_end))
    }
}

/// A reader bound to an [`MdfIndex`] and a single byte-range data source.
///
/// Obtained from [`MdfIndex::open`] / [`MdfIndex::open_file`]. The source is
/// supplied once; afterwards channels are read by name. Reads of the same
/// channel re-fetch from the underlying source, so cache or memory-map the
/// source (e.g. [`MmapRangeReader`], [`CachingRangeReader`]) when reading many
/// channels.
pub struct MdfReader<'a, R: ByteRangeReader<Error = MdfError>> {
    index: &'a MdfIndex,
    reader: R,
}

impl<'a, R: ByteRangeReader<Error = MdfError>> MdfReader<'a, R> {
    /// The index this reader was opened against.
    pub fn index(&self) -> &MdfIndex {
        self.index
    }

    /// Mutable access to the underlying byte-range reader (e.g. to toggle
    /// [`CachingRangeReader::set_bypass`] or inspect request counters).
    pub fn reader_mut(&mut self) -> &mut R {
        &mut self.reader
    }

    /// Consume the reader, returning the underlying byte-range source.
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Read a channel as a [`Signal`] — see [`MdfIndex::read`].
    pub fn read<'o>(&mut self, name: &str, options: impl Into<Option<&'o ReadOptions>>) -> Result<Signal, MdfError> {
        let default = ReadOptions::default();
        let options = options.into().unwrap_or(&default);
        let window = options.window()?;
        let (g, c) = self.index.locate_checked(name, options.group_name())?;
        if window == Window::All {
            return self.read_signal(g, c, options.raw);
        }
        let (start, count) = self.index.resolve_window(g, c, window, &mut self.reader)?;
        read_signal_window(self.index, g, c, start, count, options.raw, &mut self.reader)
    }

    /// Read several channels at once — see [`MdfIndex::read_many`].
    pub fn read_many<'o, S: AsRef<str>>(
        &mut self,
        names: &[S],
        options: impl Into<Option<&'o ReadOptions>>,
    ) -> Result<Vec<Signal>, MdfError> {
        let default = ReadOptions::default();
        let options = options.into().unwrap_or(&default);
        let window = options.window()?;
        let targets = self.index.locate_all(names, options.group_name())?;
        self.index.read_many_at(&targets, window, options.raw, &mut self.reader)
    }

    /// Read a numeric channel as plain `f64` values — see [`MdfIndex::values`].
    pub fn values<'o>(&mut self, name: &str, options: impl Into<Option<&'o ReadOptions>>) -> Result<Vec<f64>, MdfError> {
        let default = ReadOptions::default();
        let options = options.into().unwrap_or(&default);
        let window = options.window()?;
        let (g, c) = self.index.locate_checked(name, options.group_name())?;
        if window == Window::All {
            return self.index.read_channel_values_as_f64(g, c, options.raw, &mut self.reader);
        }
        let (start, count) = self.index.resolve_window(g, c, window, &mut self.reader)?;
        self.index.values_f64_window(g, c, start, count, options.raw, &mut self.reader)
    }

    /// Rust-only: all samples of a channel as decoded values (no time axis),
    /// keeping text and byte values that `values` would turn into `NaN`.
    pub fn decoded_values(&mut self, name: &str, group: Option<&str>) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        let (g, c) = self.index.locate_checked(name, group)?;
        self.index.read_channel_values(g, c, false, &mut self.reader)
    }

    /// First and last master (time) value of the group holding `name` — see
    /// [`MdfIndex::time_range`].
    pub fn time_range(&mut self, name: &str, group: Option<&str>) -> Result<Option<TimeRange>, MdfError> {
        let (g, c) = self.index.locate_checked(name, group)?;
        self.index.time_range_at(g, c, &mut self.reader)
    }

    fn read_signal(&mut self, g: usize, c: usize, raw: bool) -> Result<Signal, MdfError> {
        let channel = &self.index.channel_groups[g].channels[c];
        let (name, unit) = (channel.name.clone().unwrap_or_default(), channel.unit.clone());

        let values = self.index.read_channel_values(g, c, raw, &mut self.reader)?;
        let timestamps = match self.index.master_location(g, c) {
            Some((mg, m)) => self.index.read_channel_values_as_f64(mg, m, false, &mut self.reader)?,
            None => Vec::new(),
        };
        Ok(Signal { name, unit, timestamps, values })
    }
}

/// Window read shared by [`MdfReader`] and the wasm fragment driver: records
/// `[start, start + count)` of `(g, c)` paired with the same window of its
/// time axis.
pub(crate) fn read_signal_window<R: ByteRangeReader<Error = MdfError>>(
    index: &MdfIndex,
    g: usize,
    c: usize,
    start: u64,
    count: u64,
    raw: bool,
    reader: &mut R,
) -> Result<Signal, MdfError> {
    let channel = &index.channel_groups[g].channels[c];
    let (name, unit) = (channel.name.clone().unwrap_or_default(), channel.unit.clone());
    let values = index.read_channel_values_window(g, c, start, count, raw, reader);
    // Read the master even when the values failed, so a probing reader
    // records both gaps in one pass. Like a full read, the master itself
    // carries no separate timestamps.
    let timestamps = match index.master_location(g, c) {
        Some((mg, m)) => index.values_f64_window(mg, m, start, count, false, reader)?,
        None => Vec::new(),
    };
    Ok(Signal { name, unit, timestamps, values: values? })
}

#[cfg(all(test, feature = "http"))]
mod http_header_tests {
    use super::{content_range_span, content_range_total};

    #[test]
    fn parses_content_range() {
        assert_eq!(content_range_span(Some("bytes 10-19/100")), Some((10, 19)));
        assert_eq!(content_range_span(Some(" bytes 0-0/*")), Some((0, 0)));
        assert_eq!(content_range_span(Some("bytes */100")), None);
        assert_eq!(content_range_span(None), None);
        assert_eq!(content_range_total(Some("bytes 10-19/100")), Some(100));
        assert_eq!(content_range_total(Some("bytes */100")), Some(100));
        assert_eq!(content_range_total(Some("bytes 0-0/*")), None);
    }
}

#[cfg(all(test, feature = "http"))]
mod http_retry_tests {
    use super::{ByteRangeReader, HttpRangeReader};
    use std::collections::VecDeque;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    const OK: &str = "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-3/10\r\nContent-Length: 4\r\n\
                      Connection: close\r\n\r\nabcd";
    const BUSY: &str = "HTTP/1.1 503 Slow Down\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    const MISSING: &str = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

    /// Serve `script` (one response per connection) and count requests.
    fn serve(script: &[&'static str]) -> (String, Arc<AtomicU64>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.mf4", listener.local_addr().unwrap());
        let script = Arc::new(Mutex::new(script.iter().copied().collect::<VecDeque<_>>()));
        let count = Arc::new(AtomicU64::new(0));
        let served = count.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                served.fetch_add(1, Ordering::SeqCst);
                let response = script.lock().unwrap().pop_front().unwrap_or(MISSING);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (url, count)
    }

    #[test]
    fn transient_failures_are_retried() {
        let (url, count) = serve(&[BUSY, BUSY, OK]);
        let bytes = HttpRangeReader::new(url).unwrap().read_range(0, 4).unwrap();
        assert_eq!(bytes, b"abcd");
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn retries_are_bounded() {
        let (url, count) = serve(&[BUSY, BUSY, BUSY, BUSY, BUSY, OK]);
        let error = HttpRangeReader::new(url).unwrap().read_range(0, 4).unwrap_err().to_string();
        assert!(error.contains("503") && error.contains("gave up after 4 attempts"), "{error}");
        assert_eq!(count.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn permanent_failures_are_not_retried() {
        let (url, count) = serve(&[MISSING, OK]);
        let error = HttpRangeReader::new(url).unwrap().read_range(0, 4).unwrap_err().to_string();
        assert!(error.contains("404"), "{error}");
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}

#[cfg(all(test, feature = "http"))]
mod url_read_tests {
    use super::{DataBlockInfo, MdfIndex, Window};
    use crate::fragments::{FragmentStore, RecordingRangeReader};
    use crate::{ColumnData, Mdf, MdfWriter, ReadOptions};
    use std::io::{BufRead, BufReader, Write};
    use std::sync::Arc;

    /// Thread-per-connection server honouring single `Range` requests.
    fn serve(bytes: Vec<u8>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f.mf4", listener.local_addr().unwrap());
        let bytes = Arc::new(bytes);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let bytes = bytes.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut stream = stream;
                    loop {
                        let mut range = None;
                        let mut line = String::new();
                        loop {
                            line.clear();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                return;
                            }
                            if line == "\r\n" {
                                break;
                            }
                            if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                                let (a, b) = v.trim().split_once('-').unwrap();
                                range = Some((a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap()));
                            }
                        }
                        let (a, b) = range.unwrap_or((0, bytes.len() - 1));
                        let b = b.min(bytes.len() - 1);
                        let head = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{b}/{}\r\nContent-Length: {}\r\n\r\n",
                            bytes.len(),
                            b + 1 - a
                        );
                        if stream.write_all(head.as_bytes()).is_err() || stream.write_all(&bytes[a..=b]).is_err() {
                            return;
                        }
                    }
                });
            }
        });
        url
    }

    fn build(path: &std::path::Path) {
        let mut w = MdfWriter::new(path.to_str().unwrap()).unwrap();
        w.init_mdf_file().unwrap();
        let g = w.add_channel_group(Some("G")).unwrap();
        w.add_time_channel(&g, "t").unwrap();
        let a = w.add_float_channel(&g, "A").unwrap();
        w.add_float_channel(&g, "B").unwrap();
        w.add_linear_conversion(&a, 1.0, 2.0).unwrap();
        let n = 300_000;
        let t: Vec<f64> = (0..n).map(|i| i as f64 * 1e-3).collect();
        let x: Vec<f64> = (0..n).map(|i| (i % 977) as f64).collect();
        w.start_data_block(&g).unwrap();
        w.write_columns(&g, &[ColumnData::F64(&t), ColumnData::F64(&x), ColumnData::F64(&x)]).unwrap();
        w.finish_data_block(&g).unwrap();
        w.finalize().unwrap();
    }

    /// Whole channels and windows over a URL give the same signals batched
    /// and streamed (the path taken above the memory limit), on a row-oriented file split
    /// into several data blocks and on its column-oriented conversion.
    #[test]
    fn streamed_and_batched_whole_reads_agree() {
        let dir = tempfile::tempdir().unwrap();
        let (row, col) = (dir.path().join("row.mf4"), dir.path().join("col.mf4"));
        build(&row);
        let opts = crate::ColumnOptions { block_size: 256 * 1024, ..Default::default() };
        crate::to_column_oriented(&row, &col, &opts).unwrap();
        for path in [&row, &col] {
            let mdf = Mdf::from_file(path.to_str().unwrap()).unwrap();
            let index = MdfIndex::from_file(path.to_str().unwrap()).unwrap();
            assert!(index.channel_groups.iter().any(|g| g.data_blocks.len() > 1), "several data blocks");
            let url = serve(std::fs::read(path).unwrap());
            let names = ["A", "t", "B", "A"];
            let targets = index.locate_all(&names, None).unwrap();
            let option_sets = [
                ReadOptions::new(),
                ReadOptions::new().raw(true),
                ReadOptions::new().time(Some(12.5), Some(250.0)),
                ReadOptions::new().time(Some(100.0), None).raw(true),
                ReadOptions::new().records(1000, 150_000),
            ];
            for options in &option_sets {
                let want = mdf.read_many(&names, options).unwrap();
                let window = options.window().unwrap();
                for limit in [0, u64::MAX] {
                    let got = index.read_many_url_limited(&url, &targets, window, options.raw, limit).unwrap();
                    for (g, w) in got.iter().zip(&want) {
                        assert_eq!((&g.values, &g.timestamps), (&w.values, &w.timestamps), "limit {limit} {options:?}");
                    }
                }
            }
        }
    }

    /// A probing reader records every missing data section in one pass.
    #[test]
    fn probing_whole_reads_record_every_section() {
        let blocks: Vec<DataBlockInfo> = [0u64, 10_000, 20_000]
            .iter()
            .map(|&o| DataBlockInfo { file_offset: o, size: 24 + 800, is_compressed: false })
            .collect();
        let store = FragmentStore::new();
        let mut reader = RecordingRangeReader { store: &store, misses: Vec::new(), whole_request: true };
        let result = MdfIndex::for_each_data_section(&mut reader, &blocks, |_| Ok(()));
        assert!(result.is_err());
        assert_eq!(reader.misses, vec![(0, 824), (10_000, 824), (20_000, 824)]);
    }
}
