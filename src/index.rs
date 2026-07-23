//! MDF File Indexing System
//!
//! This module provides functionality to create lightweight indexes of MDF files
//! that can be serialized to JSON and used later to read specific channel data
//! without parsing the entire file structure.

use serde::{Deserialize, Serialize};
use crate::api::mdf::MDF;
use crate::blocks::common::{DataType, BlockHeader, BlockParse};
use crate::blocks::conversion::{ConversionBlock, ConversionType};
use crate::error::MdfError;
use crate::parsing::decoder::{check_value_validity, decode_channel_value, decode_channel_value_with_validity, decode_f64_from_record, DecodedValue};
use crate::signal::{decoded_opt_to_f64, Signal};

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
    /// `true` if this is the group's master channel (usually time).
    pub fn is_master(&self) -> bool {
        self.channel_type == 2
    }

    /// `true` if this is a variable-length (VLSD) channel.
    pub fn is_vlsd(&self) -> bool {
        self.channel_type == 1 && self.vlsd_data_address.is_some()
    }

    /// Create a temporary `ChannelBlock` for use with the decoder functions.
    /// This should be called once and reused across all records.
    fn to_channel_block(&self) -> crate::blocks::channel_block::ChannelBlock {
        Self::build_channel_block(
            self.channel_type,
            self.data_type.clone(),
            self.bit_offset,
            self.byte_offset,
            self.bit_count,
            self.flags,
            self.pos_invalidation_bit,
            self.name.clone(),
            self.conversion.clone(),
        )
    }

    /// Create a lightweight `ChannelBlock` for decode-only use (f64 fast path).
    /// Skips cloning the name and conversion since the decoder doesn't use them.
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
/// The source is *not* serialized with the index — after [`MdfIndex::load_from_file`]
/// re-attach one with [`MdfIndex::set_file`] / [`MdfIndex::set_url`]. Building an
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
    /// `from_url`, re-attachable after load via `set_file` / `set_url`. Never
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
            .map_err(|e| MdfError::IOError(e))?;
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
            .map_err(|e| MdfError::IOError(e))?;

        let mut buffer = vec![0u8; length as usize];
        self.file.read_exact(&mut buffer)
            .map_err(|e| MdfError::IOError(e))?;

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
}

#[cfg(feature = "http")]
impl HttpRangeReader {
    pub fn new(url: impl Into<String>) -> Result<Self, MdfError> {
        // ureq's native-tls feature is not auto-wired (unlike rustls): the
        // connector must be constructed and attached explicitly, otherwise
        // HTTPS requests fail with "no TLS backend is configured".
        let connector = native_tls::TlsConnector::new().map_err(|e| {
            MdfError::BlockSerializationError(format!("failed to init TLS backend: {e}"))
        })?;
        let agent = ureq::AgentBuilder::new()
            .tls_connector(std::sync::Arc::new(connector))
            .build();
        Ok(Self {
            agent,
            url: url.into(),
            request_count: 0,
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

        let resp = self
            .agent
            .get(&self.url)
            .set("Range", "bytes=0-0")
            .call()
            .map_err(|e| MdfError::BlockSerializationError(format!("HTTP GET error: {e}")))?;
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
        use std::io::Read;

        if length == 0 {
            return Ok(Vec::new());
        }
        let range_header = format!("bytes={}-{}", offset, offset + length - 1);
        let resp = self
            .agent
            .get(&self.url)
            .set("Range", &range_header)
            .call()
            .map_err(|e| MdfError::BlockSerializationError(format!("HTTP GET error: {e}")))?;
        self.request_count += 1;

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
                    return Err(MdfError::BlockSerializationError(format!(
                        "HTTP server ignored Range request (status 200) for offset {}; \
                         refusing to return bytes from the wrong position",
                        offset
                    )));
                }
            }
            other => {
                return Err(MdfError::BlockSerializationError(format!(
                    "HTTP range request returned unexpected status {other}"
                )));
            }
        }

