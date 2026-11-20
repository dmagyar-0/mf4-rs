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

use crate::api::mdf::Mdf;
use crate::blocks::common::DataType;
use crate::error::MdfError;
use crate::fragments::{
    merge_ranges, merge_ranges_bridging, FragmentRangeReader, FragmentStore, RecordingRangeReader,
    BLOCK_GAP_BRIDGE,
};
use crate::index::{ByteRangeReader, MdfIndex};
use crate::parsing::decoder::DecodedValue;
use crate::signal::Signal;
use crate::types::{ConversionInfo, ReadOptions};
use crate::writer::MdfWriter;

/// Largest integer exactly representable as a JS `number` (2^53 - 1).
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn err_to_js(e: MdfError) -> JsError {
    JsError::new(&e.to_string())
}

/// Symbolic MDF data-type name (matches the Python `DataType.name`).
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


/// Collapse the raw gaps a build/conversion pass recorded into the compact
/// `[[offset, length], ...]` list handed back to JS: merge overlapping and
/// touching ranges (via [`merge_ranges`]) so the driver fetches a few spans
/// rather than one request per tiny text block.
fn misses_to_needed(misses: Vec<(u64, u64)>) -> Vec<[f64; 2]> {
    merge_ranges(misses)
        .into_iter()
        .map(|(offset, length)| [offset as f64, length as f64])
        .collect()
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
    let store = FragmentStore::from_pairs(fragments_out).map_err(err_to_js)?;
    Ok(FragmentRangeReader { store })
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

/// Serialize a contract type (`GroupInfo`, `ChannelInfo`, `ConversionInfo`,
/// …) to a plain JS object (maps become objects, `None` becomes `undefined`).
fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsError> {
    value
        .serialize(&serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true))
        .map_err(|e| JsError::new(&e.to_string()))
}

/// `conversion` as a plain JS `ConversionInfo` object, or `null`.
fn conversion_to_js(
    conversion: Option<&crate::blocks::conversion::ConversionBlock>,
) -> Result<JsValue, JsError> {
    match conversion {
        None => Ok(JsValue::NULL),
        Some(cc) => to_js(&ConversionInfo::from(cc)),
    }
}

/// A JS array of `Signal` objects.
fn signals_to_js(signals: Vec<Signal>) -> Result<JsValue, JsError> {
    let array = js_sys::Array::new_with_length(signals.len() as u32);
    for (i, signal) in signals.into_iter().enumerate() {
        array.set(i as u32, signal_to_js(signal)?);
    }
    Ok(array.into())
}

/// Finish one step of a gap-driven read: `{ done: false, needed }` while the
/// probing reader recorded misses, otherwise `{ done: true, [key]: value }`
/// (or the read's error).
fn gap_step<T>(
    misses: Vec<(u64, u64)>,
    result: Result<T, MdfError>,
    key: &str,
    to_value: impl FnOnce(T) -> Result<JsValue, JsError>,
) -> Result<JsValue, JsError> {
    let out = Object::new();
    let set = |k: &str, v: &JsValue| -> Result<(), JsError> {
        Reflect::set(&out, &JsValue::from_str(k), v)
            .map(|_| ())
            .map_err(|_| JsError::new("failed to build step result"))
    };
    if !misses.is_empty() {
        set("done", &JsValue::FALSE)?;
        let needed = serde_wasm_bindgen::to_value(&misses_to_needed(misses))
            .map_err(|e| JsError::new(&e.to_string()))?;
        set("needed", &needed)?;
        return Ok(out.into());
    }
    set("done", &JsValue::TRUE)?;
    set(key, &to_value(result.map_err(err_to_js)?)?)?;
    Ok(out.into())
}

/// Build [`ReadOptions`] from a group name and a JS `ReadWindow` object.
fn read_options(group: Option<String>, window: JsValue) -> Result<ReadOptions, JsError> {
    let w: WindowJs = if window.is_undefined() || window.is_null() {
        WindowJs::default()
    } else {
        serde_wasm_bindgen::from_value(window).map_err(|e| JsError::new(&e.to_string()))?
    };
    let integer = |v: Option<f64>, what: &str| v.map(|v| f64_to_u64(Some(v), what)).transpose();
    Ok(ReadOptions {
        group,
        start_time: w.start_time,
        end_time: w.end_time,
        start_record: integer(w.start_record, "startRecord")?,
        record_count: integer(w.record_count, "recordCount")?,
        raw: w.raw.unwrap_or(false),
    })
}

/// Record or time window passed to `readWindowStep` (all fields optional),
/// plus `raw` (skip the channel's conversion).
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct WindowJs {
    start_time: Option<f64>,
    end_time: Option<f64>,
    start_record: Option<f64>,
    record_count: Option<f64>,
    raw: Option<bool>,
}

/// One step of the incremental index build (`buildIndexStep`): either the
/// finished index as JSON, or the byte ranges the build still needs.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildStepJs {
    done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    json: Option<String>,
    /// `[[offset, length], ...]` of every range the walk still needs when
    /// `done` is false. A single walk gathers **all** the ranges it can
    /// (every channel name/unit/comment reachable this pass, plus the structural
    /// block that blocked further progress), so the JS driver fetches them in
    /// one batch and the whole build finishes in a handful of passes instead of
    /// one pass per metadata block.
    #[serde(skip_serializing_if = "Option::is_none")]
    needed: Option<Vec<[f64; 2]>>,
}

// ---------------------------------------------------------------------------
// MDF (reader)
// ---------------------------------------------------------------------------

/// Read-only handle to an MDF 4 file parsed from an in-memory byte buffer.
///
/// Navigate by group/channel **name**. Only metadata is parsed up front;
/// samples are decoded lazily by `values`/`read`.
#[wasm_bindgen(js_name = Mdf)]
pub struct WasmMdf {
    mdf: Box<Mdf>,
}

#[wasm_bindgen(js_class = Mdf)]
impl WasmMdf {
    /// Parse an MDF 4 file from an owned byte buffer (e.g. a `Uint8Array`).
    #[wasm_bindgen(constructor)]
    pub fn new(data: &[u8]) -> Result<WasmMdf, JsError> {
        let mdf = Mdf::from_bytes(data.to_vec()).map_err(err_to_js)?;
        Ok(WasmMdf { mdf: Box::new(mdf) })
    }

    /// Parse an MDF 4 file from an owned byte buffer (static alias for the
    /// constructor).
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(data: &[u8]) -> Result<WasmMdf, JsError> {
        WasmMdf::new(data)
    }

