use crate::api::channel::Channel;
use crate::api::channel_group::ChannelGroup;
use crate::block_layout::FileLayout;
use crate::error::MdfError;
use crate::parsing::mdf_file::MdfFile;
use crate::signal::Signal;
use crate::types::{
    logical_group_members, merge_group_infos, slice_signal, ChannelInfo, ConversionInfo, GroupInfo, ReadOptions,
    TimeRange, Window,
};
use std::collections::HashMap;

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
}

impl Mdf {
    /// Open an MDF 4 file from disk (memory-mapped).
    ///
    /// Not available on `wasm32-unknown-unknown`; use [`from_bytes`](Self::from_bytes) instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(path: &str) -> Result<Self, MdfError> {
        let raw = MdfFile::parse_from_file(path)?;
        Ok(Mdf { raw })
    }

    /// Parse an MDF 4 file from an owned byte buffer.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, MdfError> {
        let raw = MdfFile::parse_from_bytes(data)?;
        Ok(Mdf { raw })
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
        let names = Self::group_names(&groups)?;
        let Some(first) = names.iter().position(|n| n.as_deref() == Some(name)) else {
            return Ok(None);
        };
        let channel_counts: Vec<usize> = groups.iter().map(|g| g.raw_channel_group().raw_channels.len()).collect();
        let members = logical_group_members(&names, &Self::time_groups(&groups), &channel_counts, first);
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
        let mut names = Vec::new();
        for g in self.channel_groups() {
            for ch in g.channels() {
                names.extend(ch.name()?);
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
        let mut axes = HashMap::new();
        located
            .iter()
            .map(|(g, ci)| self.read_located(g, *ci, options.raw, window, &mut axes))
            .collect()
    }

    fn read_impl(&self, name: &str, options: &ReadOptions) -> Result<Signal, MdfError> {
        let window = options.window()?;
        let (g, ci) = self.locate(name, options.group_name())?;
        self.read_located(&g, ci, options.raw, window, &mut HashMap::new())
    }

    /// Read channel `ci` of `g`. `axes` caches decoded time axes by master
    /// block (keyed by its address), so `read_many` decodes each once.
    fn read_located(
        &self,
        g: &ChannelGroup<'_>,
        ci: usize,
        raw: bool,
        window: Window,
        axes: &mut HashMap<usize, Vec<f64>>,
    ) -> Result<Signal, MdfError> {
        let channels = g.channels();
        let channel = &channels[ci];
        let values = channel.decoded_values_as(raw)?;
        // First master wins (matches MdfIndex's master selection).
        let timestamps = match g.master_channel() {
            Some(m) if !std::ptr::eq(m.block(), channel.block()) => {
                let key = m.block() as *const _ as usize;
                match axes.get(&key) {
                    Some(axis) => axis.clone(),
                    None => {
                        let axis = m.values()?;
                        axes.insert(key, axis.clone());
                        axis
                    }
                }
            }
            _ => Vec::new(),
        };
        let mut signal = Signal { name: channel.name()?.unwrap_or_default(), unit: channel.unit()?, timestamps, values };
        let is_master = channel.is_master();
        if raw && is_master && matches!(window, Window::Time(..)) {
            // The raw master's own values are not its time axis: search the
            // physical one (as `MdfIndex` does), carried as timestamps while
            // slicing and dropped afterwards.
            signal.timestamps = channel.values()?;
            let mut sliced = slice_signal(signal, window, false)?;
            sliced.timestamps.clear();
            return Ok(sliced);
        }
        slice_signal(signal, window, is_master)
    }

    /// Read a numeric channel as plain `f64` values (no timestamps).
    /// Conversions are applied unless `options.raw`; invalid and non-numeric
    /// samples are `NaN`. `options` as for [`read`](Self::read).
    pub fn values<'o>(&self, name: &str, options: impl Into<Option<&'o ReadOptions>>) -> Result<Vec<f64>, MdfError> {
        let default = ReadOptions::default();
        let options = options.into().unwrap_or(&default);
        if options.window()? == Window::All {
            let (g, ci) = self.locate(name, options.group_name())?;
            return g.channels()[ci].values_as(options.raw);
        }
        Ok(self.read_impl(name, options)?.values_f64())
    }

    // ----- inspection -----------------------------------------------------

    /// First and last master (time) value of the group holding `name`, or
    /// `None` for an empty group. Errors if the group has no master channel.
    pub fn time_range(&self, name: &str, group: Option<&str>) -> Result<Option<TimeRange>, MdfError> {
        let (g, ci) = self.locate(name, group)?;
        let channel = &g.channels()[ci];
        let axis = match g.master_channel() {
            Some(m) => m.values()?,
            None if channel.is_master() => channel.values()?,
            None if g.raw_channel_group().block.cycles_nr == 0 => return Ok(None),
            None => {
                return Err(MdfError::InvalidArgument(format!(
                    "channel '{name}' has no master (time) channel"
                )))
            }
        };
        Ok(match (axis.first(), axis.last()) {
            (Some(&start), Some(&end)) => Some(TimeRange { start, end }),
            _ => None,
        })
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
            for (i, g) in groups.iter().enumerate() {
                if let Some(ci) = g.position(name)? {
                    return Ok(Some((groups.swap_remove(i), ci)));
                }
            }
            return Ok(None);
        };
        let names = Self::group_names(&groups)?;
        let named: Vec<usize> = (0..groups.len()).filter(|&i| names[i].as_deref() == Some(wanted)).collect();
        let time_groups = Self::time_groups(&groups);
        let remote = named.iter().filter_map(|&i| time_groups[i]).filter(|&t| names[t].as_deref() != Some(wanted));
        let candidates: Vec<usize> = named.iter().copied().chain(remote).collect();
        for i in candidates {
            if let Some(ci) = groups[i].position(name)? {
                return Ok(Some((groups.swap_remove(i), ci)));
            }
        }
        Ok(None)
    }

    fn group_names(groups: &[ChannelGroup<'_>]) -> Result<Vec<Option<String>>, MdfError> {
        groups.iter().map(ChannelGroup::name).collect()
    }

    /// For each group, the group holding its time axis: itself for a local
    /// master, the referenced group for an MDF 4.2 remote master, `None` when
    /// untimed.
    fn time_groups(groups: &[ChannelGroup<'_>]) -> Vec<Option<usize>> {
        let has_master = |g: &ChannelGroup<'_>| {
            g.raw_channel_group().raw_channels.iter().any(|c| matches!(c.block.channel_type, 2 | 3))
        };
        (0..groups.len())
            .map(|i| {
                if has_master(&groups[i]) {
                    return Some(i);
                }
                let target = groups[i].raw_channel_group().block.master_cg_addr;
                if target == 0 {
                    return None;
                }
                groups
                    .iter()
                    .position(|g| g.raw_channel_group().addr == target)
                    .filter(|&m| m != i && has_master(&groups[m]))
            })
            .collect()
    }

    fn locate(&self, name: &str, group: Option<&str>) -> Result<(ChannelGroup<'_>, usize), MdfError> {
        self.find(name, group)?
            .ok_or_else(|| MdfError::channel_not_found(name, group))
    }
}
