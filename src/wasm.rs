//! WebAssembly (wasm-bindgen) bindings for mf4-rs.
//!
//! This module mirrors the name-based API philosophy of the Python bindings
//! (`src/python.rs`): navigation is by group/channel **name**, with an optional
//! `group` argument to disambiguate duplicate channel names. It is compiled only
//! when the `wasm` feature is enabled and is intended for the
//! `wasm32-unknown-unknown` target published as an npm package, though it also
//! compiles on native targets for IDE friendliness.
//!
//! Exposed classes: [`Mdf`](WasmMdf) (in-memory reader), [`MdfIndex`](WasmMdfIndex)
//! (self-contained index + fragment-based reads), and [`MdfWriter`](WasmMdfWriter)
//! (in-memory writer producing a `Uint8Array`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Cursor, Seek, Write};
use std::rc::Rc;

use js_sys::{Array, Float64Array, Object, Reflect, Uint8Array};
use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, JsError};

use crate::api::mdf::MDF;
use crate::blocks::common::DataType;
use crate::error::MdfError;
use crate::index::{ByteRangeReader, MdfIndex};
use crate::parsing::decoder::DecodedValue;
use crate::signal::Signal;
use crate::writer::MdfWriter;

/// Largest integer exactly representable as a JS `number` (2^53 - 1).
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn err_to_js(e: MdfError) -> JsError {
    JsError::new(&e.to_string())
}

/// Symbolic MDF data-type name (matches the Python `DataType.name`).
fn data_type_name(dt: &DataType) -> &'static str {
    match dt {
        DataType::UnsignedIntegerLE => "UnsignedIntegerLE",
        DataType::UnsignedIntegerBE => "UnsignedIntegerBE",
        DataType::SignedIntegerLE => "SignedIntegerLE",
        DataType::SignedIntegerBE => "SignedIntegerBE",
        DataType::FloatLE => "FloatLE",
        DataType::FloatBE => "FloatBE",
        DataType::StringLatin1 => "StringLatin1",
        DataType::StringUtf8 => "StringUtf8",
        DataType::StringUtf16LE => "StringUtf16LE",
        DataType::StringUtf16BE => "StringUtf16BE",
        DataType::ByteArray => "ByteArray",
        DataType::MimeSample => "MimeSample",
        DataType::MimeStream => "MimeStream",
        DataType::CanOpenDate => "CanOpenDate",
        DataType::CanOpenTime => "CanOpenTime",
        DataType::ComplexLE => "ComplexLE",
        DataType::ComplexBE => "ComplexBE",
        DataType::Unknown(_) => "Unknown",
    }
}

/// Parse a symbolic data-type name (used by the writer's generic `addChannel`).
fn data_type_from_str(name: &str) -> Result<DataType, JsError> {
    Ok(match name {
        "UnsignedIntegerLE" | "u" | "uint" => DataType::UnsignedIntegerLE,
        "UnsignedIntegerBE" => DataType::UnsignedIntegerBE,
        "SignedIntegerLE" | "i" | "int" => DataType::SignedIntegerLE,
        "SignedIntegerBE" => DataType::SignedIntegerBE,
        "FloatLE" | "f" | "float" => DataType::FloatLE,
        "FloatBE" => DataType::FloatBE,
        "StringLatin1" => DataType::StringLatin1,
        "StringUtf8" | "string" => DataType::StringUtf8,
        "StringUtf16LE" => DataType::StringUtf16LE,
        "StringUtf16BE" => DataType::StringUtf16BE,
        "ByteArray" | "bytes" => DataType::ByteArray,
        other => {
            return Err(JsError::new(&format!(
                "Unknown data type '{}'. Expected one of: FloatLE, FloatBE, \
                 UnsignedIntegerLE, UnsignedIntegerBE, SignedIntegerLE, \
                 SignedIntegerBE, StringUtf8, StringLatin1, StringUtf16LE, \
                 StringUtf16BE, ByteArray",
                other
            )));
        }
    })
}

/// `true` for the text data types, which this writer stores as variable-length
/// (VLSD) channels rather than fixed-length record fields.
fn is_string_type(dt: &DataType) -> bool {
    matches!(
        dt,
        DataType::StringLatin1
            | DataType::StringUtf8
            | DataType::StringUtf16LE
            | DataType::StringUtf16BE
    )
}

/// Convert a single decoded value to a JS value, matching the Python
/// `decoded_value_to_pyobject` mapping (large integers become `BigInt`).
fn decoded_to_js(dv: &DecodedValue) -> JsValue {
    match dv {
        DecodedValue::Float(v) => JsValue::from_f64(*v),
        DecodedValue::UnsignedInteger(v) => {
            if *v <= MAX_SAFE_INTEGER {
                JsValue::from_f64(*v as f64)
            } else {
                JsValue::from(js_sys::BigInt::from(*v))
            }
        }
        DecodedValue::SignedInteger(v) => {
            if v.unsigned_abs() <= MAX_SAFE_INTEGER {
                JsValue::from_f64(*v as f64)
            } else {
                JsValue::from(js_sys::BigInt::from(*v))
            }
        }
        DecodedValue::String(s) => JsValue::from_str(s),
        DecodedValue::ByteArray(v) | DecodedValue::MimeSample(v) | DecodedValue::MimeStream(v) => {
            Uint8Array::from(&v[..]).into()
        }
        DecodedValue::Unknown => JsValue::NULL,
    }
}