    /// Metadata of every channel group (`GroupInfo[]`), in file order.
    #[wasm_bindgen(js_name = groups)]
    pub fn groups(&self) -> Result<JsValue, JsError> {
        to_js(&self.mdf.groups().map_err(err_to_js)?)
    }

    /// Metadata of the first group named `name`, or `undefined`.
    #[wasm_bindgen(js_name = group)]
    pub fn group(&self, name: &str) -> Result<JsValue, JsError> {
        to_js(&self.mdf.group(name).map_err(err_to_js)?)
    }

    /// Metadata of the first channel named `name` (in the group(s) named
    /// `group`, when given), or `undefined`.
    #[wasm_bindgen(js_name = channel)]
    pub fn channel(&self, name: &str, group: Option<String>) -> Result<JsValue, JsError> {
        to_js(&self.mdf.channel(name, group.as_deref()).map_err(err_to_js)?)
    }

    /// Names of every named channel across all groups (duplicates kept).
    #[wasm_bindgen(js_name = channelNames)]
    pub fn channel_names(&self) -> Result<Vec<String>, JsError> {
        self.mdf.channel_names().map_err(err_to_js)
    }

    /// Read a numeric channel as `Float64Array` (conversions applied; invalid
    /// and non-numeric samples are `NaN`). `window` is a `ReadWindow` object.
    #[wasm_bindgen(js_name = values)]
    pub fn values(&self, name: &str, group: Option<String>, window: JsValue) -> Result<Vec<f64>, JsError> {
        let options = read_options(group, window)?;
        self.mdf.values(name, &options).map_err(err_to_js)
    }

    /// Read a channel as a `Signal` object. `window` is a `ReadWindow` object.
    #[wasm_bindgen(js_name = read)]
    pub fn read(&self, name: &str, group: Option<String>, window: JsValue) -> Result<JsValue, JsError> {
        let options = read_options(group, window)?;
        signal_to_js(self.mdf.read(name, &options).map_err(err_to_js)?)
    }

    /// Read several channels as an array of `Signal` objects, in order, with
    /// the same `group` / `window` for all of them (a shared time axis is
    /// decoded once).
    #[wasm_bindgen(js_name = readMany)]
    pub fn read_many(&self, names: Vec<String>, group: Option<String>, window: JsValue) -> Result<JsValue, JsError> {
        let options = read_options(group, window)?;
        signals_to_js(self.mdf.read_many(&names, &options).map_err(err_to_js)?)
    }

    /// `{ start, end }` of the group's master (time) channel, or `undefined`
    /// for an empty group.
    #[wasm_bindgen(js_name = timeRange)]
    pub fn time_range(&self, name: &str, group: Option<String>) -> Result<JsValue, JsError> {
        to_js(&self.mdf.time_range(name, group.as_deref()).map_err(err_to_js)?)
    }

    /// Measurement start time in nanoseconds since the Unix epoch, or
    /// `undefined` if the file header does not record one.
    #[wasm_bindgen(js_name = startTimeNs)]
    pub fn start_time_ns(&self) -> Option<u64> {
        self.mdf.start_time_ns()
    }

    /// The resolved conversion of channel `name` (`ConversionInfo`), or `null`.
    #[wasm_bindgen(js_name = conversion)]
    pub fn conversion(&self, name: &str, group: Option<String>) -> Result<JsValue, JsError> {
        match self.mdf.conversion(name, group.as_deref()).map_err(err_to_js)? {
            Some(info) => to_js(&info),
            None => Ok(JsValue::NULL),
        }
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
        self.index.locate_checked(name, group).map_err(err_to_js)
    }

    /// Byte ranges covering every data block of the group owning `name`.
    ///
    /// Returns one `(file_offset, size)` span per `##DT`/`##DV` fragment of the
    /// owning group — the whole block including its 24-byte header, which the
    /// decoder verifies before decoding — merged, with fragments stored back to
    /// back (up to 7 bytes of alignment padding apart) bridged into one span,
    /// so a column-oriented signal comes back as a single span. When the
    /// group's master lives in another group (MDF 4.2 remote master) that
    /// group's fragments are included too when `with_master` is set, so the
    /// timestamps decode.
    /// For a VLSD channel the spans of its `##SD` fragment chain are included
    /// as well — the decoder resolves each record's inline offset into that
    /// stream, so its bytes must be fetched alongside the fixed records.
    /// This is exactly what the fragment readers need: `valuesFromFragments` /
    /// `readFromFragments` decode whole data sections, not just the requested
    /// channel's columns, so the fragments must cover each full section.
    fn signal_ranges(&self, name: &str, group: Option<&str>, with_master: bool) -> Result<Vec<(u64, u64)>, JsError> {
        let (g, c) = self.locate(name, group)?;
        let grp = &self.index.channel_groups[g];
        let channel = &grp.channels[c];
        let remote_master = match self.index.master_location(g, c) {
            Some((mg, _)) if with_master && mg != g => self.index.channel_groups[mg].data_blocks.as_slice(),
            _ => &[],
        };
        let mut ranges =
            Vec::with_capacity(grp.data_blocks.len() + channel.vlsd_data_blocks.len());
        for db in grp
            .data_blocks
            .iter()
            .chain(channel.vlsd_data_blocks.iter())
            .chain(remote_master.iter())
        {
            if db.is_compressed {
                return Err(JsError::new(
                    "compressed (##DZ) data blocks are not supported for fragment reads",
                ));
            }
            if db.size < 24 {
                return Err(JsError::new(&format!(
                    "invalid index: data block at offset {} has size {} (smaller than the \
                     24-byte block header)",
                    db.file_offset, db.size
                )));
            }
            ranges.push((db.file_offset, db.size));
        }
        Ok(merge_ranges_bridging(ranges, BLOCK_GAP_BRIDGE))
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
    /// completes, or `{ done: false, needed: [[offset, length], ...] }` listing
    /// **all** the byte ranges the walk still needs (gathered in one pass) to
    /// fetch and feed back.
    ///
    /// This is a **thin compatibility shim** kept for existing custom drivers:
    /// every call re-marshals the whole `ranges`/`fragments` arrays from JS
    /// into a fresh [`FragmentStore`] (an `O(F)` copy each round), so a build
    /// with many rounds still does `O(passes × F)` copying overall. Prefer
    /// [`IndexBuilder`] for new code — it copies each fetched fragment into
    /// wasm memory exactly once and keeps them in the sorted store across
    /// `step()` calls, so building the index no longer re-copies previously
    /// fetched bytes on every pass. The JS `MdfIndex.fromRangeSource` wrapper
    /// drives `IndexBuilder` for this reason.
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
        let mut reader = RecordingRangeReader { store: &parsed.store, misses: Vec::new(), whole_request: false };
        let walk = MdfIndex::from_range_reader(&mut reader, file_size);

        // The walk may have finished the linked-list traversal while still
        // skipping optional leaf reads (names/units) whose bytes were not yet
        // available — those are recorded in `misses`. So a build is only truly
        // done when the walk succeeded *and* nothing was missed this pass.
        if !reader.misses.is_empty() {
            let step = BuildStepJs {
                done: false,
                json: None,
                needed: Some(misses_to_needed(reader.misses)),
            };
            return serde_wasm_bindgen::to_value(&step).map_err(|e| JsError::new(&e.to_string()));
        }

        match walk {
            Ok(index) => {
                let json = index.to_json().map_err(err_to_js)?;
                let step = BuildStepJs { done: true, json: Some(json), needed: None };
                serde_wasm_bindgen::to_value(&step).map_err(|e| JsError::new(&e.to_string()))
            }
            // No misses recorded, so this is a real parse error — surface it.
            Err(e) => Err(err_to_js(e)),
        }
    }

