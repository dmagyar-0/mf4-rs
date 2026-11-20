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

/// Physical groups that make up the logical group `first` — shared by `Mdf`
/// and `MdfIndex` so both answer `group(name)` and group-filtered lookups
/// alike.
///
/// `names[i]` is the name of physical group `i`; `time_groups[i]` the group
/// holding its time axis (itself for a local master, the referenced group for
/// an MDF 4.2 remote master, `None` when untimed).
///
/// A column-oriented file splits one source group into one physical group per
/// signal, all carrying the source group's name and pointing at a shared time
/// group — which may carry *another* source group's name when identical time
/// axes were merged. The logical group is then: that time group (if it has a
/// different name) followed by every group with `first`'s name that shares
/// its time axis. A row-oriented group (local master, or untimed) is just
/// itself.
pub(crate) fn logical_group_members(names: &[Option<String>], time_groups: &[Option<usize>], first: usize) -> Vec<usize> {
    let Some(time_group) = time_groups[first] else {
        return vec![first];
    };
    let mut members = Vec::new();
    if names[time_group] != names[first] {
        members.push(time_group);
    }
    members.extend(
        (0..names.len()).filter(|&h| names[h] == names[first] && time_groups[h] == Some(time_group)),
    );
    members
}

/// Merge the [`GroupInfo`] of `first` with those of the other `members` of
/// its logical group (see [`logical_group_members`]): the channels of all
/// members, in member order; every other field is `first`'s.
pub(crate) fn merge_group_infos(first: GroupInfo, members: impl IntoIterator<Item = GroupInfo>) -> GroupInfo {
    let mut merged = GroupInfo { channel_names: Vec::new(), channels: Vec::new(), ..first };
    for info in members {
        merged.channel_names.extend(info.channel_names);
        merged.channels.extend(info.channels);
    }
    merged
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
    #[serde(skip_serializing_if = "BTreeMap::is_empty", serialize_with = "string_keys")]
    pub texts: BTreeMap<usize, String>,
    /// Nested conversions, keyed by reference index.
    #[serde(skip_serializing_if = "BTreeMap::is_empty", serialize_with = "string_keys")]
    pub refs: BTreeMap<usize, ConversionInfo>,
    /// Default (fallback) conversion, when it is itself a conversion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Box<ConversionInfo>>,
}