/// Build a `{ name, unit, timestamps, values }` JS object from a [`Signal`].
fn signal_to_js(sig: Signal) -> Result<JsValue, JsError> {
    let obj = Object::new();
    let set = |k: &str, v: &JsValue| -> Result<(), JsError> {
        Reflect::set(&obj, &JsValue::from_str(k), v)
            .map(|_| ())
            .map_err(|_| JsError::new("failed to build signal object"))
    };

    set("name", &JsValue::from_str(&sig.name))?;
    match &sig.unit {
        Some(u) => set("unit", &JsValue::from_str(u))?,
        None => set("unit", &JsValue::NULL)?,
    }
    set("timestamps", &Float64Array::from(&sig.timestamps[..]).into())?;

    let values = Array::new();
    for v in &sig.values {
        let jv = match v {
            Some(dv) => decoded_to_js(dv),
            None => JsValue::NULL,
        };
        values.push(&jv);
    }
    set("values", &values.into())?;

    Ok(obj.into())
}

/// Convert byte ranges to a JS array of `[offset, length]` number pairs,
/// erroring if any value exceeds `Number.MAX_SAFE_INTEGER`.
fn ranges_to_js(ranges: Vec<(u64, u64)>) -> Result<JsValue, JsError> {
    for (offset, length) in &ranges {
        if *offset > MAX_SAFE_INTEGER || *length > MAX_SAFE_INTEGER {
            return Err(JsError::new(
                "byte range exceeds Number.MAX_SAFE_INTEGER (2^53-1); use a 64-bit aware reader",
            ));
        }
    }
    let pairs: Vec<[f64; 2]> = ranges
        .iter()
        .map(|(o, l)| [*o as f64, *l as f64])
        .collect();
    serde_wasm_bindgen::to_value(&pairs).map_err(|e| JsError::new(&e.to_string()))
}