        // Read at most `length` bytes (a 200 response carries the whole
        // resource and must be truncated), then insist we actually received
        // all of them: a short body must be an error, not silently fewer
        // records. Capacity is capped so a forged huge length cannot abort
        // on allocation.
        let mut buf = Vec::with_capacity(length.min(1 << 20) as usize);
        resp.into_reader()
            .take(length)
            .read_to_end(&mut buf)
            .map_err(MdfError::IOError)?;
        if (buf.len() as u64) < length {
            // Report absolute positions so wrappers (e.g. CachingRangeReader)
            // can learn the resource's true end from `actual`.
            return Err(MdfError::TooShortBuffer {
                actual: (offset + buf.len() as u64) as usize,
                expected: (offset + length) as usize,
                file: file!(),
                line: line!(),
            });
        }
        Ok(buf)
    }
}

impl MdfIndex {
    /// Create an index from an MDF file on disk.
    ///
    /// Not available on `wasm32-unknown-unknown`; use [`from_bytes`] instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(file_path: &str) -> Result<Self, MdfError> {
        let mdf = MDF::from_file(file_path)?;
        let file_size = std::fs::metadata(file_path)
            .map_err(|e| MdfError::IOError(e))?
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
        let mut index = Self::from_range_reader(&mut cached, file_size)?;
        index.source = Some(Source::Url(url.to_string()));
        Ok(index)
    }

    /// Shared index-building logic operating on an already-parsed [`MDF`].
    fn build_index(mdf: MDF, file_size: u64) -> Result<Self, MdfError> {
        let start_time_ns = mdf.start_time_ns();
        let mut indexed_groups = Vec::new();

        for group in mdf.channel_groups() {
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
            });
        }

        Ok(MdfIndex { file_size, start_time_ns, channel_groups: indexed_groups, source: None })
    }

    /// Extract data block information from a channel group
    fn extract_data_blocks(group: &crate::api::channel_group::ChannelGroup) -> Result<Vec<DataBlockInfo>, MdfError> {
        let mut data_blocks = Vec::new();
        let raw_data_group = group.raw_data_group();
        let mmap = group.mmap();
        
        // Start at the group's primary data pointer
        let mut current_block_address = raw_data_group.block.data_block_addr;
        while current_block_address != 0 {
            let byte_offset = current_block_address as usize;

            // Read the block header
            let block_header = crate::blocks::common::BlockHeader::from_bytes(&mmap[byte_offset..byte_offset + 24])?;

            match block_header.id.as_str() {
                "##DT" | "##DV" => {
                    // Single contiguous DataBlock
                    let data_block_info = DataBlockInfo {
                        file_offset: current_block_address,
                        size: block_header.block_len,
                        is_compressed: false,
                    };
                    data_blocks.push(data_block_info);
                    // No list to follow, we're done
                    current_block_address = 0;
                }
                "##DZ" => {
                    // Compressed data block  
                    let data_block_info = DataBlockInfo {
                        file_offset: current_block_address,
                        size: block_header.block_len,
                        is_compressed: true,
                    };
                    data_blocks.push(data_block_info);
                    current_block_address = 0;
                }
                "##DL" => {
                    // Fragmented list of data blocks
                    let data_list_block = crate::blocks::data_list_block::DataListBlock::from_bytes(&mmap[byte_offset..])?;

                    // Parse each fragment in this list
                    for &fragment_address in &data_list_block.data_links {
                        let fragment_offset = fragment_address as usize;
                        let fragment_header = crate::blocks::common::BlockHeader::from_bytes(&mmap[fragment_offset..fragment_offset + 24])?;
                        
                        let is_compressed = fragment_header.id == "##DZ";
                        let data_block_info = DataBlockInfo {
                            file_offset: fragment_address,
                            size: fragment_header.block_len,
                            is_compressed,
                        };
                        data_blocks.push(data_block_info);
                    }

                    // Move to the next DLBLOCK in the chain (0 = end)
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

    /// Create an index from an in-memory MDF byte buffer.
    ///
    /// This is the primary constructor on `wasm32-unknown-unknown`.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, MdfError> {
        let file_size = data.len() as u64;
        let mdf = MDF::from_bytes(data)?;
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

            let data_blocks =
                Self::extract_data_blocks_via_reader(reader, group.data_block_addr)?;

            indexed_groups.push(IndexedChannelGroup {
                name: group.cg_name,
                comment: group.cg_comment,
                record_id_len: group.record_id_len,
                record_size: group.cg.samples_byte_nr,
                invalidation_bytes: group.cg.invalidation_bytes_nr,
                record_count: group.cg.cycles_nr,
                channels: indexed_channels,
                data_blocks,
            });
        }

        Ok(MdfIndex {
            file_size,
            start_time_ns,
            channel_groups: indexed_groups,
            source: None,
        })
    }

    /// Mirror of [`Self::extract_data_blocks`] that fetches headers via a
    /// [`ByteRangeReader`] instead of slicing into a memory map.
    fn extract_data_blocks_via_reader<R>(
        reader: &mut R,
        data_block_addr: u64,
    ) -> Result<Vec<DataBlockInfo>, MdfError>
    where
        R: ByteRangeReader<Error = MdfError>,
    {
        let mut data_blocks = Vec::new();
        let mut current_block_address = data_block_addr;

        while current_block_address != 0 {
            let header_bytes = reader.read_range(current_block_address, 24)?;
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
                    let dl_bytes =
                        reader.read_range(current_block_address, block_header.block_len)?;
                    let data_list_block =
                        crate::blocks::data_list_block::DataListBlock::from_bytes(&dl_bytes)?;

                    for &fragment_address in &data_list_block.data_links {
                        let frag_header_bytes = reader.read_range(fragment_address, 24)?;
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
    fn extract_vlsd_data_blocks<R>(
        reader: &mut R,
        data_addr: u64,
    ) -> Result<Vec<DataBlockInfo>, MdfError>
    where
        R: ByteRangeReader<Error = MdfError>,
    {
        let mut data_blocks: Vec<DataBlockInfo> = Vec::new();
        let mut visited: std::collections::HashSet<u64> = std::collections::HashSet::new();
        // Running sum of fragment data-section sizes (size - 24), i.e. the
        // virtual start offset of the next fragment in the concatenated stream.
        let mut running_offset: u64 = 0;
        let mut current = data_addr;

        while current != 0 {
            let header_bytes = reader.read_range(current, 24)?;
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
                    let dl_bytes = reader.read_range(current, header.block_len)?;
                    let dl = crate::blocks::data_list_block::DataListBlock::from_bytes(&dl_bytes)?;

                    for (i, &fragment_address) in dl.data_links.iter().enumerate() {
                        if fragment_address == 0 {
                            continue;
                        }
                        let frag_header_bytes = reader.read_range(fragment_address, 24)?;
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
                        if let Some(offsets) = &dl.offsets {
                            if let Some(&declared) = offsets.get(i) {
                                if declared != running_offset {
                                    return Err(MdfError::BlockSerializationError(format!(
                                        "non-contiguous VLSD ##SD chain is unsupported: \
                                         fragment at offset {} declares virtual start {} \
                                         but the running data-section sum is {}",
                                        fragment_address, declared, running_offset
                                    )));
                                }
                            }
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
    /// Not available on `wasm32-unknown-unknown`; use [`to_json`] instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save_to_file(&self, index_path: &str) -> Result<(), MdfError> {
        let json = self.to_json()?;
        std::fs::write(index_path, json)
            .map_err(|e| MdfError::IOError(e))?;
        Ok(())
    }

    /// Load an index from a JSON file.
    ///
    /// Not available on `wasm32-unknown-unknown`; use [`from_json`] instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_from_file(index_path: &str) -> Result<Self, MdfError> {
        let json = std::fs::read_to_string(index_path)
            .map_err(|e| MdfError::IOError(e))?;
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
    /// Called from [`Self::from_json`] / [`Self::load_from_file`] so that
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
        reader: &mut R
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        let conversion = Self::resolve_conversion(channel, reader)?;

        if channel.channel_type == 1 {
            return self.read_vlsd_channel_values(group, channel, conversion.as_ref(), reader);
        }

        // For regular channels, read from data blocks
        self.read_regular_channel_values(group, channel, conversion.as_ref(), reader)
    }

    /// Resolve a channel's conversion for reading.
    ///
    /// Prefers a conversion already resolved into the index (local-file /
    /// in-memory builds, and older serialized indexes). Otherwise — the remote
    /// / range-reader build path, which records only
    /// [`IndexedChannel::conversion_addr`] — it fetches the `##CC` block and
    /// resolves its dependency chain through `reader`, exactly when the caller
    /// asks to read values. Returns `None` when the channel has no conversion.
    fn resolve_conversion<R: ByteRangeReader<Error = MdfError>>(
        channel: &IndexedChannel,
        reader: &mut R,
    ) -> Result<Option<ConversionBlock>, MdfError> {
        if let Some(conv) = &channel.conversion {
            return Ok(Some(conv.clone()));
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
        Ok(Some(cc))
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
        Self::resolve_conversion(channel, reader)
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
        let temp_cb = channel.to_channel_block();

        for data_block in &group.data_blocks {
            let block_data = reader
                .read_range(data_block.file_offset + 24, Self::data_section_size(data_block)?)?;
            Self::decode_records_to_values(&block_data, record_size, group, conversion, &temp_cb, &mut values)?;
        }

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
        let validity_cb = channel.to_channel_block();
        let mut payload_cb = channel.to_channel_block();
        payload_cb.data = 1;

        let mut total_records: usize = 0;
        for data_block in &group.data_blocks {
            total_records += Self::records_in_block(data_block, record_size)? as usize;
        }
        let mut values = Vec::with_capacity(total_records.min(1 << 24));

        for data_block in &group.data_blocks {
            let block_data = reader
                .read_range(data_block.file_offset + 24, Self::data_section_size(data_block)?)?;
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
        }

        Ok(values)
    }

    /// Map VLSD decoded values to `f64`, matching `Channel::values_as_f64`:
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
        if block_data.len() % record_size != 0 {
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
        if block_data.len() % record_size != 0 {
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
            values.extend(std::iter::repeat(f64::NAN).take(record_count));
            return Ok(());
        }

        if !has_invalidation && !has_conversion {
            // Fastest path: no invalidation, no conversion - just decode f64 directly
            for i in 0..record_count {
                let record = &block_data[i * record_size..(i + 1) * record_size];
                values.push(decode_f64_from_record(record, record_id_len, temp_cb));
            }
        } else if !has_invalidation && linear_coeffs.is_some() {
            // Fast path: no invalidation, linear conversion
            let (a, b) = linear_coeffs.unwrap();
            for i in 0..record_count {
                let record = &block_data[i * record_size..(i + 1) * record_size];
                let raw = decode_f64_from_record(record, record_id_len, temp_cb);
                values.push(a + b * raw);
            }
        } else if !has_invalidation {
            // No invalidation but non-linear conversion - need full decode for conversion
            for i in 0..record_count {
                let record = &block_data[i * record_size..(i + 1) * record_size];
                let raw = decode_f64_from_record(record, record_id_len, temp_cb);
                if let Some(coeffs) = linear_coeffs {
                    values.push(coeffs.0 + coeffs.1 * raw);
                } else if has_conversion {
                    // Need to decode via DecodedValue for complex conversions
                    if let Some(decoded) = decode_channel_value_with_validity(
                        record, record_id_len, cg_data_bytes, temp_cb,
                    ) {
                        match conversion.unwrap().apply_decoded(decoded.value, &[])? {
                            DecodedValue::Float(v) => values.push(v),
                            DecodedValue::UnsignedInteger(v) => values.push(v as f64),
                            DecodedValue::SignedInteger(v) => values.push(v as f64),
                            _ => values.push(f64::NAN),
                        }
                    } else {
                        values.push(f64::NAN);
                    }
                } else {
                    values.push(raw);
                }
            }
        } else {
            // Has invalidation bytes - must check validity
            for i in 0..record_count {
                let record = &block_data[i * record_size..(i + 1) * record_size];
                let is_valid = check_value_validity(record, record_id_len, cg_data_bytes, temp_cb);
                if is_valid {
                    let raw = decode_f64_from_record(record, record_id_len, temp_cb);
                    if let Some((a, b)) = linear_coeffs {
                        values.push(a + b * raw);
                    } else if has_conversion {
                        if let Some(decoded) = decode_channel_value_with_validity(
                            record, record_id_len, cg_data_bytes, temp_cb,
                        ) {
                            match conversion.unwrap().apply_decoded(decoded.value, &[])? {
                                DecodedValue::Float(v) => values.push(v),
                                DecodedValue::UnsignedInteger(v) => values.push(v as f64),
                                DecodedValue::SignedInteger(v) => values.push(v as f64),
                                _ => values.push(f64::NAN),
                            }
                        } else {
                            values.push(f64::NAN);
                        }
                    } else {
                        values.push(raw);
                    }
                } else {
                    values.push(f64::NAN);
                }
            }
        }
        Ok(())
    }

    /// All channel groups in the file, in file order.
    pub fn groups(&self) -> &[IndexedChannelGroup] {
        &self.channel_groups
    }

    /// Find a channel group by its name.
    ///
    /// Returns the first group whose `##CG` acquisition name matches `name`.
    pub fn group(&self, name: &str) -> Option<&IndexedChannelGroup> {
        self.channel_groups
            .iter()
            .find(|g| g.name.as_deref() == Some(name))
    }

    /// Look up a single channel by name across all groups (first match).
    pub fn channel(&self, name: &str) -> Option<&IndexedChannel> {
        let (g, c) = self.locate(name)?;
        self.channel_groups.get(g)?.channels.get(c)
    }

    /// Look up a channel by group name + channel name.
    pub fn channel_in(&self, group: &str, name: &str) -> Option<&IndexedChannel> {
        self.group(group)?.channel(name)
    }

    /// The attached data source rendered as a string (file path or URL).
    pub fn source_string(&self) -> Option<String> {
        match &self.source {
            Some(Source::File(p)) => Some(p.clone()),
            #[cfg(feature = "http")]
            Some(Source::Url(u)) => Some(u.clone()),
            None => None,
        }
    }

    /// A flat catalog of every channel as `(source, group name, channel name)`.
    ///
    /// `source` is this index's attached source (the same value for every row,
    /// handy when concatenating catalogs from many files/URLs). Built purely
    /// from metadata — no sample data is read.
    pub fn signal_list(&self) -> Vec<(Option<String>, Option<String>, Option<String>)> {
        let src = self.source_string();
        let mut out = Vec::new();
        for group in &self.channel_groups {
            for channel in &group.channels {
                out.push((src.clone(), group.name.clone(), channel.name.clone()));
            }
        }
        out
    }

    /// Every channel name across all groups, in file order (duplicates kept).
    pub fn channel_names(&self) -> Vec<&str> {
        self.channel_groups
            .iter()
            .flat_map(|g| g.channels.iter())
            .filter_map(|c| c.name.as_deref())
            .collect()
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
    pub(crate) fn locate_in(&self, group: &str, name: &str) -> Option<(usize, usize)> {
        let g = self
            .channel_groups
            .iter()
            .position(|grp| grp.name.as_deref() == Some(group))?;
        let c = self.channel_groups[g]
            .channels
            .iter()
            .position(|ch| ch.name.as_deref() == Some(name))?;
        Some((g, c))
    }

    /// All `(group_index, channel_index)` positions matching a channel name.
    ///
    /// Useful when the same name appears in several groups (e.g. a per-group
    /// `Time` master channel).
    pub fn find_channels(&self, name: &str) -> Vec<(usize, usize)> {
        let mut matches = Vec::new();
        for (g, group) in self.channel_groups.iter().enumerate() {
            for (c, channel) in group.channels.iter().enumerate() {
                if channel.name.as_deref() == Some(name) {
                    matches.push((g, c));
                }
            }
        }
        matches
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
        Ok(self.open(MmapRangeReader::new(path)?))
    }

    /// The data source attached to this index, if any.
    pub fn source(&self) -> Option<&Source> {
        self.source.as_ref()
    }

    /// Attach (or replace) the data source used by [`MdfIndex::read`].
    pub fn set_source(&mut self, source: Source) {
        self.source = Some(source);
    }

    /// Attach a local file path as the data source for lazy reads.
    pub fn set_file(&mut self, path: impl Into<String>) {
        self.source = Some(Source::File(path.into()));
    }

    /// Attach an HTTP/S3 URL as the data source for lazy reads.
    #[cfg(feature = "http")]
    pub fn set_url(&mut self, url: impl Into<String>) {
        self.source = Some(Source::Url(url.into()));
    }

    /// Read a channel by name as a [`Signal`] using the attached [`Source`].
    ///
    /// Values are paired with the channel's group master (time) axis. This is
    /// the lazy read path: the byte-range request happens now, not at index
    /// build time. Errors if no source is attached (see [`MdfIndex::set_file`] /
    /// [`MdfIndex::set_url`]).
    pub fn read(&self, name: &str) -> Result<Signal, MdfError> {
        let (g, c) = self.locate(name).ok_or_else(|| {
            MdfError::BlockSerializationError(format!("Channel '{}' not found", name))
        })?;
        self.read_signal(g, c)
    }

    /// [`MdfIndex::read`] addressed by group name + channel name.
    pub fn read_in(&self, group: &str, name: &str) -> Result<Signal, MdfError> {
        let (g, c) = self.locate_in(group, name).ok_or_else(|| {
            MdfError::BlockSerializationError(format!(
                "Channel '{}' not found in group '{}'",
                name, group
            ))
        })?;
        self.read_signal(g, c)
    }

    /// Decode a channel + its group master from the attached source.
    fn read_signal(&self, g: usize, c: usize) -> Result<Signal, MdfError> {
        let (name, unit, master) = {
            let channel = &self.channel_groups[g].channels[c];
            let master = self.channel_groups[g]
                .channels
                .iter()
                .position(|ch| ch.is_master())
                .filter(|&m| m != c);
            (channel.name.clone().unwrap_or_default(), channel.unit.clone(), master)
        };

        let values = self.read_values_via_source(g, c)?;
        let timestamps = match master {
            Some(m) => self.read_values_f64_via_source(g, m)?,
            None => Vec::new(),
        };

        Ok(Signal { name, unit, timestamps, values })
    }

    /// Resolve the attached [`Source`], erroring with a helpful message if none.
    fn require_source(&self) -> Result<&Source, MdfError> {
        self.source.as_ref().ok_or_else(|| {
            MdfError::BlockSerializationError(
                "no data source attached to index; build with from_file/from_url or call set_file/set_url".to_string(),
            )
        })
    }

    /// Read one channel's decoded values lazily through the attached source.
    pub(crate) fn read_values_via_source(
        &self,
        g: usize,
        c: usize,
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        match self.require_source()? {
            #[cfg(not(target_arch = "wasm32"))]
            Source::File(path) => {
                let file = std::fs::File::open(path).map_err(MdfError::IOError)?;
                let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(MdfError::IOError)?;
                self.read_channel_values_from_slice(g, c, &mmap)
            }
            #[cfg(target_arch = "wasm32")]
            Source::File(_) => Err(MdfError::BlockSerializationError(
                "file sources are not available on wasm32".to_string(),
            )),
            #[cfg(feature = "http")]
            Source::Url(url) => {
                let http = HttpRangeReader::new(url)?;
                let mut cached = CachingRangeReader::new(http);
                cached.set_bypass(true);
                self.read_channel_values(g, c, &mut cached)
            }
        }
    }

    /// Resolve a channel's conversion, fetching it through the attached source
    /// if it was not resolved at index-build time (the remote / range-reader
    /// path records only its location — see [`IndexedChannel::conversion_addr`]).
    ///
    /// Returns `None` when the channel has no conversion. Errors if resolution
    /// needs the source (a deferred conversion) but none is attached — attach
    /// one with [`set_file`](Self::set_file) / [`set_url`](Self::set_url).
    pub fn conversion(&self, name: &str) -> Result<Option<ConversionBlock>, MdfError> {
        let (g, c) = self.locate(name).ok_or_else(|| {
            MdfError::BlockSerializationError(format!("Channel '{}' not found", name))
        })?;
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
                let file = std::fs::File::open(path).map_err(MdfError::IOError)?;
                let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(MdfError::IOError)?;
                let mut slice_reader = BorrowedSliceReader { data: &mmap };
                Self::resolve_conversion(channel, &mut slice_reader)
            }
            #[cfg(target_arch = "wasm32")]
            Source::File(_) => Err(MdfError::BlockSerializationError(
                "file sources are not available on wasm32".to_string(),
            )),
            #[cfg(feature = "http")]
            Source::Url(url) => {
                let http = HttpRangeReader::new(url)?;
                let mut cached = CachingRangeReader::new(http);
                Self::resolve_conversion(channel, &mut cached)
            }
        }
    }

    /// Read one channel's values as `f64` lazily through the attached source.
    pub(crate) fn read_values_f64_via_source(
        &self,
        g: usize,
        c: usize,
    ) -> Result<Vec<f64>, MdfError> {
        match self.require_source()? {
            #[cfg(not(target_arch = "wasm32"))]
            Source::File(path) => {
                let file = std::fs::File::open(path).map_err(MdfError::IOError)?;
                let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(MdfError::IOError)?;
                self.read_channel_values_from_slice_as_f64(g, c, &mmap)
            }
            #[cfg(target_arch = "wasm32")]
            Source::File(_) => Err(MdfError::BlockSerializationError(
                "file sources are not available on wasm32".to_string(),
            )),
            #[cfg(feature = "http")]
            Source::Url(url) => {
                let http = HttpRangeReader::new(url)?;
                let mut cached = CachingRangeReader::new(http);
                cached.set_bypass(true);
                self.read_channel_values_as_f64(g, c, &mut cached)
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
            ((channel.bit_offset as usize + channel.bit_count as usize + 7) / 8).max(1)
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
    pub fn byte_ranges(&self, name: &str) -> Result<Vec<(u64, u64)>, MdfError> {
        let (g, c) = self.locate(name).ok_or_else(|| {
            MdfError::BlockSerializationError(format!("Channel '{}' not found", name))
        })?;
        self.get_channel_byte_ranges(g, c)
    }

    /// Byte ranges for a channel addressed by group name + channel name.
    pub fn byte_ranges_in(&self, group: &str, name: &str) -> Result<Vec<(u64, u64)>, MdfError> {
        let (g, c) = self.locate_in(group, name).ok_or_else(|| {
            MdfError::BlockSerializationError(format!(
                "Channel '{}' not found in group '{}'",
                name, group
            ))
        })?;
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
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        let (g, c) = self.locate(name).ok_or_else(|| {
            MdfError::BlockSerializationError(format!("Channel '{}' not found", name))
        })?;
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
        reader: &mut R,
    ) -> Result<Vec<f64>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        let conversion = Self::resolve_conversion(channel, reader)?;

        if channel.channel_type == 1 {
            let vals = self.read_vlsd_channel_values(group, channel, conversion.as_ref(), reader)?;
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
        let linear_coeffs = Self::get_linear_coeffs(conversion.as_ref());
        let has_conversion = conversion.is_some();

        for data_block in &group.data_blocks {
            let block_data = reader
                .read_range(data_block.file_offset + 24, Self::data_section_size(data_block)?)?;
            Self::decode_records_to_f64(&block_data, record_size, group, channel, conversion.as_ref(), &temp_cb, linear_coeffs, has_conversion, &mut values)?;
        }

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
        file_data: &[u8],
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        let mut slice_reader = BorrowedSliceReader { data: file_data };
        let conversion = Self::resolve_conversion(channel, &mut slice_reader)?;

        if channel.channel_type == 1 {
            return self.read_vlsd_channel_values(group, channel, conversion.as_ref(), &mut slice_reader);
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
        let temp_cb = channel.to_channel_block();

        for data_block in &group.data_blocks {
            let block_data = Self::slice_data_block(file_data, data_block)?;
            Self::decode_records_to_values(block_data, record_size, group, conversion.as_ref(), &temp_cb, &mut values)?;
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
        file_data: &[u8],
    ) -> Result<Vec<f64>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        let mut slice_reader = BorrowedSliceReader { data: file_data };
        let conversion = Self::resolve_conversion(channel, &mut slice_reader)?;

        if channel.channel_type == 1 {
            let vals = self.read_vlsd_channel_values(group, channel, conversion.as_ref(), &mut slice_reader)?;
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
        let linear_coeffs = Self::get_linear_coeffs(conversion.as_ref());
        let has_conversion = conversion.is_some();

        for data_block in &group.data_blocks {
            let block_data = Self::slice_data_block(file_data, data_block)?;
            Self::decode_records_to_f64(block_data, record_size, group, channel, conversion.as_ref(), &temp_cb, linear_coeffs, has_conversion, &mut values)?;
        }

        Ok(values)
    }

    /// Slice a data block from file_data, skipping the 24-byte block header.
    /// Uses checked arithmetic so a forged offset/size errors instead of
    /// wrapping or panicking.
    #[allow(dead_code)] // used by the Python bindings (pyo3 feature)
    fn slice_data_block<'a>(file_data: &'a [u8], data_block: &DataBlockInfo) -> Result<&'a [u8], MdfError> {
        let data_len = Self::data_section_size(data_block)?;
        let bounds = data_block
            .file_offset
            .checked_add(24)
            .and_then(|start| start.checked_add(data_len).map(|end| (start, end)))
            .and_then(|(start, end)| {
                Some((usize::try_from(start).ok()?, usize::try_from(end).ok()?))
            });
        let (data_start, data_end) = match bounds {
            Some(b) => b,
            None => {
                return Err(MdfError::TooShortBuffer {
                    actual: file_data.len(),
                    expected: usize::MAX,
                    file: file!(),
                    line: line!(),
                });
            }
        };
        if data_end > file_data.len() {
            return Err(MdfError::TooShortBuffer {
                actual: file_data.len(),
                expected: data_end,
                file: file!(),
                line: line!(),
            });
        }
        Ok(&file_data[data_start..data_end])
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

    fn locate(&self, name: &str) -> Result<(usize, usize), MdfError> {
        self.index.locate(name).ok_or_else(|| {
            MdfError::BlockSerializationError(format!("Channel '{}' not found", name))
        })
    }

    fn locate_in(&self, group: &str, name: &str) -> Result<(usize, usize), MdfError> {
        self.index.locate_in(group, name).ok_or_else(|| {
            MdfError::BlockSerializationError(format!(
                "Channel '{}' not found in group '{}'",
                name, group
            ))
        })
    }

    /// Read all samples of a channel by name (first match across groups).
    ///
    /// Conversions stored in the index are applied; invalid samples are `None`.
    pub fn values(&mut self, name: &str) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        let (g, c) = self.locate(name)?;
        self.index.read_channel_values(g, c, &mut self.reader)
    }

    /// Read all samples of a channel, addressed by group name + channel name.
    pub fn values_in(
        &mut self,
        group: &str,
        name: &str,
    ) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        let (g, c) = self.locate_in(group, name)?;
        self.index.read_channel_values(g, c, &mut self.reader)
    }

    /// Fast path: read a numeric channel by name as `Vec<f64>`.
    ///
    /// Invalid / non-numeric samples are `f64::NAN`. Conversions that reduce to
    /// a linear scale are applied inline.
    pub fn values_f64(&mut self, name: &str) -> Result<Vec<f64>, MdfError> {
        let (g, c) = self.locate(name)?;
        self.index.read_channel_values_as_f64(g, c, &mut self.reader)
    }

    /// Fast `f64` path addressed by group name + channel name.
    pub fn values_f64_in(&mut self, group: &str, name: &str) -> Result<Vec<f64>, MdfError> {
        let (g, c) = self.locate_in(group, name)?;
        self.index.read_channel_values_as_f64(g, c, &mut self.reader)
    }

    /// Read a channel by name as a [`Signal`] (values paired with the group's
    /// master/time axis), using this reader's bound source.
    pub fn signal(&mut self, name: &str) -> Result<Signal, MdfError> {
        let (g, c) = self.locate(name)?;
        self.read_signal(g, c)
    }

    /// [`MdfReader::signal`] addressed by group name + channel name.
    pub fn signal_in(&mut self, group: &str, name: &str) -> Result<Signal, MdfError> {
        let (g, c) = self.locate_in(group, name)?;
        self.read_signal(g, c)
    }

    fn read_signal(&mut self, g: usize, c: usize) -> Result<Signal, MdfError> {
        let (name, unit, master) = {
            let group = &self.index.channel_groups[g];
            let channel = &group.channels[c];
            let master = group
                .channels
                .iter()
                .position(|ch| ch.is_master())
                .filter(|&m| m != c);
            (channel.name.clone().unwrap_or_default(), channel.unit.clone(), master)
        };

        let values = self.index.read_channel_values(g, c, &mut self.reader)?;
        let timestamps = match master {
            Some(m) => self
                .index
                .read_channel_values(g, m, &mut self.reader)?
                .iter()
                .map(decoded_opt_to_f64)
                .collect(),
            None => Vec::new(),
        };
        Ok(Signal { name, unit, timestamps, values })
    }
}