    /// Report the next conversion-block byte range still needed to read `name`
    /// from fragments, or that all the conversion metadata is already present.
    ///
    /// Indexes built over the network (`fromUrl`/`fromRangeSource`) record only
    /// each channel's conversion-block **location** and resolve it lazily on
    /// the first read — so the fragment readers need those bytes fetched
    /// alongside the data sections. Drive this like `buildIndexStep`: it
    /// returns `{ done: true }` once every conversion block required for the
    /// read is present in `fragments`, or `{ done: false, needed: [[offset,
    /// length], ...] }` listing the ranges to fetch and feed back. Pass
    /// `withMaster = true` for a signal read (`read`/`readFromFragments`),
    /// whose timestamps decode the group master and thus also need the
    /// master's conversion; `false` for a plain values read. For a
    /// self-contained index (built from bytes, or a channel with no
    /// conversion) it returns `done` immediately with no fetching. `raw`
    /// (default `false`) skips the channel's own conversion, for a raw read.
    #[wasm_bindgen(js_name = conversionRangesStep)]
    pub fn conversion_ranges_step(
        &self,
        name: &str,
        group: Option<String>,
        with_master: bool,
        ranges: JsValue,
        fragments: JsValue,
        raw: Option<bool>,
    ) -> Result<JsValue, JsError> {
        let (g, c) = self.locate(name, group.as_deref())?;
        let parsed = build_fragment_reader(&ranges, &fragments)?;
        let mut reader = RecordingRangeReader { store: &parsed.store, misses: Vec::new(), whole_request: false };

        // A raw read applies no conversion to the channel itself (the
        // master's is still needed for its timestamps).
        let mut targets = if raw.unwrap_or(false) { Vec::new() } else { vec![(g, c)] };
        if with_master {
            if let Some(m) = self.index.master_location(g, c) {
                targets.push(m);
            }
        }

        for (tg, idx) in targets {
            let before = reader.misses.len();
            if let Err(e) = self.index.resolve_channel_conversion(tg, idx, &mut reader) {
                // If this target recorded no gap it is a genuine error, not a
                // not-yet-fetched abort — surface it.
                if reader.misses.len() == before {
                    return Err(err_to_js(e));
                }
            }
        }

        if !reader.misses.is_empty() {
            let step = BuildStepJs {
                done: false,
                json: None,
                needed: Some(misses_to_needed(reader.misses)),
            };
            return serde_wasm_bindgen::to_value(&step).map_err(|e| JsError::new(&e.to_string()));
        }

        let step = BuildStepJs { done: true, json: None, needed: None };
        serde_wasm_bindgen::to_value(&step).map_err(|e| JsError::new(&e.to_string()))
    }

