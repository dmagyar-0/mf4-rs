use crate::blocks::common::read_string_block;
use crate::parsing::raw_data_group::RawDataGroup;
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::parsing::source_info::SourceInfo;
use crate::api::channel::Channel;
use crate::error::MdfError;

/// A high‐level ChannelGroup that exposes metadata and lazily builds `Channel<'a>`s.
pub struct ChannelGroup<'a> {
    raw_data_group:    &'a RawDataGroup,
    raw_channel_group: &'a RawChannelGroup,
    mmap:              &'a [u8],
}

impl<'a> ChannelGroup<'a> {
    /// Create a new ChannelGroup, no decoding or slicing yet.
    pub fn new(
        raw_data_group: &'a RawDataGroup,
        raw_channel_group: &'a RawChannelGroup,
        mmap: &'a [u8],
    ) -> Self {
        ChannelGroup { raw_data_group, raw_channel_group, mmap }
    }

    /// Human‐readable name
    pub fn name(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.raw_channel_group.block.acq_name_addr)
    }

    /// Comment, if any
    pub fn comment(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.raw_channel_group.block.comment_addr)
    }

    /// The signal source for this channel, if present.
    pub fn source(&self) -> Result<Option<SourceInfo>, MdfError> {
        let addr = self.raw_channel_group.block.acq_source_addr;
        SourceInfo::from_mmap(self.mmap, addr)
    }

    /// Build all `Channel<'a>` for this group; none of them is decoded yet.
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
}
