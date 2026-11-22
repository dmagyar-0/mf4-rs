use crate::api::channel::{Channel, PreparedChannel};
use crate::api::channel_group::ChannelGroup;
use crate::block_layout::FileLayout;
use crate::error::MdfError;
use crate::parsing::mdf_file::MdfFile;
use crate::signal::Signal;
use crate::types::{
    logical_group_members, merge_group_infos, resolve_records, search_time_window, ChannelInfo, ConversionInfo,
    GroupInfo, ReadOptions, TimeRange, Window,
};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Decoded time axes of one `read_many` call, by (master block address,
/// first record, end record).
type AxisCache = HashMap<(usize, u64, u64), Vec<f64>>;

/// Names and time-axis links of every physical group, read once on the first
/// lookup — otherwise every lookup re-reads every name block it passes.
#[derive(Debug)]
struct Lookup {
    /// Per group: its name; `None` when its name block cannot be read (the
    /// lookup that reaches it re-reads it to report the error).
    group_names: Vec<Option<Option<String>>>,
    /// Per group, per channel: its name; `None` when unreadable.
    channel_names: Vec<Vec<Option<Option<String>>>>,
    /// Per group: the group holding its time axis — itself for a local
    /// master, the referenced group for an MDF 4.2 remote master, `None`
    /// when untimed.
    time_groups: Vec<Option<usize>>,
}

impl Lookup {
    fn new(groups: &[ChannelGroup<'_>]) -> Self {
        let has_master = |g: &ChannelGroup<'_>| {
            g.raw_channel_group().raw_channels.iter().any(|c| matches!(c.block.channel_type, 2 | 3))
        };
        let by_addr: HashMap<u64, usize> =
            groups.iter().enumerate().rev().map(|(i, g)| (g.raw_channel_group().addr, i)).collect();
        let time_groups = (0..groups.len())
            .map(|i| {
                if has_master(&groups[i]) {
                    return Some(i);
                }
                let target = groups[i].raw_channel_group().block.master_cg_addr;
                if target == 0 {
                    return None;
                }
                by_addr.get(&target).copied().filter(|&m| m != i && has_master(&groups[m]))
            })
            .collect();
        Lookup {
            group_names: groups.iter().map(|g| g.name().ok()).collect(),
            channel_names: groups
                .iter()
                .map(|g| g.channels().iter().map(|c| c.name().ok()).collect())
                .collect(),
            time_groups,
        }
    }
}

#[derive(Debug)]
/// Reader for a whole MDF 4 file (memory-mapped, or parsed from bytes).
///
/// `Mdf` and [`MdfIndex`](crate::index::MdfIndex) share one reading API —
/// the cross-language contract documented in `docs/API.md`:
///
/// | | |
/// |---|---|
/// | open | [`from_file`](Self::from_file), [`from_bytes`](Self::from_bytes), [`from_url`](Self::from_url) |
/// | metadata | [`groups`](Self::groups), [`group`](Self::group), [`channel`](Self::channel), [`channel_names`](Self::channel_names), [`start_time_ns`](Self::start_time_ns) |
/// | data | [`read`](Self::read) → [`Signal`], [`read_many`](Self::read_many) → `Vec<Signal>`, [`values`](Self::values) → `Vec<f64>` |
/// | inspection | [`time_range`](Self::time_range), [`conversion`](Self::conversion), [`file_layout`](Self::file_layout) |
///
/// Everything is addressed by **name**. Only metadata is parsed up front;
/// samples are decoded when `read` / `values` is called.
///
/// ```no_run
/// use mf4_rs::{Mdf, ReadOptions};
/// # fn main() -> Result<(), mf4_rs::MdfError> {
/// let mdf = Mdf::from_file("recording.mf4")?;
/// for group in mdf.groups()? {
///     println!("{:?}: {} records, {:?}", group.name, group.record_count, group.channel_names);
/// }
/// let speed = mdf.read("Speed", None)?;                 // Signal: values + timestamps
/// let rpm = mdf.values("RPM", &ReadOptions::new().group("Engine").time(Some(1.0), Some(2.0)))?;
/// # Ok(()) }
/// ```
///
/// Rust additionally exposes the borrowed block wrappers behind the contract
/// via [`channel_groups`](Self::channel_groups) ([`ChannelGroup`] /
/// [`Channel`]) for code that needs raw block access.
pub struct Mdf {
    raw: MdfFile,
    /// Group / channel names, filled on the first lookup.
    lookup: OnceLock<Lookup>,
}

impl Mdf {
    /// Open an MDF 4 file from disk (memory-mapped).
    ///
    /// Not available on `wasm32-unknown-unknown`; use [`from_bytes`](Self::from_bytes) instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(path: &str) -> Result<Self, MdfError> {
        let raw = MdfFile::parse_from_file(path)?;
        Ok(Mdf { raw, lookup: OnceLock::new() })
    }

    /// Parse an MDF 4 file from an owned byte buffer.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, MdfError> {
        let raw = MdfFile::parse_from_bytes(data)?;
        Ok(Mdf { raw, lookup: OnceLock::new() })
    }

    /// Download an MDF 4 file in full over HTTP(S) and parse it.
    ///
    /// To read a remote file *without* downloading it, use
    /// [`MdfIndex::from_url`](crate::index::MdfIndex::from_url).
    #[cfg(feature = "http")]
    pub fn from_url(url: &str) -> Result<Self, MdfError> {
        let mut http = crate::index::HttpRangeReader::new(url)?;
        Self::from_bytes(http.fetch_all()?)
    }

    // ----- metadata -------------------------------------------------------

    /// Metadata of every channel group, in file order.
    pub fn groups(&self) -> Result<Vec<GroupInfo>, MdfError> {
        self.channel_groups().iter().map(ChannelGroup::info).collect()
    }

    /// Metadata of the first channel group named `name`, or `None`.
    ///
    /// In a column-oriented (MDF 4.2) file one source group is stored as one
    /// group per signal plus a shared time group; this returns them merged
    /// back into the source group (time channel first), so the answer matches
    /// the source file's — also when the time group was shared with another
    /// source group and carries that group's name.
    pub fn group(&self, name: &str) -> Result<Option<GroupInfo>, MdfError> {
        let groups = self.channel_groups();
        let names = self.group_names(&groups)?;
        let Some(first) = names.iter().position(|n| *n == Some(name)) else {
            return Ok(None);
        };
        let channel_counts: Vec<usize> = groups.iter().map(|g| g.raw_channel_group().raw_channels.len()).collect();
        let members = logical_group_members(&names, &self.lookup().time_groups, &channel_counts, first);
        let infos = members.iter().map(|&m| groups[m].info()).collect::<Result<Vec<_>, _>>()?;
        Ok(Some(merge_group_infos(groups[first].info()?, infos)))
    }

    /// Metadata of the first channel named `name`, or `None`. Pass `group` to
    /// look only in the group(s) with that name.
    pub fn channel(&self, name: &str, group: Option<&str>) -> Result<Option<ChannelInfo>, MdfError> {
        match self.find(name, group)? {
            Some((g, ci)) => Ok(Some(g.channels()[ci].info()?)),
            None => Ok(None),
        }
    }

    /// Names of every named channel across all groups (duplicates kept).
    pub fn channel_names(&self) -> Result<Vec<String>, MdfError> {
        let groups = self.channel_groups();
        let mut names = Vec::new();
        for (g, channels) in self.lookup().channel_names.iter().enumerate() {
            for (c, name) in channels.iter().enumerate() {
                match name {
                    Some(name) => names.extend(name.clone()),
                    None => names.extend(groups[g].channels()[c].name()?),
                }
            }
        }
        Ok(names)
    }

    /// Measurement start time in nanoseconds since the Unix epoch, or `None`
    /// if the file header does not record one.
    pub fn start_time_ns(&self) -> Option<u64> {
        let abs_time = self.raw.header.abs_time;
        if abs_time == 0 {
            None
        } else {
            Some(abs_time)
        }
    }

    // ----- data -----------------------------------------------------------

    /// Read a channel as a [`Signal`]: its values (conversions applied,
    /// `None` = invalid sample) paired with the group's time axis. For the
    /// master channel itself `timestamps` is empty. With
    /// [`ReadOptions::raw`] the values are the stored ones, unconverted
    /// (the time axis stays physical).
    ///
    /// Errors with [`MdfError::ChannelNotFound`] if no such channel exists.
    ///
    /// `options` is `None` for the whole channel (first match across all
    /// groups), or a [`ReadOptions`] with a group filter and/or a time or
    /// record window:
    ///
    /// ```no_run
    /// # use mf4_rs::{Mdf, ReadOptions};
    /// # fn main() -> Result<(), mf4_rs::MdfError> {
    /// # let mdf = Mdf::from_file("recording.mf4")?;
    /// let speed = mdf.read("Speed", None)?;
    /// let part = mdf.read("Speed", &ReadOptions::new().group("Engine").time(Some(1.0), Some(2.5)))?;
    /// # Ok(()) }
    /// ```
    pub fn read<'o>(&self, name: &str, options: impl Into<Option<&'o ReadOptions>>) -> Result<Signal, MdfError> {
        self.read_impl(name, options.into().unwrap_or(&ReadOptions::default()))
    }

    /// Read several channels at once, with the same `options` for all of
    /// them: one [`Signal`] per name, in the order given.
    ///
    /// Equivalent to calling [`read`](Self::read) for each name, but a time
    /// axis shared by several of the channels is decoded once (in a
    /// column-oriented file every signal of a source group shares one).
    /// Errors with [`MdfError::ChannelNotFound`] — before reading anything —
    /// if any name is unknown.
    pub fn read_many<'o, S: AsRef<str>>(
        &self,
        names: &[S],
        options: impl Into<Option<&'o ReadOptions>>,
    ) -> Result<Vec<Signal>, MdfError> {
        let default = ReadOptions::default();
        let options = options.into().unwrap_or(&default);
        let window = options.window()?;
        let located = names
            .iter()
            .map(|name| self.locate(name.as_ref(), options.group_name()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut axes = AxisCache::new();
        located
            .iter()
            .map(|(g, ci)| self.read_located(g, *ci, options.raw, window, &mut axes))
            .collect()
    }

    fn read_impl(&self, name: &str, options: &ReadOptions) -> Result<Signal, MdfError> {
        let window = options.window()?;
        let (g, ci) = self.locate(name, options.group_name())?;
        self.read_located(&g, ci, options.raw, window, &mut AxisCache::new())
    }

    /// Read channel `ci` of `g`: only the records the window selects are
    /// decoded (a time window is searched on the master, not decoded in
    /// full). `axes` caches decoded time axes, so `read_many` decodes each
    /// once.
    fn read_located(
        &self,
        g: &ChannelGroup<'_>,
        ci: usize,
        raw: bool,
        window: Window,
        axes: &mut AxisCache,
    ) -> Result<Signal, MdfError> {
        let channels = g.channels();
        let channel = &channels[ci];
        // First master wins (matches MdfIndex's master selection).
        let master = g.master_channel().filter(|m| !std::ptr::eq(m.block(), channel.block()));
        let prepared = channel.prepare(raw)?;
        let master_prepared = master.as_ref().map(|m| m.prepare(false)).transpose()?;
        let name = channel.name()?.unwrap_or_default();
        let (start, count) =
            Self::record_window(channel, &name, &prepared, master_prepared.as_ref(), raw, window)?;
        let values = prepared.decoded(start, start + count)?;
        let timestamps = match (&master, &master_prepared) {
            (Some(m), Some(mp)) if mp.record_count() > 0 => {
                // A whole read pairs the values with the whole time axis.
                let (from, to) = match window {
                    Window::All => (0, mp.record_count()),
                    _ => (start, start + count),
                };
                let key = (m.block() as *const _ as usize, from, to);
                match axes.get(&key) {
                    Some(axis) => axis.clone(),
                    None => {
                        let axis = mp.values(from, to)?;
                        axes.insert(key, axis.clone());
                        axis
                    }
                }
            }
            _ => Vec::new(),
        };
        Ok(Signal { name, unit: channel.unit()?, timestamps, values })
    }

    /// Records `(start, count)` a window selects in `channel` (prepared as
    /// `prepared`, its group's master as `master`) — the same selection the
    /// index makes: a time window is located by searching the master's time
    /// axis (or the channel's own values when it is the master), probing
    /// single records instead of decoding the axis.
    fn record_window(
        channel: &Channel<'_>,
        name: &str,
        prepared: &PreparedChannel<'_>,
        master: Option<&PreparedChannel<'_>>,
        raw: bool,
        window: Window,
    ) -> Result<(u64, u64), MdfError> {
        let n = prepared.record_count();
        // A (malformed) file may give the master a different record count
        // than the channel; only the records both have can be selected.
        let master = master.filter(|m| m.record_count() > 0);
        let total = master.map_or(n, |m| n.min(m.record_count()));
        match window {
            Window::All => Ok((0, n)),
            Window::Records(start, count) => resolve_records(start, count, total),
            Window::Time(t0, t1) => match master {
                Some(m) => search_time_window(total, t0, t1, |i| m.value_at(i)),
                // The master's own values are its time axis — physical ones,
                // also for a raw read.
                None if channel.is_master() && raw => {
                    let physical = channel.prepare(false)?;
                    search_time_window(n, t0, t1, |i| physical.value_at(i))
                }
                None if channel.is_master() => search_time_window(n, t0, t1, |i| prepared.value_at(i)),
                None if n == 0 => Ok((0, 0)),
                None => Err(MdfError::InvalidArgument(format!(
                    "channel '{name}' has no master (time) channel; cannot select by time"
                ))),
            },
        }
    }

    /// Read a numeric channel as plain `f64` values (no timestamps).
    /// Conversions are applied unless `options.raw`; invalid and non-numeric
    /// samples are `NaN`. `options` as for [`read`](Self::read).
    pub fn values<'o>(&self, name: &str, options: impl Into<Option<&'o ReadOptions>>) -> Result<Vec<f64>, MdfError> {
        let default = ReadOptions::default();
        let options = options.into().unwrap_or(&default);
        let window = options.window()?;
        let (g, ci) = self.locate(name, options.group_name())?;
        let channels = g.channels();
        let channel = &channels[ci];
        let prepared = channel.prepare(options.raw)?;
        if window == Window::All {
            return prepared.values(0, prepared.record_count());
        }
        let master = g.master_channel().filter(|m| !std::ptr::eq(m.block(), channel.block()));
        let master_prepared = master.as_ref().map(|m| m.prepare(false)).transpose()?;
        let name = channel.name()?.unwrap_or_default();
        let (start, count) =
            Self::record_window(channel, &name, &prepared, master_prepared.as_ref(), options.raw, window)?;
        prepared.values(start, start + count)
    }

    // ----- inspection -----------------------------------------------------

    /// First and last master (time) value of the group holding `name`, or
    /// `None` for an empty group. Errors if the group has no master channel.
    pub fn time_range(&self, name: &str, group: Option<&str>) -> Result<Option<TimeRange>, MdfError> {
        let (g, ci) = self.locate(name, group)?;
        let channel = &g.channels()[ci];
        // Only the first and last record of the time axis are decoded.
        let axis = match g.master_channel() {
            Some(m) => m.prepare(false)?,
            None if channel.is_master() => channel.prepare(false)?,
            None if g.raw_channel_group().block.cycles_nr == 0 => return Ok(None),
            None => {
                return Err(MdfError::InvalidArgument(format!(
                    "channel '{name}' has no master (time) channel"
                )))
            }
        };
        let n = axis.record_count();
        if n == 0 {
            return Ok(None);
        }
        Ok(Some(TimeRange { start: axis.value_at(0)?, end: axis.value_at(n - 1)? }))
    }

    /// The conversion `read` / `values` apply to channel `name`, fully
    /// resolved, or `None` when the channel has none.
    pub fn conversion(&self, name: &str, group: Option<&str>) -> Result<Option<ConversionInfo>, MdfError> {
        let (g, ci) = self.locate(name, group)?;
        Ok(g.channels()[ci].conversion()?.as_ref().map(ConversionInfo::from))
    }

    /// Build a [`FileLayout`] describing every block in this file.
    ///
    /// Useful for debugging or analysing on-disk structure.
    pub fn file_layout(&self) -> Result<FileLayout, MdfError> {
        FileLayout::from_bytes(&self.raw.mmap)
    }

    // ----- Rust-only block access ----------------------------------------

    /// The file's channel groups as borrowed block wrappers.
    ///
    /// This is the layer underneath the contract: each [`ChannelGroup`] /
    /// [`Channel`] references the raw blocks and decodes nothing until asked.
    pub fn channel_groups(&self) -> Vec<ChannelGroup<'_>> {
        let mut groups = Vec::new();

        for raw_data_group in &self.raw.data_groups {
            for raw_channel_group in &raw_data_group.channel_groups {
                groups.push(ChannelGroup::with_file_groups(
                    raw_data_group,
                    raw_channel_group,
                    &self.raw.mmap,
                    &self.raw.data_groups,
                ));
            }
        }

        groups
    }

    /// The first channel named `name` as a borrowed [`Channel`] wrapper. Pass
    /// `group` to look only in the group(s) with that name.
    pub fn find_channel(&self, name: &str, group: Option<&str>) -> Result<Option<Channel<'_>>, MdfError> {
        Ok(self.find(name, group)?.map(|(g, ci)| g.channels().swap_remove(ci)))
    }

    /// First `(group, channel position)` matching `name`. With `group`, every
    /// group carrying that name is searched (a column-oriented file has one
    /// channel group per signal, all sharing the source group's name), then
    /// the time groups they reference under another name — so the shared time
    /// channel is found in every source group it serves, as in the source file.
    fn find(&self, name: &str, group: Option<&str>) -> Result<Option<(ChannelGroup<'_>, usize)>, MdfError> {
        let mut groups = self.channel_groups();
        let Some(wanted) = group else {
            for i in 0..groups.len() {
                if let Some(ci) = self.position(&groups, i, name)? {
                    return Ok(Some((groups.swap_remove(i), ci)));
                }
            }
            return Ok(None);
        };
        let names = self.group_names(&groups)?;
        let named: Vec<usize> = (0..groups.len()).filter(|&i| names[i] == Some(wanted)).collect();
        let time_groups = &self.lookup().time_groups;
        let remote = named.iter().filter_map(|&i| time_groups[i]).filter(|&t| names[t] != Some(wanted));
        let candidates: Vec<usize> = named.iter().copied().chain(remote).collect();
        for i in candidates {
            if let Some(ci) = self.position(&groups, i, name)? {
                return Ok(Some((groups.swap_remove(i), ci)));
            }
        }
        Ok(None)
    }

    fn lookup(&self) -> &Lookup {
        self.lookup.get_or_init(|| Lookup::new(&self.channel_groups()))
    }

    /// Position of the first channel named `name` in group `g` (of
    /// `groups`, this file's [`channel_groups`](Self::channel_groups)). An
    /// unreadable name it passes is re-read to report its error.
    fn position(&self, groups: &[ChannelGroup<'_>], g: usize, name: &str) -> Result<Option<usize>, MdfError> {
        for (c, channel_name) in self.lookup().channel_names[g].iter().enumerate() {
            match channel_name {
                Some(n) if n.as_deref() == Some(name) => return Ok(Some(c)),
                Some(_) => {}
                None => {
                    if groups[g].channels()[c].name()?.as_deref() == Some(name) {
                        return Ok(Some(c));
                    }
                }
            }
        }
        Ok(None)
    }

    /// Every group's name (an unreadable one is re-read to report its error).
    fn group_names<'s>(&'s self, groups: &[ChannelGroup<'_>]) -> Result<Vec<Option<&'s str>>, MdfError> {
        let mut names = Vec::with_capacity(groups.len());
        for (g, name) in self.lookup().group_names.iter().enumerate() {
            match name {
                Some(name) => names.push(name.as_deref()),
                None => {
                    groups[g].name()?;
                    return Err(MdfError::BlockSerializationError(format!(
                        "the name of channel group {g} cannot be read"
                    )));
                }
            }
        }
        Ok(names)
    }

    fn locate(&self, name: &str, group: Option<&str>) -> Result<(ChannelGroup<'_>, usize), MdfError> {
        self.find(name, group)?
            .ok_or_else(|| MdfError::channel_not_found(name, group))
    }
}