    /// `conversionRangesStep` for a whole `readMany` batch: the conversion
    /// blocks every name needs (and, with `withMaster`, their time axes'),
    /// each block once. Drive it with only the conversion fragments fetched
    /// so far — never the data — so each call stays cheap.
    #[wasm_bindgen(js_name = conversionRangesManyStep)]
    pub fn conversion_ranges_many_step(
        &self,
        names: Vec<String>,
        group: Option<String>,
        with_master: bool,
        ranges: JsValue,
        fragments: JsValue,
        raw: Option<bool>,
    ) -> Result<JsValue, JsError> {
        let targets = self.index.locate_all(&names, group.as_deref()).map_err(err_to_js)?;
        let raw = raw.unwrap_or(false);
        let mut wanted: Vec<(usize, usize)> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (g, c) in targets {
            let own = (!raw).then_some((g, c));
            let master = if with_master { self.index.master_location(g, c) } else { None };
            for (tg, tc) in own.into_iter().chain(master) {
                let channel = &self.index.channel_groups[tg].channels[tc];
                if channel.conversion.is_none() && channel.conversion_addr != 0 && seen.insert(channel.conversion_addr) {
                    wanted.push((tg, tc));
                }
            }
        }
        if wanted.is_empty() {
            return serde_wasm_bindgen::to_value(&BuildStepJs { done: true, json: None, needed: None })
                .map_err(|e| JsError::new(&e.to_string()));
        }
        let parsed = build_fragment_reader(&ranges, &fragments)?;
        let mut reader = RecordingRangeReader { store: &parsed.store, misses: Vec::new(), whole_request: false };
        for (tg, tc) in wanted {
            let before = reader.misses.len();
            if let Err(e) = self.index.resolve_channel_conversion(tg, tc, &mut reader) {
                if reader.misses.len() == before {
                    return Err(err_to_js(e));
                }
            }
        }
        let step = if reader.misses.is_empty() {
            BuildStepJs { done: true, json: None, needed: None }
        } else {
            BuildStepJs { done: false, json: None, needed: Some(misses_to_needed(reader.misses)) }
        };
        serde_wasm_bindgen::to_value(&step).map_err(|e| JsError::new(&e.to_string()))
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

    /// Metadata of every channel group (`GroupInfo[]`), in file order.
    #[wasm_bindgen(js_name = groups)]
    pub fn groups(&self) -> Result<JsValue, JsError> {
        to_js(&self.index.groups().map_err(err_to_js)?)
    }

    /// Metadata of the first group named `name`, or `undefined`.
    #[wasm_bindgen(js_name = group)]
    pub fn group(&self, name: &str) -> Result<JsValue, JsError> {
        to_js(&self.index.group(name).map_err(err_to_js)?)
    }

    /// Metadata of the first channel named `name` (in the group(s) named
    /// `group`, when given), or `undefined`.
    #[wasm_bindgen(js_name = channel)]
    pub fn channel(&self, name: &str, group: Option<String>) -> Result<JsValue, JsError> {
        to_js(&self.index.channel(name, group.as_deref()).map_err(err_to_js)?)
    }

    /// Measurement start time in nanoseconds since the Unix epoch, or
    /// `undefined` if the file header does not record one.
    #[wasm_bindgen(js_name = startTimeNs)]
    pub fn start_time_ns(&self) -> Option<u64> {
        self.index.start_time_ns
    }

    /// One step of resolving the conversion of `name` from caller-fetched
    /// fragments. Returns `{ done: true, conversion }` (`conversion` is
    /// `null` when the channel has none) once every block it needs is in
    /// `fragments` — immediately for a locally built index, which stores
    /// conversions resolved — else `{ done: false, needed: [[offset,
    /// length], ...] }` to fetch and feed back, like `conversionRangesStep`.
    #[wasm_bindgen(js_name = conversionStep)]
    pub fn conversion_step(
        &self,
        name: &str,
        group: Option<String>,
        ranges: JsValue,
        fragments: JsValue,
    ) -> Result<JsValue, JsError> {
        let (g, c) = self.locate(name, group.as_deref())?;
        let parsed = build_fragment_reader(&ranges, &fragments)?;
        let mut reader = RecordingRangeReader { store: &parsed.store, misses: Vec::new(), whole_request: false };
        let result = self.index.resolve_channel_conversion(g, c, &mut reader);
        let out = Object::new();
        let set = |k: &str, v: &JsValue| -> Result<(), JsError> {
            Reflect::set(&out, &JsValue::from_str(k), v)
                .map(|_| ())
                .map_err(|_| JsError::new("failed to build conversion step"))
        };
        if !reader.misses.is_empty() {
            set("done", &JsValue::FALSE)?;
            let needed = serde_wasm_bindgen::to_value(&misses_to_needed(reader.misses))
                .map_err(|e| JsError::new(&e.to_string()))?;
            set("needed", &needed)?;
            return Ok(out.into());
        }
        let conversion = result.map_err(err_to_js)?;
        set("done", &JsValue::TRUE)?;
        set("conversion", &conversion_to_js(conversion.as_ref())?)?;
        Ok(out.into())
    }

    /// One step of a windowed read of `name` from caller-fetched fragments.
    ///
    /// `window` is `{ startTime?, endTime? }` (seconds of the master channel,
    /// inclusive; the master is searched, not read in full) or
    /// `{ startRecord?, recordCount? }` (a record window), either with an
    /// optional `raw: true` to skip the channel's conversion. Drive it like
    /// `conversionRangesStep`: start with empty `ranges`/`fragments`; each
    /// call returns `{ done: false, needed: [[offset, length], ...] }` until
    /// every byte the read touches is present, then
    /// `{ done: true, signal, startRecord, recordCount }`. Only the selected
    /// records (plus a few single-record probes of the master and any lazily
    /// resolved conversion blocks) are ever requested.
    #[wasm_bindgen(js_name = readWindowStep)]
    pub fn read_window_step(
        &self,
        name: &str,
        group: Option<String>,
        window: JsValue,
        ranges: JsValue,
        fragments: JsValue,
    ) -> Result<JsValue, JsError> {
        let (g, c) = self.locate(name, group.as_deref())?;
        let options = read_options(None, window)?;
        let window = options.window().map_err(err_to_js)?;

        let parsed = build_fragment_reader(&ranges, &fragments)?;
        let mut reader = RecordingRangeReader { store: &parsed.store, misses: Vec::new(), whole_request: true };
        let result = (|| {
            let (start, count) = self.index.resolve_window(g, c, window, &mut reader)?;
            let signal = crate::index::read_signal_window(&self.index, g, c, start, count, options.raw, &mut reader)?;
            Ok::<_, MdfError>((start, count, signal))
        })();

        let out = Object::new();
        let set = |k: &str, v: &JsValue| -> Result<(), JsError> {
            Reflect::set(&out, &JsValue::from_str(k), v)
                .map(|_| ())
                .map_err(|_| JsError::new("failed to build window step"))
        };
        if !reader.misses.is_empty() {
            set("done", &JsValue::FALSE)?;
            let needed = serde_wasm_bindgen::to_value(&misses_to_needed(reader.misses))
                .map_err(|e| JsError::new(&e.to_string()))?;
            set("needed", &needed)?;
            return Ok(out.into());
        }
        let (start, count, signal) = result.map_err(err_to_js)?;
        set("done", &JsValue::TRUE)?;
        set("signal", &signal_to_js(signal)?)?;
        set("startRecord", &JsValue::from_f64(start as f64))?;
        set("recordCount", &JsValue::from_f64(count as f64))?;
        Ok(out.into())
    }

    /// One step of a windowed `values` read: like `readWindowStep`, but reads
    /// only the channel (not its time axis) once the window is located.
    /// Finishes with `{ done: true, values: Float64Array }`.
    #[wasm_bindgen(js_name = valuesWindowStep)]
    pub fn values_window_step(
        &self,
        name: &str,
        group: Option<String>,
        window: JsValue,
        ranges: JsValue,
        fragments: JsValue,
    ) -> Result<JsValue, JsError> {
        let (g, c) = self.locate(name, group.as_deref())?;
        let options = read_options(None, window)?;
        let window = options.window().map_err(err_to_js)?;
        let parsed = build_fragment_reader(&ranges, &fragments)?;
        let mut reader = RecordingRangeReader { store: &parsed.store, misses: Vec::new(), whole_request: true };
        let result = (|| {
            let (start, count) = self.index.resolve_window(g, c, window, &mut reader)?;
            self.index.values_f64_window(g, c, start, count, options.raw, &mut reader)
        })();
        gap_step(reader.misses, result, "values", |values| {
            Ok(Float64Array::from(values.as_slice()).into())
        })
    }

    /// One step of reading several channels at once (`readMany`), from
    /// caller-fetched fragments.
    ///
    /// `window` is as for `readWindowStep` (or `undefined` for whole
    /// channels). Drive it like `readWindowStep`: each call returns
    /// `{ done: false, needed: [[offset, length], ...] }` with the gaps of
    /// **every** channel of the batch, until it finishes with
    /// `{ done: true, signals }` (one `Signal` per name, in order). A time
    /// axis shared by several channels is searched and decoded once. Throws
    /// for an unknown name before reporting any gap.
    #[wasm_bindgen(js_name = readManyStep)]
    pub fn read_many_step(
        &self,
        names: Vec<String>,
        group: Option<String>,
        window: JsValue,
        ranges: JsValue,
        fragments: JsValue,
    ) -> Result<JsValue, JsError> {
        let options = read_options(None, window)?;
        let window = options.window().map_err(err_to_js)?;
        let targets = self.index.locate_all(&names, group.as_deref()).map_err(err_to_js)?;
        let parsed = build_fragment_reader(&ranges, &fragments)?;
        let mut reader = RecordingRangeReader { store: &parsed.store, misses: Vec::new(), whole_request: true };
        let result = self.index.read_many_at(&targets, window, options.raw, &mut reader);
        gap_step(reader.misses, result, "signals", signals_to_js)
    }

    /// One step of `timeRange`: reads the first and last record of the time
    /// axis of `name`'s group. Finishes with `{ done: true, range }`, where
    /// `range` is `{ start, end }` or `undefined` for an empty group.
    #[wasm_bindgen(js_name = timeRangeStep)]
    pub fn time_range_step(
        &self,
        name: &str,
        group: Option<String>,
        ranges: JsValue,
        fragments: JsValue,
    ) -> Result<JsValue, JsError> {
        let (g, c) = self.locate(name, group.as_deref())?;
        let parsed = build_fragment_reader(&ranges, &fragments)?;
        let mut reader = RecordingRangeReader { store: &parsed.store, misses: Vec::new(), whole_request: true };
        let result = self.index.time_range_at(g, c, &mut reader);
        gap_step(reader.misses, result, "range", |range| to_js(&range))
    }

    /// Names of every named channel across all groups (duplicates kept).
    #[wasm_bindgen(js_name = channelNames)]
    pub fn channel_names(&self) -> Vec<String> {
        self.index.channel_names().unwrap_or_default()
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
        let ranges = self.index.byte_ranges(name, group.as_deref()).map_err(err_to_js)?;
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
        let ranges = self
            .index
            .byte_ranges_for_records(name, start_record, record_count, group.as_deref())
            .map_err(err_to_js)?;
        ranges_to_js(ranges)
    }

    /// Byte ranges of every data block of the group owning `name`, merged.
    ///
    /// Returns one span per data-block fragment covering the whole block
    /// (including its 24-byte header, which the decoder verifies); blocks
    /// stored back to back are merged into one span. For a VLSD channel the
    /// spans of its `##SD` fragment chain are included too, and for an MDF 4.2
    /// remote-master group so are the shared time group's blocks — unless
    /// `withMaster` is `false` (default `true`), which a values-only read
    /// (`valuesFromFragments`) can pass to skip the time channel. These are exactly
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
        with_master: Option<bool>,
    ) -> Result<JsValue, JsError> {
        let ranges = self.signal_ranges(name, group.as_deref(), with_master.unwrap_or(true))?;
        ranges_to_js(ranges)
    }

