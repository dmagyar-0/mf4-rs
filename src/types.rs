//! The plain data types of the cross-language API contract.
//!
//! Every binding (Rust, Python, TypeScript) exposes these with the same
//! fields — only the casing differs (`record_count` / `recordCount`). See
//! `docs/API.md` for the contract and `docs/api-contract.json` for the
//! machine-readable list the parity tests check.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::blocks::conversion::ConversionBlock;
use crate::error::MdfError;
use crate::signal::Signal;

/// Metadata of one channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelInfo {
    /// Channel name.
    pub name: Option<String>,
    /// Physical unit.
    pub unit: Option<String>,
    /// Channel comment (`##TX` text, or the raw `##MD` XML).
    pub comment: Option<String>,
    /// Canonical data-type name, e.g. `"FloatLE"` (see
    /// [`DataType::name`](crate::blocks::common::DataType::name)).
    pub data_type: String,
    /// Width of the stored value in bits.
    pub bit_count: u32,
    /// The channel is its group's master (time) channel.
    pub is_master: bool,
    /// Variable-length (VLSD) channel, e.g. strings.
    pub is_vlsd: bool,
    /// The channel carries a conversion (applied by `read` / `values`).
    pub has_conversion: bool,
}

/// Metadata of one channel group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfo {
    /// Group (acquisition) name.
    pub name: Option<String>,
    /// Group comment.
    pub comment: Option<String>,
    /// Number of records (samples per channel).
    pub record_count: u64,
    /// Names of the group's named channels, in file order.
    pub channel_names: Vec<String>,
    /// Name of the group's master (time) channel — local, or the remote master
    /// of an MDF 4.2 column-oriented group.
    pub master_channel: Option<String>,
    /// The group's channels, in file order.
    pub channels: Vec<ChannelInfo>,
}

impl GroupInfo {
    /// Find one of this group's channels by name.
    pub fn channel(&self, name: &str) -> Option<&ChannelInfo> {
        self.channels.iter().find(|c| c.name.as_deref() == Some(name))
    }
}

/// First and last master (time) value of a channel's group.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: f64,
    pub end: f64,
}

/// A channel's conversion (raw → physical value), fully resolved.
///
/// `type` is one of `Identity`, `Linear`, `Rational`, `Algebraic`,
/// `TableLookupInterp`, `TableLookupNoInterp`, `RangeLookup`, `ValueToText`,
/// `RangeToText`, `TextToValue`, `TextToText`, `BitfieldText`. `values` holds
/// the block's parameters (`cc_val`): `[offset, factor]` for `Linear`
/// (`phys = values[0] + values[1] * raw`), the six rational coefficients, or
/// the table keys. `texts` / `refs` are keyed by the position in the block's
/// reference list; how they pair with `values` depends on `type` (MDF 4.1
/// §6.17) — for `ValueToText`, `values[i]` maps to `texts[i]` (or the nested
/// `refs[i]`) and index `values.len()` is the default.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionInfo {
    /// Conversion type name.
    #[serde(rename = "type")]
    pub conversion_type: String,
    /// The block's numeric parameters (`cc_val`).
    pub values: Vec<f64>,
    /// Decimal places to display.
    pub precision: u8,
    /// Raw `cc_flags`.
    pub flags: u16,
    /// `[min, max]` physical range, when the block declares one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_range: Option<[f64; 2]>,
    /// Formula of an `Algebraic` conversion (variable `X`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// Text references, keyed by reference index.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub texts: BTreeMap<usize, String>,
    /// Nested conversions, keyed by reference index.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub refs: BTreeMap<usize, ConversionInfo>,
    /// Default (fallback) conversion, when it is itself a conversion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Box<ConversionInfo>>,
}

impl From<&ConversionBlock> for ConversionInfo {
    fn from(cc: &ConversionBlock) -> Self {
        ConversionInfo {
            conversion_type: format!("{:?}", cc.cc_type),
            values: cc.cc_val.clone(),
            precision: cc.cc_precision,
            flags: cc.cc_flags,
            physical_range: cc.cc_phy_range_min.zip(cc.cc_phy_range_max).map(|(a, b)| [a, b]),
            formula: cc.formula.clone(),
            texts: cc.resolved_texts.iter().flatten().map(|(k, v)| (*k, v.clone())).collect(),
            refs: cc
                .resolved_conversions
                .iter()
                .flatten()
                .map(|(k, v)| (*k, ConversionInfo::from(v.as_ref())))
                .collect(),
            default: cc.default_conversion.as_deref().map(|d| Box::new(d.into())),
        }
    }
}