/// Sort and merge overlapping/adjacent byte ranges into a minimal cover.
fn merge_ranges(mut ranges: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    if ranges.is_empty() {
        return ranges;
    }
    ranges.sort_by_key(|r| r.0);
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (offset, length) in ranges {
        let end = offset + length;
        if let Some(last) = out.last_mut() {
            let last_end = last.0 + last.1;
            if offset <= last_end {
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

/// A [`ByteRangeReader`] that serves absolute `(offset, length)` requests from a
/// caller-provided list of file fragments, each tagged with its absolute file
/// offset. A request is satisfied if the union of fragments fully covers it
/// (assembled across fragments when necessary); otherwise a clear error is
/// returned naming the first uncovered offset.
struct FragmentRangeReader {
    /// `(absolute_offset, bytes)` pairs.
    fragments: Vec<(u64, Vec<u8>)>,
}

/// Narrow a `u64` byte count/offset to `usize`, erroring instead of truncating
/// on a 32-bit (wasm) target where the value exceeds `usize::MAX`.
fn u64_to_usize(v: u64) -> Result<usize, MdfError> {
    usize::try_from(v).map_err(|_| {
        MdfError::BlockSerializationError(format!("byte value {} exceeds addressable range", v))
    })
}

/// End offset of a fragment, erroring on overflow instead of wrapping.
fn fragment_end(fstart: u64, len: usize) -> Result<u64, MdfError> {
    fstart.checked_add(len as u64).ok_or_else(|| {
        MdfError::BlockSerializationError("fragment offset + length overflows u64".to_string())
    })
}

/// Outcome of trying to serve a read from a set of file fragments.
enum Coverage {
    /// The request was fully covered; here are the assembled bytes.
    Full(Vec<u8>),
    /// A gap was found: `offset`/`length` describe the first still-missing
    /// sub-range of the request.
    Gap { offset: u64, length: u64 },
}

/// Try to assemble the absolute range `[offset, offset + length)` from
/// `fragments` (each an `(absolute_offset, bytes)` pair, possibly
/// overlapping). Returns [`Coverage::Full`] with the bytes, or
/// [`Coverage::Gap`] naming the first uncovered sub-range.
fn assemble_from_fragments(
    fragments: &[(u64, Vec<u8>)],
    offset: u64,
    length: u64,
) -> Result<Coverage, MdfError> {
    let req_end = offset.checked_add(length).ok_or_else(|| {
        MdfError::BlockSerializationError(
            "requested byte range offset + length overflows u64".to_string(),
        )
    })?;
    let len = u64_to_usize(length)?;

    // Fast path: a single fragment fully contains the request.
    for (start, bytes) in fragments {
        let fstart = *start;
        let fend = fragment_end(fstart, bytes.len())?;
        if offset >= fstart && req_end <= fend {
            let s = u64_to_usize(offset - fstart)?;
            return Ok(Coverage::Full(bytes[s..s + len].to_vec()));
        }
    }

    // Assembly path: stitch the request together from overlapping fragments.
    let mut out = vec![0u8; len];
    let mut covered = vec![false; len];
    for (start, bytes) in fragments {
        let fstart = *start;
        let fend = fragment_end(fstart, bytes.len())?;
        let lo = offset.max(fstart);
        let hi = req_end.min(fend);
        if lo < hi {
            let dst_lo = u64_to_usize(lo - offset)?;
            let dst_hi = u64_to_usize(hi - offset)?;
            let src_lo = u64_to_usize(lo - fstart)?;
            out[dst_lo..dst_hi].copy_from_slice(&bytes[src_lo..src_lo + (dst_hi - dst_lo)]);
            for c in &mut covered[dst_lo..dst_hi] {
                *c = true;
            }
        }
    }
    match covered.iter().position(|&c| !c) {
        None => Ok(Coverage::Full(out)),
        Some(pos) => {
            let miss_off = offset + pos as u64;
            Ok(Coverage::Gap { offset: miss_off, length: req_end - miss_off })
        }
    }
}

impl ByteRangeReader for FragmentRangeReader {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        match assemble_from_fragments(&self.fragments, offset, length)? {
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
/// build. It serves reads from the fragments gathered so far; on the first
/// read it cannot fully cover, it records the still-missing sub-range in
/// [`miss`](Self::miss) and returns an error to abort the metadata walk. The
/// JS driver fetches that range, appends the fragment, and retries — repeating
/// until the walk completes. Only metadata blocks are ever requested, so the
/// bulk sample data is never downloaded.
struct RecordingRangeReader {
    fragments: Vec<(u64, Vec<u8>)>,
    miss: Option<(u64, u64)>,
}

impl ByteRangeReader for RecordingRangeReader {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        match assemble_from_fragments(&self.fragments, offset, length)? {
            Coverage::Full(bytes) => Ok(bytes),
            Coverage::Gap { offset: miss, length: needed } => {
                self.miss = Some((miss, needed));
                Err(MdfError::BlockSerializationError(
                    "range not yet available; fetch and retry".to_string(),
                ))
            }
        }
    }
}

/// Build a [`FragmentRangeReader`] from a JS `[[offset, length], ...]` ranges
/// array and a parallel array of `Uint8Array` fragments (same order/length).
fn build_fragment_reader(
    ranges: &JsValue,
    fragments: &JsValue,
) -> Result<FragmentRangeReader, JsError> {
    let range_arr: Array = ranges
        .dyn_ref::<Array>()
        .ok_or_else(|| JsError::new("ranges must be an array of [offset, length] pairs"))?
        .clone();
    let frag_arr: Array = fragments
        .dyn_ref::<Array>()
        .ok_or_else(|| JsError::new("fragments must be an array of Uint8Array"))?
        .clone();

    if range_arr.length() != frag_arr.length() {
        return Err(JsError::new(&format!(
            "ranges length ({}) must match fragments length ({})",
            range_arr.length(),
            frag_arr.length()
        )));
    }

    let mut fragments_out = Vec::with_capacity(range_arr.length() as usize);
    for i in 0..range_arr.length() {
        let pair = range_arr
            .get(i)
            .dyn_into::<Array>()
            .map_err(|_| JsError::new("each range must be an [offset, length] array"))?;
        let offset = f64_to_u64(pair.get(0).as_f64(), "range offset")?;
        let length = f64_to_u64(pair.get(1).as_f64(), "range length")?;
        let bytes = frag_arr
            .get(i)
            .dyn_into::<Uint8Array>()
            .map_err(|_| JsError::new("each fragment must be a Uint8Array"))?
            .to_vec();
        if bytes.len() as u64 != length {
            return Err(JsError::new(&format!(
                "fragment {} has {} bytes but its declared range length is {}",
                i,
                bytes.len(),
                length
            )));
        }
        fragments_out.push((offset, bytes));
    }
    Ok(FragmentRangeReader { fragments: fragments_out })
}

/// Validate a JS number is a non-negative, finite integer and return it as
/// `u64` — rejecting `NaN`, infinities, negatives and fractionals instead of
/// silently saturating via `as u64`.
fn f64_to_u64(v: Option<f64>, what: &str) -> Result<u64, JsError> {
    let v = v.ok_or_else(|| JsError::new(&format!("{} must be a number", what)))?;
    if !v.is_finite() || v < 0.0 || v.fract() != 0.0 || v > MAX_SAFE_INTEGER as f64 {
        return Err(JsError::new(&format!(
            "{} must be a non-negative integer within Number.MAX_SAFE_INTEGER, got {}",
            what, v
        )));
    }
    Ok(v as u64)
}

// ---------------------------------------------------------------------------
// Serde info structs (serialized to plain JS objects via serde-wasm-bindgen)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelInfoJs {
    name: Option<String>,
    unit: Option<String>,
    comment: Option<String>,
    data_type: String,
    is_master: bool,
    bit_count: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupInfoJs {
    name: Option<String>,
    comment: Option<String>,
    record_count: u64,
    channels: Vec<ChannelInfoJs>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexGroupInfoJs {
    name: Option<String>,
    record_count: u64,
    channel_names: Vec<String>,
    master_channel: Option<String>,
}

/// One step of the incremental index build (`buildIndexStep`): either the
/// finished index as JSON, or the next byte range the build still needs.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildStepJs {
    done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    json: Option<String>,
    /// `[offset, length]` of the next range to fetch when `done` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    needed: Option<[f64; 2]>,
}

// ---------------------------------------------------------------------------
// Mdf (reader)
// ---------------------------------------------------------------------------

/// Read-only handle to an MDF 4 file parsed from an in-memory byte buffer.
///
/// Navigate by group/channel **name**. Only metadata is parsed up front;
/// samples are decoded lazily by `values`/`read`.
#[wasm_bindgen(js_name = Mdf)]
pub struct WasmMdf {
    mdf: Box<MDF>,
}

impl WasmMdf {
    fn build_groups(&self) -> Result<Vec<GroupInfoJs>, MdfError> {
        let mut groups = Vec::new();
        for g in self.mdf.channel_groups() {
            let mut channels = Vec::new();
            for ch in g.channels() {
                let block = ch.block();
                channels.push(ChannelInfoJs {
                    name: ch.name()?,
                    unit: ch.unit()?,
                    comment: ch.comment()?,
                    data_type: data_type_name(&block.data_type).to_string(),
                    is_master: block.channel_type == 2,
                    bit_count: block.bit_count,
                });
            }
            groups.push(GroupInfoJs {
                name: g.name()?,
                comment: g.comment()?,
                record_count: g.raw_channel_group().block.cycles_nr,
                channels,
            });
        }
        Ok(groups)
    }
}

#[wasm_bindgen(js_class = Mdf)]
impl WasmMdf {
    /// Parse an MDF 4 file from an owned byte buffer (e.g. a `Uint8Array`).
    #[wasm_bindgen(constructor)]
    pub fn new(data: &[u8]) -> Result<WasmMdf, JsError> {
        let mdf = MDF::from_bytes(data.to_vec()).map_err(err_to_js)?;
        Ok(WasmMdf { mdf: Box::new(mdf) })
    }

    /// Parse an MDF 4 file from an owned byte buffer (static alias for the
    /// constructor).
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(data: &[u8]) -> Result<WasmMdf, JsError> {
        WasmMdf::new(data)
    }

    /// Metadata for every channel group, each carrying its `channels`.
    ///
    /// Returns an array of
    /// `{ name, comment, recordCount, channels: [{ name, unit, comment, dataType, isMaster, bitCount }] }`.
    #[wasm_bindgen(js_name = groups)]
    pub fn groups(&self) -> Result<JsValue, JsError> {
        let groups = self.build_groups().map_err(err_to_js)?;
        serde_wasm_bindgen::to_value(&groups).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Names of every named channel across all groups (duplicates kept).
    #[wasm_bindgen(js_name = channelNames)]
    pub fn channel_names(&self) -> Result<Vec<String>, JsError> {
        let mut names = Vec::new();
        for g in self.mdf.channel_groups() {
            for ch in g.channels() {
                if let Some(n) = ch.name().map_err(err_to_js)? {
                    names.push(n);
                }
            }
        }
        Ok(names)
    }

    /// Read a numeric channel as a plain `Float64Array` (no timestamps).
    ///
    /// Conversions are applied; invalid / non-numeric samples become `NaN`.
    /// Pass `group` to disambiguate a channel name shared by several groups.
    #[wasm_bindgen(js_name = values)]
    pub fn values(&self, name: &str, group: Option<String>) -> Result<Vec<f64>, JsError> {
        for g in self.mdf.channel_groups() {
            if let Some(gn) = group.as_deref() {
                if g.name().map_err(err_to_js)?.as_deref() != Some(gn) {
                    continue;
                }
            }
            for ch in g.channels() {
                if ch.name().map_err(err_to_js)?.as_deref() == Some(name) {
                    return ch.values_as_f64().map_err(err_to_js);
                }
            }
        }
        Err(JsError::new(&match group {
            Some(gn) => format!("Channel '{}' not found in group '{}'", name, gn),
            None => format!("Channel '{}' not found", name),
        }))
    }

    /// Read a channel as a `{ name, unit, timestamps, values }` object.
    ///
    /// `values` carry all conversions; numeric samples are `number`, integers
    /// beyond `Number.MAX_SAFE_INTEGER` are `BigInt`, strings are `string`,
    /// byte arrays are `Uint8Array`, and invalid samples are `null`.
    #[wasm_bindgen(js_name = read)]
    pub fn read(&self, name: &str, group: Option<String>) -> Result<JsValue, JsError> {
        let signal = match group.as_deref() {
            Some(gn) => match self.mdf.group(gn) {
                Some(g) => g.signal(name).map_err(err_to_js)?,
                None => {
                    return Err(JsError::new(&format!("Channel group '{}' not found", gn)));
                }
            },
            None => self.mdf.signal(name).map_err(err_to_js)?,
        };
        let signal = signal.ok_or_else(|| {
            JsError::new(&match group {
                Some(gn) => format!("Channel '{}' not found in group '{}'", name, gn),
                None => format!("Channel '{}' not found", name),
            })
        })?;
        signal_to_js(signal)
    }

    /// Measurement start time in nanoseconds since the Unix epoch, or
    /// `undefined` if the file header does not record one.
    #[wasm_bindgen(js_name = startTimeNs)]
    pub fn start_time_ns(&self) -> Option<u64> {
        self.mdf.start_time_ns()
    }
}

// ---------------------------------------------------------------------------
// MdfIndex
// ---------------------------------------------------------------------------

/// A self-contained, JSON-serialisable index over an MDF 4 file.
///
/// Build it from bytes (or reload from JSON), inspect metadata by name, compute
/// byte ranges for partial fetches, then decode values from caller-fetched
/// fragments — the model for reading large or remote MDF files in the browser.
#[wasm_bindgen(js_name = MdfIndex)]
pub struct WasmMdfIndex {
    index: MdfIndex,
}

impl WasmMdfIndex {
    /// Resolve `(group_index, channel_index)` by name (+ optional group).
    fn locate(&self, name: &str, group: Option<&str>) -> Result<(usize, usize), JsError> {
        let found = match group {
            Some(gn) => self.index.locate_in(gn, name),
            None => self.index.locate(name),
        };
        found.ok_or_else(|| {
            JsError::new(&match group {
                Some(gn) => format!("Channel '{}' not found in group '{}'", name, gn),
                None => format!("Channel '{}' not found", name),
            })
        })
    }

    /// Full data-section byte ranges for the group owning `name`.
    ///
    /// Returns one `(file_offset + 24, size - 24)` span per `##DT`/`##DV`
    /// fragment of the owning group (the 24 skips the block header), merged.
    /// For a VLSD channel the spans of its `##SD` fragment chain are included
    /// as well — the decoder resolves each record's inline offset into that
    /// stream, so its bytes must be fetched alongside the fixed records.
    /// This is exactly what the fragment readers need: `valuesFromFragments` /
    /// `readFromFragments` decode whole data sections, not just the requested
    /// channel's columns, so the fragments must cover each full section.
    fn signal_ranges(&self, name: &str, group: Option<&str>) -> Result<Vec<(u64, u64)>, JsError> {
        let (g, c) = self.locate(name, group)?;
        let grp = &self.index.channel_groups[g];
        let channel = &grp.channels[c];
        let mut ranges =
            Vec::with_capacity(grp.data_blocks.len() + channel.vlsd_data_blocks.len());
        for db in grp.data_blocks.iter().chain(channel.vlsd_data_blocks.iter()) {
            if db.is_compressed {
                return Err(JsError::new(
                    "compressed (##DZ) data blocks are not supported for fragment reads",
                ));
            }
            let data_len = db.size.checked_sub(24).ok_or_else(|| {
                JsError::new(&format!(
                    "invalid index: data block at offset {} has size {} (smaller than the \
                     24-byte block header)",
                    db.file_offset, db.size
                ))
            })?;
            ranges.push((db.file_offset + 24, data_len));
        }
        Ok(merge_ranges(ranges))
    }
}

#[wasm_bindgen(js_class = MdfIndex)]
impl WasmMdfIndex {
    /// Build a fresh index by parsing an MDF file from an in-memory buffer.
    ///
    /// All conversions are resolved during construction, so the index is fully
    /// self-contained afterwards (and can be serialised with `toJson`).
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(data: &[u8]) -> Result<WasmMdfIndex, JsError> {
        let index = MdfIndex::from_bytes(data.to_vec()).map_err(err_to_js)?;
        Ok(WasmMdfIndex { index })
    }

    /// Reload a previously serialised index from its JSON string.
    #[wasm_bindgen(js_name = fromJson)]
    pub fn from_json(json: &str) -> Result<WasmMdfIndex, JsError> {
        let index = MdfIndex::from_json(json).map_err(err_to_js)?;
        Ok(WasmMdfIndex { index })
    }

    /// One step of an incremental, range-fetched index build.
    ///
    /// Drives the native metadata walk (which reads only structural blocks,
    /// never sample data) against the `ranges`/`fragments` gathered so far.
    /// Returns `{ done: true, json }` with the finished index once the walk
    /// completes, or `{ done: false, needed: [offset, length] }` naming the
    /// next byte range to fetch and feed back. The JS `MdfIndex.fromRangeSource`
    /// wrapper loops over this; call it directly only for a custom driver.
    ///
    /// `fileSize` is the total file length (from a HEAD request or
    /// `RangeSource.size()`); it is stored on the resulting index. `ranges` is
    /// the `[[offset, length], ...]` array of fragments fetched so far and
    /// `fragments` the parallel array of `Uint8Array` bytes.
    #[wasm_bindgen(js_name = buildIndexStep)]
    pub fn build_index_step(
        file_size: f64,
        ranges: JsValue,
        fragments: JsValue,
    ) -> Result<JsValue, JsError> {
        let file_size = f64_to_u64(Some(file_size), "file size")?;
        let parsed = build_fragment_reader(&ranges, &fragments)?;
        let mut reader = RecordingRangeReader { fragments: parsed.fragments, miss: None };
        match MdfIndex::from_range_reader(&mut reader, file_size) {
            Ok(index) => {
                let json = index.to_json().map_err(err_to_js)?;
                let step = BuildStepJs { done: true, json: Some(json), needed: None };
                serde_wasm_bindgen::to_value(&step).map_err(|e| JsError::new(&e.to_string()))
            }
            Err(e) => match reader.miss {
                Some((offset, length)) => {
                    let step = BuildStepJs {
                        done: false,
                        json: None,
                        needed: Some([offset as f64, length as f64]),
                    };
                    serde_wasm_bindgen::to_value(&step).map_err(|e| JsError::new(&e.to_string()))
                }
                // A real error (not a missing-range abort) — surface it.
                None => Err(err_to_js(e)),
            },
        }
    }

    /// Serialise the index to a JSON string.
    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> Result<String, JsError> {
        self.index.to_json().map_err(err_to_js)
    }

    /// Validate internal consistency of the index (block layout vs file size).
    #[wasm_bindgen(js_name = validate)]
    pub fn validate(&self) -> Result<(), JsError> {
        self.index.validate().map_err(err_to_js)
    }

    /// Metadata for every channel group.
    ///
    /// Returns an array of `{ name, recordCount, channelNames, masterChannel }`.
    #[wasm_bindgen(js_name = groups)]
    pub fn groups(&self) -> Result<JsValue, JsError> {
        let groups: Vec<IndexGroupInfoJs> = self
            .index
            .groups()
            .iter()
            .map(|g| IndexGroupInfoJs {
                name: g.name.clone(),
                record_count: g.record_count,
                channel_names: g.channel_names().into_iter().map(String::from).collect(),
                master_channel: g.master_channel().and_then(|c| c.name.clone()),
            })
            .collect();
        serde_wasm_bindgen::to_value(&groups).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Names of every named channel across all groups (duplicates kept).
    #[wasm_bindgen(js_name = channelNames)]
    pub fn channel_names(&self) -> Vec<String> {
        self.index.channel_names().into_iter().map(String::from).collect()
    }

    /// Total size of the source MDF file (bytes) when the index was built.
    #[wasm_bindgen(js_name = fileSize)]
    pub fn file_size(&self) -> Result<f64, JsError> {
        if self.index.file_size > MAX_SAFE_INTEGER {
            return Err(JsError::new(
                "file size exceeds Number.MAX_SAFE_INTEGER (2^53-1)",
            ));
        }
        Ok(self.index.file_size as f64)
    }

    /// Byte ranges `[[offset, length], ...]` occupied by a channel.
    ///
    /// Each span coalesces one data-block fragment and, because records
    /// interleave all channels, includes neighbouring channels' bytes. Pass
    /// `group` to disambiguate.
    #[wasm_bindgen(js_name = byteRanges)]
    pub fn byte_ranges(&self, name: &str, group: Option<String>) -> Result<JsValue, JsError> {
        let ranges = match group.as_deref() {
            Some(gn) => self.index.byte_ranges_in(gn, name).map_err(err_to_js)?,
            None => self.index.byte_ranges(name).map_err(err_to_js)?,
        };
        ranges_to_js(ranges)
    }

    /// Byte ranges covering a record window `[startRecord, startRecord+recordCount)`.
    #[wasm_bindgen(js_name = byteRangesForRecords)]
    pub fn byte_ranges_for_records(
        &self,
        name: &str,
        start_record: u64,
        record_count: u64,
        group: Option<String>,
    ) -> Result<JsValue, JsError> {
        let ranges = match group.as_deref() {
            Some(gn) => {
                let (g, c) = self.locate(name, Some(gn))?;
                self.index
                    .get_channel_byte_ranges_for_records(g, c, start_record, record_count)
                    .map_err(err_to_js)?
            }
            None => self
                .index
                .byte_ranges_for_records(name, start_record, record_count)
                .map_err(err_to_js)?,
        };
        ranges_to_js(ranges)
    }

    /// Full data-section byte ranges for the group owning `name`, merged.
    ///
    /// Returns one span per data-block fragment covering the entire data
    /// section (block bytes minus the 24-byte header); for a VLSD channel the
    /// spans of its `##SD` fragment chain are included too. These are exactly
    /// the ranges the fragment readers need: `valuesFromFragments` /
    /// `readFromFragments` decode whole data sections (all channels are
    /// interleaved per record), so fetch these and pass the fetched fragments
    /// straight through — the requested channel and its master are both
    /// covered regardless of record layout. Errors if any block is compressed.
    #[wasm_bindgen(js_name = signalByteRanges)]
    pub fn signal_byte_ranges(
        &self,
        name: &str,
        group: Option<String>,
    ) -> Result<JsValue, JsError> {
        let ranges = self.signal_ranges(name, group.as_deref())?;
        ranges_to_js(ranges)
    }

    /// Decode a numeric channel to a `Float64Array` from caller-fetched
    /// fragments (invalid / non-numeric samples become `NaN`).
    ///
    /// `ranges` is the `[[offset, length], ...]` array previously returned by
    /// `byteRanges` / `signalByteRanges`, and `fragments` is the parallel array
    /// of `Uint8Array` byte blobs fetched for those ranges. The decoder reads
    /// whole data sections, so the fragments must cover them — pass
    /// `signalByteRanges` output when in doubt.
    #[wasm_bindgen(js_name = valuesFromFragments)]
    pub fn values_from_fragments(
        &self,
        name: &str,
        group: Option<String>,
        ranges: JsValue,
        fragments: JsValue,
    ) -> Result<Vec<f64>, JsError> {
        let reader = build_fragment_reader(&ranges, &fragments)?;
        let mut bound = self.index.open(reader);
        match group.as_deref() {
            Some(gn) => bound.values_f64_in(gn, name).map_err(err_to_js),
            None => bound.values_f64(name).map_err(err_to_js),
        }
    }

    /// Decode a channel to a `{ name, unit, timestamps, values }` object from
    /// caller-fetched fragments.
    ///
    /// Pass the ranges from `signalByteRanges` (channel + master, merged) and
    /// the fragments fetched for them.
    #[wasm_bindgen(js_name = readFromFragments)]
    pub fn read_from_fragments(
        &self,
        name: &str,
        group: Option<String>,
        ranges: JsValue,
        fragments: JsValue,
    ) -> Result<JsValue, JsError> {
        let reader = build_fragment_reader(&ranges, &fragments)?;
        let mut bound = self.index.open(reader);
        let signal = match group.as_deref() {
            Some(gn) => bound.signal_in(gn, name).map_err(err_to_js)?,
            None => bound.signal(name).map_err(err_to_js)?,
        };
        signal_to_js(signal)
    }
}

// ---------------------------------------------------------------------------
// MdfWriter (in-memory)
// ---------------------------------------------------------------------------

/// A `Write + Seek` sink over a shared in-memory cursor, so the finished file
/// bytes can be recovered after the writer consumes itself in `finalize`.
struct SharedBuf(Rc<RefCell<Cursor<Vec<u8>>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.borrow_mut().flush()
    }
}

impl Seek for SharedBuf {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.0.borrow_mut().seek(pos)
    }
}

/// Streaming writer that produces an MDF 4 file entirely in memory.
///
/// Mirrors the Python `MdfWriter`: `initMdfFile`, then define structure
/// (`addChannelGroup`, `addTimeChannel`, `addFloatChannel`, `addIntChannel`,
/// `addChannel`, `setTimeChannel`), then write data (`startDataBlock`,
/// `writeRecord`, `finishDataBlock`), then `finalize()` to get a `Uint8Array`.
/// Group/channel IDs are opaque strings — pass them back to later calls.
#[wasm_bindgen(js_name = MdfWriter)]
pub struct WasmMdfWriter {
    writer: Option<MdfWriter>,
    sink: Rc<RefCell<Cursor<Vec<u8>>>>,
    channel_groups: HashMap<String, String>,
    channels: HashMap<String, String>,
    last_channels: HashMap<String, String>,
    channel_types: HashMap<String, Vec<DataType>>,
    next_id: usize,
}

impl WasmMdfWriter {
    fn writer_mut(&mut self) -> Result<&mut MdfWriter, JsError> {
        self.writer
            .as_mut()
            .ok_or_else(|| JsError::new("Writer has been finalized"))
    }

    fn add_channel_with_bits(
        &mut self,
        group_id: &str,
        name: &str,
        data_type: DataType,
        bit_count: u32,
    ) -> Result<String, JsError> {
        let cg_id = self
            .channel_groups
            .get(group_id)
            .ok_or_else(|| JsError::new("Channel group not found"))?
            .clone();
        let prev_channel_id = self
            .last_channels
            .get(group_id)
            .and_then(|py_id| self.channels.get(py_id))
            .cloned();
        let dt = data_type.clone();
        let ch_id = self
            .writer_mut()?
            .add_channel(&cg_id, prev_channel_id.as_deref(), |ch| {
                ch.data_type = dt.clone();
                ch.name = Some(name.to_string());
                ch.bit_count = bit_count;
            })
            .map_err(err_to_js)?;

        let py_id = format!("ch_{}", self.next_id);
        self.next_id += 1;
        self.channels.insert(py_id.clone(), ch_id);
        self.last_channels.insert(group_id.to_string(), py_id.clone());
        self.channel_types
            .entry(group_id.to_string())
            .or_default()
            .push(data_type);
        Ok(py_id)
    }
}

#[wasm_bindgen(js_class = MdfWriter)]
impl WasmMdfWriter {
    /// Create an in-memory writer. No MDF blocks are written until
    /// `initMdfFile` is called.
    #[wasm_bindgen(constructor)]
    pub fn new() -> WasmMdfWriter {
        let sink = Rc::new(RefCell::new(Cursor::new(Vec::new())));
        let writer = MdfWriter::new_from_writer(SharedBuf(Rc::clone(&sink)));
        WasmMdfWriter {
            writer: Some(writer),
            sink,
            channel_groups: HashMap::new(),
            channels: HashMap::new(),
            last_channels: HashMap::new(),
            channel_types: HashMap::new(),
            next_id: 0,
        }
    }

    /// Write the identification (`##ID`) and header (`##HD`) blocks. Call once,
    /// before adding any channel group.
    #[wasm_bindgen(js_name = initMdfFile)]
    pub fn init_mdf_file(&mut self) -> Result<(), JsError> {
        self.writer_mut()?.init_mdf_file().map_err(err_to_js)?;
        Ok(())
    }

    /// Set the measurement start time (header `abs_time`) in nanoseconds since
    /// the Unix epoch. Optional; call after `initMdfFile`.
    #[wasm_bindgen(js_name = setStartTime)]
    pub fn set_start_time(&mut self, abs_time_ns: u64) -> Result<(), JsError> {
        self.writer_mut()?
            .set_start_time(abs_time_ns, 0, 0, 0, 0)
            .map_err(err_to_js)?;
        Ok(())
    }

    /// Append a new channel group. Returns an opaque group ID.
    #[wasm_bindgen(js_name = addChannelGroup)]
    pub fn add_channel_group(&mut self, name: Option<String>) -> Result<String, JsError> {
        let cg_id = self.writer_mut()?.add_channel_group(None, |_cg| {}).map_err(err_to_js)?;
        if let Some(n) = &name {
            self.writer_mut()?.set_channel_group_name(&cg_id, n).map_err(err_to_js)?;
        }
        let py_id = format!("cg_{}", self.next_id);
        self.next_id += 1;
        self.channel_groups.insert(py_id.clone(), cg_id);
        self.channel_types.insert(py_id.clone(), Vec::new());
        Ok(py_id)
    }

    /// Add a generic fixed-length channel using the data type's natural bit
    /// width.
    ///
    /// `dataType` is a symbolic name such as `"FloatLE"`, `"UnsignedIntegerLE"`,
    /// or `"SignedIntegerLE"` (float defaults to 32 bits — use
    /// `addFloatChannel` for f64). String channels are **not** fixed-length in
    /// this writer; use `addStringChannel` for text.
    #[wasm_bindgen(js_name = addChannel)]
    pub fn add_channel(
        &mut self,
        group_id: &str,
        name: &str,
        data_type: &str,
    ) -> Result<String, JsError> {
        let dt = data_type_from_str(data_type)?;
        if is_string_type(&dt) {
            return Err(JsError::new(
                "string data types are variable-length in this writer; use addStringChannel",
            ));
        }
        let bits = dt.default_bits();
        self.add_channel_with_bits(group_id, name, dt, bits)
    }

    /// Add a variable-length (VLSD) UTF-8 string channel.
    ///
    /// Mirrors the Python `add_string_channel`: the record slot holds a `u64`
    /// offset into a `##SD` block the writer manages automatically. Pass JS
    /// `string` values for this channel to `writeRecord`.
    #[wasm_bindgen(js_name = addStringChannel)]
    pub fn add_string_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        let cg_id = self
            .channel_groups
            .get(group_id)
            .ok_or_else(|| JsError::new("Channel group not found"))?
            .clone();
        let prev_channel_id = self
            .last_channels
            .get(group_id)
            .and_then(|py_id| self.channels.get(py_id))
            .cloned();
        let name_owned = name.to_string();
        let ch_id = self
            .writer_mut()?
            .add_channel(&cg_id, prev_channel_id.as_deref(), |ch| {
                ch.data_type = DataType::StringUtf8;
                ch.name = Some(name_owned.clone());
                ch.channel_type = 1; // VLSD
                ch.data = 1; // routed to the SD-block writer; patched on finish
                ch.bit_count = 64; // record slot holds a u64 offset
            })
            .map_err(err_to_js)?;

        let py_id = format!("ch_{}", self.next_id);
        self.next_id += 1;
        self.channels.insert(py_id.clone(), ch_id);
        self.last_channels.insert(group_id.to_string(), py_id.clone());
        self.channel_types
            .entry(group_id.to_string())
            .or_default()
            .push(DataType::StringUtf8);
        Ok(py_id)
    }

    /// Add a 64-bit float channel and mark it as the group's master/time
    /// channel. Usually the first channel added to a group.
    #[wasm_bindgen(js_name = addTimeChannel)]
    pub fn add_time_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        let ch_id = self.add_channel_with_bits(group_id, name, DataType::FloatLE, 64)?;
        self.set_time_channel(&ch_id)?;
        Ok(ch_id)
    }

    /// Add a 64-bit little-endian float (`f64`) data channel.
    #[wasm_bindgen(js_name = addFloatChannel)]
    pub fn add_float_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        self.add_channel_with_bits(group_id, name, DataType::FloatLE, 64)
    }

    /// Add a 32-bit little-endian float (`f32`) data channel.
    #[wasm_bindgen(js_name = addFloat32Channel)]
    pub fn add_float32_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        self.add_channel_with_bits(group_id, name, DataType::FloatLE, 32)
    }

    /// Add a 64-bit little-endian unsigned integer (`u64`) data channel.
    #[wasm_bindgen(js_name = addIntChannel)]
    pub fn add_int_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        self.add_channel_with_bits(group_id, name, DataType::UnsignedIntegerLE, 64)
    }

    /// Add a 64-bit little-endian signed integer (`i64`) data channel.
    #[wasm_bindgen(js_name = addSintChannel)]
    pub fn add_sint_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        self.add_channel_with_bits(group_id, name, DataType::SignedIntegerLE, 64)
    }

    /// Mark an existing channel as the group's master/time channel.
    #[wasm_bindgen(js_name = setTimeChannel)]
    pub fn set_time_channel(&mut self, channel_id: &str) -> Result<(), JsError> {
        let ch_id = self
            .channels
            .get(channel_id)
            .ok_or_else(|| JsError::new("Channel not found"))?
            .clone();
        self.writer_mut()?.set_time_channel(&ch_id).map_err(err_to_js)?;
        Ok(())
    }

    /// Open a fresh `##DT` data block for a channel group. Call once, after all
    /// channels have been added and before any `writeRecord`.
    #[wasm_bindgen(js_name = startDataBlock)]
    pub fn start_data_block(&mut self, group_id: &str) -> Result<(), JsError> {
        let cg_id = self
            .channel_groups
            .get(group_id)
            .ok_or_else(|| JsError::new("Channel group not found"))?
            .clone();
        self.writer_mut()?.start_data_block_for_cg(&cg_id, 0).map_err(err_to_js)?;
        Ok(())
    }

    /// Append a single record: one value per channel, in the order channels were
    /// added. Each value is a `number` (encoded per the channel's data type), a
    /// `string` (for string channels), or a `BigInt` — accepted for integer
    /// channels so values beyond `Number.MAX_SAFE_INTEGER` round-trip (the
    /// reader emits `BigInt` for those). A `BigInt` must fit the channel's
    /// signed/unsigned 64-bit range.
    #[wasm_bindgen(js_name = writeRecord)]
    pub fn write_record(&mut self, group_id: &str, values: JsValue) -> Result<(), JsError> {
        let cg_id = self
            .channel_groups
            .get(group_id)
            .ok_or_else(|| JsError::new("Channel group not found"))?
            .clone();
        let types = self
            .channel_types
            .get(group_id)
            .ok_or_else(|| JsError::new("Channel group not found"))?
            .clone();
        let arr: Array = values
            .dyn_ref::<Array>()
            .ok_or_else(|| JsError::new("record values must be an array"))?
            .clone();
        if arr.length() as usize != types.len() {
            return Err(JsError::new(&format!(
                "expected {} values for this group, got {}",
                types.len(),
                arr.length()
            )));
        }

        let mut rust_values = Vec::with_capacity(types.len());
        for (i, dt) in types.iter().enumerate() {
            let v = arr.get(i as u32);
            let decoded = if let Some(s) = v.as_string() {
                DecodedValue::String(s)
            } else if let Some(b) = v.dyn_ref::<js_sys::BigInt>() {
                match dt {
                    DataType::SignedIntegerLE | DataType::SignedIntegerBE => {
                        let n = i64::try_from(b.clone()).map_err(|_| {
                            JsError::new(&format!(
                                "record value at index {} (BigInt) does not fit in a signed 64-bit \
                                 integer",
                                i
                            ))
                        })?;
                        DecodedValue::SignedInteger(n)
                    }
                    DataType::UnsignedIntegerLE | DataType::UnsignedIntegerBE => {
                        let n = u64::try_from(b.clone()).map_err(|_| {
                            JsError::new(&format!(
                                "record value at index {} (BigInt) does not fit in an unsigned \
                                 64-bit integer",
                                i
                            ))
                        })?;
                        DecodedValue::UnsignedInteger(n)
                    }
                    _ => {
                        return Err(JsError::new(&format!(
                            "record value at index {} is a BigInt but that channel is not an \
                             integer channel",
                            i
                        )));
                    }
                }
            } else if let Some(f) = v.as_f64() {
                match dt {
                    DataType::FloatLE | DataType::FloatBE => DecodedValue::Float(f),
                    DataType::SignedIntegerLE | DataType::SignedIntegerBE => {
                        DecodedValue::SignedInteger(f as i64)
                    }
                    DataType::UnsignedIntegerLE | DataType::UnsignedIntegerBE => {
                        DecodedValue::UnsignedInteger(f as u64)
                    }
                    _ => DecodedValue::Float(f),
                }
            } else {
                return Err(JsError::new(&format!(
                    "record value at index {} must be a number, string or BigInt",
                    i
                )));
            };
            rust_values.push(decoded);
        }

        self.writer_mut()?.write_record(&cg_id, &rust_values).map_err(err_to_js)?;
        Ok(())
    }

    /// Close the open `##DT` block for a channel group (writing a `##DL` block
    /// if the data was split). Call once per group after all records.
    #[wasm_bindgen(js_name = finishDataBlock)]
    pub fn finish_data_block(&mut self, group_id: &str) -> Result<(), JsError> {
        let cg_id = self
            .channel_groups
            .get(group_id)
            .ok_or_else(|| JsError::new("Channel group not found"))?
            .clone();
        self.writer_mut()?.finish_data_block(&cg_id).map_err(err_to_js)?;
        Ok(())
    }

    /// Flush the writer and return the finished MDF file as a `Uint8Array`.
    ///
    /// After this the writer is consumed; any further call raises.
    #[wasm_bindgen(js_name = finalize)]
    pub fn finalize(&mut self) -> Result<Vec<u8>, JsError> {
        let writer = self
            .writer
            .take()
            .ok_or_else(|| JsError::new("Writer already finalized"))?;
        writer.finalize().map_err(err_to_js)?;
        let bytes = self.sink.borrow().get_ref().clone();
        Ok(bytes)
    }
}

impl Default for WasmMdfWriter {
    fn default() -> Self {
        WasmMdfWriter::new()
    }
}