    /// Decode a numeric channel to a `Float64Array` from caller-fetched
    /// fragments (invalid / non-numeric samples become `NaN`).
    ///
    /// `ranges` is the `[[offset, length], ...]` array previously returned by
    /// `signalByteRanges`, and `fragments` is the parallel array of
    /// `Uint8Array` byte blobs fetched for those ranges. The decoder reads
    /// whole data blocks and verifies each 24-byte block header, so the
    /// fragments must cover the blocks including their headers (`byteRanges`
    /// spans do not). `raw` (default `false`) returns the stored values,
    /// without the channel's conversion.
    #[wasm_bindgen(js_name = valuesFromFragments)]
    pub fn values_from_fragments(
        &self,
        name: &str,
        group: Option<String>,
        ranges: JsValue,
        fragments: JsValue,
        raw: Option<bool>,
    ) -> Result<Vec<f64>, JsError> {
        let reader = build_fragment_reader(&ranges, &fragments)?;
        let mut bound = self.index.open(reader);
        let options = ReadOptions { group, raw: raw.unwrap_or(false), ..ReadOptions::default() };
        bound.values(name, &options).map_err(err_to_js)
    }

    /// Decode a channel to a `{ name, unit, timestamps, values }` object from
    /// caller-fetched fragments.
    ///
    /// Pass the ranges from `signalByteRanges` (channel + master, merged) and
    /// the fragments fetched for them. `raw` (default `false`) returns the
    /// stored values, without the channel's conversion (timestamps stay
    /// physical).
    #[wasm_bindgen(js_name = readFromFragments)]
    pub fn read_from_fragments(
        &self,
        name: &str,
        group: Option<String>,
        ranges: JsValue,
        fragments: JsValue,
        raw: Option<bool>,
    ) -> Result<JsValue, JsError> {
        let reader = build_fragment_reader(&ranges, &fragments)?;
        let mut bound = self.index.open(reader);
        let options = ReadOptions { group, raw: raw.unwrap_or(false), ..ReadOptions::default() };
        signal_to_js(bound.read(name, &options).map_err(err_to_js)?)
    }
}

// ---------------------------------------------------------------------------
// IndexBuilder
// ---------------------------------------------------------------------------

/// Stateful driver for an incremental, range-fetched [`WasmMdfIndex`] build.
///
/// Preferred over the static [`WasmMdfIndex::build_index_step`] shim: fetched
/// fragments are copied from JS into wasm memory exactly once, in [`push`](Self::push),
/// and kept in a [`FragmentStore`] across every [`step`](Self::step) call
/// rather than being re-marshalled from JS on each pass. Typical use (mirrors
/// the JS `MdfIndex.fromRangeSource` loop):
///
/// 1. `new(fileSize)`, then `push(0, seedBytes)` with an initial prefix fetch.
/// 2. Call `step()`. If `done`, the finished index JSON is ready. Otherwise
///    fetch every `[offset, length]` span in `needed` (concurrently) and
///    `push` each result.
/// 3. Repeat step 2 until `done`.
/// 4. `free()` the builder once finished (or let it drop) to release its
///    wasm-side fragment memory.
#[wasm_bindgen]
pub struct IndexBuilder {
    file_size: u64,
    store: FragmentStore,
}

#[wasm_bindgen]
impl IndexBuilder {
    /// Start a new incremental build for a file of `fileSize` bytes (from a
    /// HEAD request or `RangeSource.size()`).
    #[wasm_bindgen(constructor)]
    pub fn new(file_size: f64) -> Result<IndexBuilder, JsError> {
        let file_size = f64_to_u64(Some(file_size), "file size")?;
        Ok(IndexBuilder { file_size, store: FragmentStore::new() })
    }