/// Options for `read` / `values`: which group to look in, and which part of
/// the channel to read.
///
/// Select **either** a time window (`start_time` / `end_time`, inclusive, in
/// the master channel's unit — seconds relative to the measurement start for
/// a time master) **or** a record window (`start_record` / `record_count`).
/// Omitted bounds are open.
///
/// ```
/// use mf4_rs::ReadOptions;
/// let opts = ReadOptions::new().group("Engine").time(Some(1.0), Some(2.5));
/// let page = ReadOptions::new().records(1_000, 500);
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadOptions {
    /// Group name, to disambiguate a channel name used by several groups.
    pub group: Option<String>,
    /// Inclusive lower time bound.
    pub start_time: Option<f64>,
    /// Inclusive upper time bound.
    pub end_time: Option<f64>,
    /// First record of a record window (0-based).
    pub start_record: Option<u64>,
    /// Number of records in a record window.
    pub record_count: Option<u64>,
}

/// A validated [`ReadOptions`] window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Window {
    /// The whole channel.
    All,
    /// Samples whose master value lies in `[start, end]`.
    Time(Option<f64>, Option<f64>),
    /// Records `[start, start + count)`; `None` count = to the end.
    Records(u64, Option<u64>),
}

impl ReadOptions {
    /// Options selecting the whole channel, first match across all groups.
    pub fn new() -> Self {
        Self::default()
    }

    /// Restrict the lookup to the group(s) with this name.
    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    /// Select samples whose master (time) value lies in `[start, end]`.
    pub fn time(mut self, start: Option<f64>, end: Option<f64>) -> Self {
        self.start_time = start;
        self.end_time = end;
        self
    }

    /// Select records `[start, start + count)`.
    pub fn records(mut self, start: u64, count: u64) -> Self {
        self.start_record = Some(start);
        self.record_count = Some(count);
        self
    }

    /// Build options from the flat optional arguments the bindings take.
    pub fn from_parts(
        group: Option<&str>,
        start_time: Option<f64>,
        end_time: Option<f64>,
        start_record: Option<u64>,
        record_count: Option<u64>,
    ) -> Self {
        ReadOptions {
            group: group.map(str::to_string),
            start_time,
            end_time,
            start_record,
            record_count,
        }
    }

    /// The group filter as `Option<&str>`.
    pub(crate) fn group_name(&self) -> Option<&str> {
        self.group.as_deref()
    }

    /// Validate and classify the requested window.
    pub(crate) fn window(&self) -> Result<Window, MdfError> {
        let by_time = self.start_time.is_some() || self.end_time.is_some();
        let by_record = self.start_record.is_some() || self.record_count.is_some();
        if by_time && by_record {
            return Err(MdfError::InvalidArgument(
                "select either a time window (start_time/end_time) or a record window \
                 (start_record/record_count), not both"
                    .to_string(),
            ));
        }
        if by_time {
            for (label, v) in [("start_time", self.start_time), ("end_time", self.end_time)] {
                if v.is_some_and(f64::is_nan) {
                    return Err(MdfError::InvalidArgument(format!("{label} must not be NaN")));
                }
            }
            return Ok(Window::Time(self.start_time, self.end_time));
        }
        if by_record {
            return Ok(Window::Records(self.start_record.unwrap_or(0), self.record_count));
        }
        Ok(Window::All)
    }
}

/// Resolve a record window against `total` records, returning
/// `(start, count)`. Errors when the window reaches past the last record.
pub(crate) fn resolve_records(start: u64, count: Option<u64>, total: u64) -> Result<(u64, u64), MdfError> {
    let count = count.unwrap_or_else(|| total.saturating_sub(start));
    match start.checked_add(count) {
        Some(end) if end <= total => Ok((start, count)),
        _ => Err(MdfError::InvalidArgument(format!(
            "record window starting at {start} for {count} records exceeds the group's {total} records"
        ))),
    }
}

/// Slice an already decoded [`Signal`] to a window (the in-memory reader's
/// path; the index fetches only the window's bytes instead).
///
/// A time window searches the signal's time axis: its `timestamps`, or its own
/// values when it is the master (`is_master`).
pub(crate) fn slice_signal(mut signal: Signal, window: Window, is_master: bool) -> Result<Signal, MdfError> {
    let total = signal.values.len() as u64;
    let (start, count) = match window {
        Window::All => return Ok(signal),
        Window::Records(start, count) => resolve_records(start, count, total)?,
        Window::Time(t0, t1) => {
            let axis: Vec<f64> = if !signal.timestamps.is_empty() {
                signal.timestamps.clone()
            } else if is_master {
                signal.values_f64()
            } else if signal.values.is_empty() {
                Vec::new()
            } else {
                return Err(MdfError::InvalidArgument(format!(
                    "channel '{}' has no master (time) channel; cannot select by time",
                    signal.name
                )));
            };
            let lo = t0.map_or(0, |t| axis.partition_point(|v| *v < t));
            let hi = t1.map_or(axis.len(), |t| axis.partition_point(|v| *v <= t));
            (lo as u64, hi.saturating_sub(lo) as u64)
        }
    };
    let (s, e) = (start as usize, (start + count) as usize);
    signal.values = signal.values[s..e].to_vec();
    if !signal.timestamps.is_empty() {
        signal.timestamps = signal.timestamps[s..e].to_vec();
    }
    Ok(signal)
}
