use crate::blocks::common::read_string_block;
use crate::parsing::raw_data_group::RawDataGroup;
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::parsing::source_info::SourceInfo;
use crate::api::channel::Channel;
use crate::error::MdfError;
use crate::signal::Signal;

/// High level wrapper for a channel group.
///
/// The struct references raw channel group data and provides ergonomic access
/// to its metadata and channels without decoding any actual samples.
pub struct ChannelGroup<'a> {
    raw_data_group:    &'a RawDataGroup,
    raw_channel_group: &'a RawChannelGroup,
    mmap:              &'a [u8],
    /// Every data group of the file, used to resolve an MDF 4.2 remote master
    /// (a master channel stored in another channel group). Empty when the
    /// group was built standalone via [`ChannelGroup::new`].
    file_groups:       &'a [RawDataGroup],
}

impl<'a> ChannelGroup<'a> {
    /// Create a new [`ChannelGroup`] referencing the underlying raw blocks.
    ///
    /// # Arguments
    /// * `raw_data_group` - Parent data group containing this channel group
    /// * `raw_channel_group` - The raw channel group block
    /// * `mmap` - Memory mapped file backing all data
    ///
    /// # Returns
    /// A [`ChannelGroup`] handle with no decoded data.
    pub fn new(
        raw_data_group: &'a RawDataGroup,
        raw_channel_group: &'a RawChannelGroup,
        mmap: &'a [u8],
    ) -> Self {
        ChannelGroup { raw_data_group, raw_channel_group, mmap, file_groups: &[] }
    }

    /// Like [`ChannelGroup::new`], but aware of the file's other data groups so
    /// MDF 4.2 remote masters can be resolved.
    pub(crate) fn with_file_groups(
        raw_data_group: &'a RawDataGroup,
        raw_channel_group: &'a RawChannelGroup,
        mmap: &'a [u8],
        file_groups: &'a [RawDataGroup],
    ) -> Self {
        ChannelGroup { raw_data_group, raw_channel_group, mmap, file_groups }
    }

    /// The channel holding this group's time axis: the group's own master
    /// channel (channel type 2), or — for an MDF 4.2 column-oriented file —
    /// the master of the channel group referenced by `cg_cg_master`.
    ///
    /// `None` when the group has no master.
    pub fn master_channel(&self) -> Option<Channel<'a>> {
        if let Some(raw) = self
            .raw_channel_group
            .raw_channels
            .iter()
            .find(|c| c.block.channel_type == 2)
        {
            return Some(Channel::new(&raw.block, self.raw_data_group, self.raw_channel_group, raw, self.mmap));
        }
        let target = self.raw_channel_group.block.master_cg_addr;
        if target == 0 {
            return None;
        }
        for dg in self.file_groups {
            for cg in &dg.channel_groups {
                if cg.addr == target {
                    let raw = cg.raw_channels.iter().find(|c| c.block.channel_type == 2)?;
                    return Some(Channel::new(&raw.block, dg, cg, raw, self.mmap));
                }
            }
        }
        None
    }

    /// Retrieve the human readable group name.
    pub fn name(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.raw_channel_group.block.acq_name_addr)
    }

    /// Retrieve the group comment if present.
    pub fn comment(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.raw_channel_group.block.comment_addr)
    }

    /// Get the acquisition source information if available.
    pub fn source(&self) -> Result<Option<SourceInfo>, MdfError> {
        let addr = self.raw_channel_group.block.acq_source_addr;
        SourceInfo::from_mmap(self.mmap, addr)
    }

    /// Build all [`Channel`] objects for this group.
    ///
    /// No channel data is decoded; the returned channels simply reference the
    /// raw blocks.
    pub fn channels(&self) -> Vec<Channel<'a>> {

        let mut channels = Vec::new();
        for raw_channel in &self.raw_channel_group.raw_channels {
            let channel = Channel::new(
                &raw_channel.block,
                self.raw_data_group,
                self.raw_channel_group,
                raw_channel,
                self.mmap,
            );
            channels.push(channel);
        }

        channels
    }

    /// Find a channel in this group by name (first match).
    pub fn channel(&self, name: &str) -> Option<Channel<'a>> {
        self.channels()
            .into_iter()
            .find(|c| c.name().ok().flatten().as_deref() == Some(name))
    }

    /// Read a channel by name as a [`Signal`] (values paired with the group's
    /// master/time axis).
    ///
    /// Returns `Ok(None)` if no channel with that name exists in this group.
    /// `timestamps` is empty when the group has no master channel or when the
    /// requested channel *is* the master. A remote master (MDF 4.2
    /// column-oriented files) is followed transparently.
    pub fn signal(&self, name: &str) -> Result<Option<Signal>, MdfError> {
        let channels = self.channels();
        let mut target: Option<usize> = None;
        for (i, ch) in channels.iter().enumerate() {
            if ch.name()?.as_deref() == Some(name) {
                target = Some(i);
                break;
            }
        }
        let Some(ci) = target else { return Ok(None) };

        let values = channels[ci].values()?;
        // First master wins (matches MdfIndex's master selection)
        let timestamps = match self.master_channel() {
            Some(m) if !std::ptr::eq(m.block(), channels[ci].block()) => m.values_as_f64()?,
            _ => Vec::new(),
        };
        Ok(Some(Signal {
            name: name.to_string(),
            unit: channels[ci].unit()?,
            timestamps,
            values,
        }))
    }

    /// Get the raw data group (for internal use)
    pub fn raw_data_group(&self) -> &RawDataGroup {
        self.raw_data_group
    }

    /// Get the raw channel group (for internal use) 
    pub fn raw_channel_group(&self) -> &RawChannelGroup {
        self.raw_channel_group
    }

    /// Get the memory mapped data (for internal use)
    pub fn mmap(&self) -> &[u8] {
        self.mmap
    }
}