    /// Add fetched bytes at the given absolute file `offset`. Copies into
    /// wasm memory once; the store stays sorted for `O(log F)` lookups on the
    /// next `step()`.
    pub fn push(&mut self, offset: f64, bytes: &[u8]) -> Result<(), JsError> {
        let offset = f64_to_u64(Some(offset), "fragment offset")?;
        self.store.push(offset, bytes.to_vec()).map_err(err_to_js)?;
        Ok(())
    }

    /// Run one gap-discovery pass of the metadata walk against the fragments
    /// accumulated so far via `push`.
    ///
    /// Returns `{ done: true, json }` with the finished index once the walk
    /// completes with no misses, or `{ done: false, needed: [[offset,
    /// length], ...] }` listing **all** the byte ranges this pass still
    /// needs (gathered in one batched pass — see [`crate::parsing::reader_walk`] —
    /// rather than one gap per call) for the caller to fetch and `push` back.
    pub fn step(&mut self) -> Result<JsValue, JsError> {
        let mut reader = RecordingRangeReader { store: &self.store, misses: Vec::new(), whole_request: false };
        let walk = MdfIndex::from_range_reader(&mut reader, self.file_size);

        // Mirrors `build_index_step`: only report `done` once the walk
        // succeeded *and* this pass recorded zero misses — a pass with
        // misses is by construction partial (see `ByteRangeReader::is_probing`).
        if !reader.misses.is_empty() {
            let step = BuildStepJs {
                done: false,
                json: None,
                needed: Some(misses_to_needed(reader.misses)),
            };
            return serde_wasm_bindgen::to_value(&step).map_err(|e| JsError::new(&e.to_string()));
        }

        match walk {
            Ok(index) => {
                let json = index.to_json().map_err(err_to_js)?;
                let step = BuildStepJs { done: true, json: Some(json), needed: None };
                serde_wasm_bindgen::to_value(&step).map_err(|e| JsError::new(&e.to_string()))
            }
            // No misses recorded, so this is a real parse error — surface it.
            Err(e) => Err(err_to_js(e)),
        }
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
    /// Channel data types per open group, cached for `writeRecord`.
    record_types: HashMap<String, Vec<DataType>>,
}

impl WasmMdfWriter {
    fn writer_mut(&mut self) -> Result<&mut MdfWriter, JsError> {
        self.writer
            .as_mut()
            .ok_or_else(|| JsError::new("Writer has been finalized"))
    }
}

/// Coerce one JS value to the `DecodedValue` its channel encodes. The rules
/// live in the Rust core (`simple::record_value`), shared with Python.
fn record_value(v: &JsValue, dt: &DataType, i: usize) -> Result<DecodedValue, JsError> {
    use crate::writer::mdf_writer::simple::{record_value, PlainValue};

    let (text, bytes);
    let plain = if v.as_bool().is_some() {
        PlainValue::Bool
    } else if let Some(s) = v.as_string() {
        text = s;
        PlainValue::Text(&text)
    } else if let Some(array) = v.dyn_ref::<Uint8Array>() {
        bytes = array.to_vec();
        PlainValue::Bytes(&bytes)
    } else if let Some(b) = v.dyn_ref::<js_sys::BigInt>() {
        match (i64::try_from(b.clone()), u64::try_from(b.clone())) {
            (Ok(n), _) => PlainValue::Int(n as i128),
            (_, Ok(n)) => PlainValue::Int(n as i128),
            // Outside the 64-bit range: too big for any integer channel, but
            // a float channel takes it like any other number.
            _ => PlainValue::Float(
                b.to_string(10)
                    .ok()
                    .and_then(|s| String::from(s).parse::<f64>().ok())
                    .unwrap_or(f64::NAN),
            ),
        }
    } else if let Some(f) = v.as_f64() {
        PlainValue::Float(f)
    } else {
        PlainValue::Other
    };
    record_value(plain, dt, i).map_err(err_to_js)
}

#[wasm_bindgen(js_class = MdfWriter)]
impl WasmMdfWriter {
    /// Create an in-memory writer. No MDF blocks are written until
    /// `initMdfFile` is called.
    #[wasm_bindgen(constructor)]
    pub fn new() -> WasmMdfWriter {
        let sink = Rc::new(RefCell::new(Cursor::new(Vec::new())));
        let writer = MdfWriter::new_from_writer(SharedBuf(Rc::clone(&sink)));
        WasmMdfWriter { writer: Some(writer), sink, record_types: HashMap::new() }
    }

    /// Write the identification (`##ID`) and header (`##HD`) blocks. Call once,
    /// before adding any channel group.
    #[wasm_bindgen(js_name = initMdfFile)]
    pub fn init_mdf_file(&mut self) -> Result<(), JsError> {
        self.writer_mut()?.init_mdf_file().map_err(err_to_js)?;
        Ok(())
    }

    /// Set the measurement start time (nanoseconds since the Unix epoch, UTC).
    #[wasm_bindgen(js_name = setStartTime)]
    pub fn set_start_time(&mut self, abs_time_ns: u64) -> Result<(), JsError> {
        self.writer_mut()?.set_start_time(abs_time_ns).map_err(err_to_js)
    }

    /// Add a channel group and return its id.
    #[wasm_bindgen(js_name = addChannelGroup)]
    pub fn add_channel_group(&mut self, name: Option<String>) -> Result<String, JsError> {
        self.writer_mut()?.add_channel_group(name.as_deref()).map_err(err_to_js)
    }

    /// Set a channel group's comment.
    #[wasm_bindgen(js_name = setChannelGroupComment)]
    pub fn set_channel_group_comment(&mut self, group_id: &str, comment: &str) -> Result<(), JsError> {
        self.writer_mut()?.set_channel_group_comment(group_id, comment).map_err(err_to_js)
    }

    /// Add a channel of the given canonical data type (e.g. `"FloatLE"`).
    #[wasm_bindgen(js_name = addChannel)]
    pub fn add_channel(&mut self, group_id: &str, name: &str, data_type: &str) -> Result<String, JsError> {
        let data_type = DataType::from_name(data_type).map_err(err_to_js)?;
        self.writer_mut()?.add_channel(group_id, name, data_type).map_err(err_to_js)
    }

    /// Add a variable-length UTF-8 string channel.
    #[wasm_bindgen(js_name = addStringChannel)]
    pub fn add_string_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        self.writer_mut()?.add_string_channel(group_id, name).map_err(err_to_js)
    }