/// Serialize reference-index maps with string keys, so the JSON / JS object
/// form is `{ "0": ..., "1": ... }`.
fn string_keys<V: Serialize, S: serde::Serializer>(map: &BTreeMap<usize, V>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_map(map.iter().map(|(k, v)| (k.to_string(), v)))
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
/// let stored = ReadOptions::new().raw(true); // skip the conversion
/// ```
///
/// `raw` returns the values as stored in the file, without the channel's
/// conversion (a linear channel gives its stored numbers, a value-to-text
/// channel its keys instead of the texts). Only the requested channel is
/// affected: `timestamps` and time windows still use the physical (converted)
/// time axis. `unit` is the channel's (physical) unit, unchanged.
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
    /// Return the stored values, without applying the channel's conversion.
    pub raw: bool,
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

    /// Return the values as stored, without the channel's conversion.
    pub fn raw(mut self, raw: bool) -> Self {
        self.raw = raw;
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
            raw: false,
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
    // A (malformed) file may give the master a different record count than
    // the channel; only the records both have can be selected.
    let total = if signal.timestamps.is_empty() {
        signal.values.len()
    } else {
        signal.values.len().min(signal.timestamps.len())
    };
    let (start, count) = match window {
        Window::All => return Ok(signal),
        Window::Records(start, count) => resolve_records(start, count, total as u64)?,
        Window::Time(t0, t1) => {
            let own_values;
            let axis: &[f64] = if !signal.timestamps.is_empty() {
                &signal.timestamps[..total]
            } else if is_master {
                own_values = signal.values_f64();
                &own_values
            } else if signal.values.is_empty() {
                &[]
            } else {
                return Err(MdfError::InvalidArgument(format!(
                    "channel '{}' has no master (time) channel; cannot select by time",
                    signal.name
                )));
            };
            // The same search the index runs over its source, so `Mdf` and
            // `MdfIndex` select exactly the same records (and reject the same
            // non-monotonic time axes).
            search_time_window(axis.len() as u64, t0, t1, |i| Ok(axis[i as usize]))?
        }
    };
    let (s, e) = (start as usize, (start + count) as usize);
    signal.values.truncate(e);
    signal.values.drain(..s);
    if !signal.timestamps.is_empty() {
        signal.timestamps.truncate(e);
        signal.timestamps.drain(..s);
    }
    Ok(signal)
}

/// Records probed per round of [`search_time_window`].
const TIME_SEARCH_FANOUT: u64 = 16;

/// Record window `(start, count)` of a time axis of `n` records whose values
/// lie in `[t_start, t_end]` (either bound optional), reading values only
/// through `probe(record)`.
///
/// The axis must be non-decreasing, as MDF requires of a time master. It is
/// searched, not scanned: each round probes up to [`TIME_SEARCH_FANOUT`]
/// records per bound, so a window costs about `log16(n)` rounds. Every probed
/// value is checked against the others; a pair out of order (a time axis that
/// resets or jumps backwards) is refused with [`MdfError::InvalidArgument`]
/// rather than silently selecting an arbitrary part of the data. `NaN` (an
/// invalid time sample) is not checked.
///
/// Both bounds' probes of a round are attempted before the first error is
/// returned, so a probing reader (the batched / wasm fragment drivers)
/// records every gap of a round in one pass. `Mdf` (whole axis in memory) and
/// `MdfIndex` (probes fetched from the source) share this function, so they
/// select identical records.
pub(crate) fn search_time_window(
    n: u64,
    t_start: Option<f64>,
    t_end: Option<f64>,
    mut probe: impl FnMut(u64) -> Result<f64, MdfError>,
) -> Result<(u64, u64), MdfError> {
    // first record with t >= t_start; first record with t > t_end
    let mut lo = Bound::new(n);
    let mut hi = Bound::new(n);
    if t_start.is_none() {
        lo.done_at(0);
    }
    if t_end.is_none() {
        hi.done_at(n);
    }
    let mut seen: Vec<(u64, f64)> = Vec::new();
    while !(lo.done() && hi.done()) {
        let mut first_err = None;
        for (bound, below) in [
            (&mut lo, &(|v: f64| t_start.is_some_and(|t| v < t)) as &dyn Fn(f64) -> bool),
            (&mut hi, &|v: f64| t_end.is_some_and(|t| v <= t)),
        ] {
            if bound.done() {
                continue;
            }
            let probes = bound.probes();
            let mut below_flags = Vec::with_capacity(probes.len());
            for &p in &probes {
                match probe(p) {
                    Ok(v) => {
                        seen.push((p, v));
                        below_flags.push(below(v));
                    }
                    Err(e) => {
                        first_err.get_or_insert(e);
                    }
                }
            }
            if below_flags.len() == probes.len() {
                bound.narrow(&probes, &below_flags);
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        check_non_decreasing(&mut seen)?;
    }
    let (start, end) = (lo.lo, hi.lo.max(lo.lo));
    Ok((start, end - start))
}

/// Refuse a time axis whose probed values are out of order.
fn check_non_decreasing(seen: &mut Vec<(u64, f64)>) -> Result<(), MdfError> {
    seen.sort_by_key(|&(i, _)| i);
    seen.dedup_by_key(|&mut (i, _)| i);
    let mut prev: Option<(u64, f64)> = None;
    for &(i, v) in seen.iter() {
        if v.is_nan() {
            continue;
        }
        if let Some((pi, pv)) = prev
            && v < pv
        {
            return Err(MdfError::InvalidArgument(format!(
                "the time axis is not monotonic (record {pi} is {pv}, record {i} is {v}); \
                 a time window needs a non-decreasing master channel — select a record \
                 window (start_record / record_count) instead"
            )));
        }
        prev = Some((i, v));
    }
    Ok(())
}

/// State of one partition-point search over `[lo, hi)`: the answer is the
/// first record for which the predicate ("still below the bound") is false.
struct Bound {
    lo: u64,
    hi: u64,
}

impl Bound {
    fn new(n: u64) -> Self {
        Bound { lo: 0, hi: n }
    }

    fn done(&self) -> bool {
        self.lo >= self.hi
    }

    fn done_at(&mut self, at: u64) {
        self.lo = at;
        self.hi = at;
    }

    /// Up to [`TIME_SEARCH_FANOUT`] distinct, increasing records in `[lo, hi)`.
    fn probes(&self) -> Vec<u64> {
        let span = self.hi - self.lo;
        if span <= TIME_SEARCH_FANOUT {
            return (self.lo..self.hi).collect();
        }
        (0..TIME_SEARCH_FANOUT)
            .map(|i| self.lo + (i as u128 * span as u128 / TIME_SEARCH_FANOUT as u128) as u64)
            .collect()
    }

    /// Narrow to the gap between the last probe below the bound and the first
    /// one not below it (values are non-decreasing).
    fn narrow(&mut self, probes: &[u64], below: &[bool]) {
        match below.iter().position(|b| !b) {
            Some(0) => self.hi = probes[0],
            Some(j) => {
                self.lo = probes[j - 1] + 1;
                self.hi = probes[j];
            }
            None => self.lo = probes[probes.len() - 1] + 1,
        }
    }
}