    /// Add a 64-bit float channel marked as the group's master (time) channel.
    #[wasm_bindgen(js_name = addTimeChannel)]
    pub fn add_time_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        self.writer_mut()?.add_time_channel(group_id, name).map_err(err_to_js)
    }

    /// Add a 64-bit float channel.
    #[wasm_bindgen(js_name = addFloatChannel)]
    pub fn add_float_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        self.writer_mut()?.add_float_channel(group_id, name).map_err(err_to_js)
    }

    /// Add a 32-bit float channel.
    #[wasm_bindgen(js_name = addFloat32Channel)]
    pub fn add_float32_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        self.writer_mut()?.add_float32_channel(group_id, name).map_err(err_to_js)
    }

    /// Add a 64-bit unsigned integer channel.
    #[wasm_bindgen(js_name = addIntChannel)]
    pub fn add_int_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        self.writer_mut()?.add_int_channel(group_id, name).map_err(err_to_js)
    }

    /// Add a 64-bit signed integer channel.
    #[wasm_bindgen(js_name = addSintChannel)]
    pub fn add_sint_channel(&mut self, group_id: &str, name: &str) -> Result<String, JsError> {
        self.writer_mut()?.add_sint_channel(group_id, name).map_err(err_to_js)
    }

    /// Mark an existing channel as its group's master (time) channel.
    #[wasm_bindgen(js_name = setTimeChannel)]
    pub fn set_time_channel(&mut self, channel_id: &str) -> Result<(), JsError> {
        self.writer_mut()?.set_time_channel(channel_id).map_err(err_to_js)
    }

    /// Attach a linear conversion (`physical = offset + factor * raw`).
    #[wasm_bindgen(js_name = addLinearConversion)]
    pub fn add_linear_conversion(&mut self, channel_id: &str, offset: f64, factor: f64) -> Result<(), JsError> {
        self.writer_mut()?
            .add_linear_conversion(channel_id, offset, factor)
            .map_err(err_to_js)
    }

    /// Attach a value-to-text conversion; `mapping` is `[value, text][]`.
    #[wasm_bindgen(js_name = addValueToTextConversion)]
    pub fn add_value_to_text_conversion(
        &mut self,
        channel_id: &str,
        mapping: JsValue,
        default_text: &str,
    ) -> Result<(), JsError> {
        let pairs: Vec<(f64, String)> = serde_wasm_bindgen::from_value(mapping)
            .map_err(|_| JsError::new("mapping must be an array of [value, text] pairs"))?;
        let mut ints = Vec::with_capacity(pairs.len());
        for (v, text) in &pairs {
            if !v.is_finite() || v.fract() != 0.0 || v.abs() > MAX_SAFE_INTEGER as f64 {
                return Err(JsError::new(&format!("mapping value {} must be a safe integer", v)));
            }
            ints.push((*v as i64, text.as_str()));
        }
        self.writer_mut()?
            .add_value_to_text_conversion(channel_id, &ints, default_text)
            .map_err(err_to_js)
    }

    /// Open the data block of a channel group.
    #[wasm_bindgen(js_name = startDataBlock)]
    pub fn start_data_block(&mut self, group_id: &str) -> Result<(), JsError> {
        let types = self.writer_mut()?.channel_data_types(group_id).map_err(err_to_js)?;
        self.writer_mut()?.start_data_block(group_id).map_err(err_to_js)?;
        self.record_types.insert(group_id.to_string(), types);
        Ok(())
    }

    /// Append one record: one plain value per channel, in channel order.
    #[wasm_bindgen(js_name = writeRecord)]
    pub fn write_record(&mut self, group_id: &str, values: JsValue) -> Result<(), JsError> {
        let types = self
            .record_types
            .get(group_id)
            .ok_or_else(|| JsError::new(&format!("no open data block for group '{group_id}'")))?;
        let arr = values
            .dyn_ref::<Array>()
            .ok_or_else(|| JsError::new("record values must be an array"))?;
        if arr.length() as usize != types.len() {
            return Err(JsError::new(&format!(
                "expected {} values for this group, got {}",
                types.len(),
                arr.length()
            )));
        }
        let record = types
            .iter()
            .enumerate()
            .map(|(i, dt)| record_value(&arr.get(i as u32), dt, i))
            .collect::<Result<Vec<_>, _>>()?;
        self.writer_mut()?.write_record(group_id, &record).map_err(err_to_js)
    }

    /// Append many records at once from one typed array per channel
    /// (`Float64Array`, `Float32Array`, `BigUint64Array` or `BigInt64Array`,
    /// matching the channel), all of the same length.
    #[wasm_bindgen(js_name = writeColumns)]
    pub fn write_columns(&mut self, group_id: &str, columns: JsValue) -> Result<(), JsError> {
        enum Owned {
            F64(Vec<f64>),
            F32(Vec<f32>),
            U64(Vec<u64>),
            I64(Vec<i64>),
        }
        let arr = columns
            .dyn_ref::<Array>()
            .ok_or_else(|| JsError::new("columns must be an array of typed arrays"))?;
        let mut owned = Vec::with_capacity(arr.length() as usize);
        for (i, col) in arr.iter().enumerate() {
            owned.push(if let Some(a) = col.dyn_ref::<Float64Array>() {
                Owned::F64(a.to_vec())
            } else if let Some(a) = col.dyn_ref::<js_sys::Float32Array>() {
                Owned::F32(a.to_vec())
            } else if let Some(a) = col.dyn_ref::<js_sys::BigUint64Array>() {
                Owned::U64(a.to_vec())
            } else if let Some(a) = col.dyn_ref::<js_sys::BigInt64Array>() {
                Owned::I64(a.to_vec())
            } else {
                return Err(JsError::new(&format!(
                    "column {i} must be a Float64Array, Float32Array, BigUint64Array or BigInt64Array"
                )));
            });
        }
        let data: Vec<crate::writer::ColumnData<'_>> = owned
            .iter()
            .map(|c| match c {
                Owned::F64(v) => crate::writer::ColumnData::F64(v),
                Owned::F32(v) => crate::writer::ColumnData::F32(v),
                Owned::U64(v) => crate::writer::ColumnData::U64(v),
                Owned::I64(v) => crate::writer::ColumnData::I64(v),
            })
            .collect();
        self.writer_mut()?.write_columns(group_id, &data).map_err(err_to_js)
    }

    /// Close a channel group's data block.
    #[wasm_bindgen(js_name = finishDataBlock)]
    pub fn finish_data_block(&mut self, group_id: &str) -> Result<(), JsError> {
        self.record_types.remove(group_id);
        self.writer_mut()?.finish_data_block(group_id).map_err(err_to_js)
    }

    /// Finish the file and return its bytes. The writer cannot be used afterwards.
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

// ---------------------------------------------------------------------------
// Column-oriented conversion
// ---------------------------------------------------------------------------

/// `ColumnOptions` as passed from JS (`{ blockSize, memoryBudget,
/// readChunkSize, shareIdenticalTime }`, all optional).
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ColumnOptionsJs {
    block_size: Option<f64>,
    memory_budget: Option<f64>,
    read_chunk_size: Option<f64>,
    share_identical_time: Option<bool>,
}

fn column_options(options: JsValue) -> Result<crate::column::ColumnOptions, JsError> {
    let js: ColumnOptionsJs = if options.is_undefined() || options.is_null() {
        ColumnOptionsJs::default()
    } else {
        serde_wasm_bindgen::from_value(options).map_err(|e| JsError::new(&e.to_string()))?
    };
    let d = crate::column::ColumnOptions::default();
    let size = |v: Option<f64>, what: &str, default: u64| -> Result<u64, JsError> {
        match v {
            None => Ok(default),
            Some(v) => f64_to_u64(Some(v), what),
        }
    };
    Ok(crate::column::ColumnOptions {
        block_size: size(js.block_size, "blockSize", d.block_size)?,
        memory_budget: size(js.memory_budget, "memoryBudget", d.memory_budget)?,
        read_chunk_size: size(js.read_chunk_size, "readChunkSize", d.read_chunk_size)?,
        share_identical_time: js.share_identical_time.unwrap_or(d.share_identical_time),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ColumnReportJs {
    source_groups: usize,
    time_groups: usize,
    signal_groups: usize,
    shared_time_groups: usize,
    data_blocks: usize,
    metadata_size: f64,
    file_size: f64,
    skipped_channels: Vec<String>,
}

fn column_report_to_js(r: crate::column::ColumnReport) -> Result<JsValue, JsError> {
    let js = ColumnReportJs {
        source_groups: r.source_groups,
        time_groups: r.time_groups,
        signal_groups: r.signal_groups,
        shared_time_groups: r.shared_time_groups,
        data_blocks: r.data_blocks,
        metadata_size: r.metadata_size as f64,
        file_size: r.file_size as f64,
        skipped_channels: r.skipped_channels,
    };
    serde_wasm_bindgen::to_value(&js).map_err(|e| JsError::new(&e.to_string()))
}

/// Convert an in-memory MDF file to a column-oriented MDF 4.20 file
/// (one signal per channel group, shared time channels, metadata first).
///
/// Returns `{ bytes: Uint8Array, report }`. Both the input and the output are
/// held in memory; use `toColumnOrientedStream` to stream large files.
#[wasm_bindgen(js_name = toColumnOriented)]
pub fn to_column_oriented(data: &[u8], options: JsValue) -> Result<JsValue, JsError> {
    let options = column_options(options)?;
    let (bytes, report) = crate::column::to_column_oriented_bytes(data, &options).map_err(err_to_js)?;
    let out = Object::new();
    Reflect::set(&out, &"bytes".into(), &Uint8Array::from(bytes.as_slice()).into())
        .map_err(|_| JsError::new("failed to build result object"))?;
    Reflect::set(&out, &"report".into(), &column_report_to_js(report)?)
        .map_err(|_| JsError::new("failed to build result object"))?;
    Ok(out.into())
}

/// [`ByteRangeReader`] over a synchronous JS callback
/// `read(offset: number, length: number) => Uint8Array`.
struct JsCallbackReader<'a> {
    read: &'a js_sys::Function,
    size: u64,
}

impl ByteRangeReader for JsCallbackReader<'_> {
    type Error = MdfError;

    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, MdfError> {
        if offset.checked_add(length).is_none_or(|end| end > self.size) {
            return Err(MdfError::TooShortBuffer {
                actual: self.size.min(usize::MAX as u64) as usize,
                expected: offset.saturating_add(length).min(usize::MAX as u64) as usize,
                file: file!(),
                line: line!(),
            });
        }
        let result = self
            .read
            .call2(&JsValue::NULL, &JsValue::from_f64(offset as f64), &JsValue::from_f64(length as f64))
            .map_err(|e| MdfError::BlockSerializationError(format!("read callback threw: {:?}", e)))?;
        let array: Uint8Array = result
            .dyn_into()
            .map_err(|_| MdfError::BlockSerializationError("read callback must return a Uint8Array".into()))?;
        if array.length() as u64 != length {
            return Err(MdfError::BlockSerializationError(format!(
                "read callback returned {} bytes for a {}-byte request at offset {}",
                array.length(),
                length,
                offset
            )));
        }
        Ok(array.to_vec())
    }
}

/// [`crate::column::ColumnSink`] over a synchronous JS callback
/// `write(offset: number, bytes: Uint8Array) => void`.
struct JsCallbackSink<'a> {
    write: &'a js_sys::Function,
}

impl crate::column::ColumnSink for JsCallbackSink<'_> {
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<(), MdfError> {
        if offset + bytes.len() as u64 > MAX_SAFE_INTEGER {
            return Err(MdfError::BlockSerializationError("output exceeds Number.MAX_SAFE_INTEGER".into()));
        }
        self.write
            .call2(&JsValue::NULL, &JsValue::from_f64(offset as f64), &Uint8Array::from(bytes).into())
            .map_err(|e| MdfError::BlockSerializationError(format!("write callback threw: {:?}", e)))?;
        Ok(())
    }
}

/// Streaming column-oriented conversion driven by synchronous callbacks.
///
/// `read(offset, length)` must return a `Uint8Array` of exactly `length`
/// bytes from the source (of `size` bytes); `write(offset, bytes)` must store
/// `bytes` at absolute position `offset` of the output (positions never
/// written must read as zero, as in a fresh file). The source is read
/// sequentially in `readChunkSize` pieces and memory stays bounded by
/// `memoryBudget`, so arbitrarily large files can be converted, e.g. with
/// Node's `fs.readSync`/`fs.writeSync` or an OPFS `FileSystemSyncAccessHandle`.
/// Returns the conversion report.
#[wasm_bindgen(js_name = toColumnOrientedStream)]
pub fn to_column_oriented_stream(
    size: f64,
    read: &js_sys::Function,
    write: &js_sys::Function,
    options: JsValue,
) -> Result<JsValue, JsError> {
    let size = f64_to_u64(Some(size), "size")?;
    let options = column_options(options)?;
    let mut reader = JsCallbackReader { read, size };
    let mut sink = JsCallbackSink { write };
    let report =
        crate::column::to_column_oriented_with(&mut reader, &mut sink, &options).map_err(err_to_js)?;
    column_report_to_js(report)
}
