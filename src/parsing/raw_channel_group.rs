use crate::blocks::channel_group_block::ChannelGroupBlock;
use crate::parsing::raw_channel::RawChannel;

#[derive(Debug)]
pub struct RawChannelGroup {
    /// File offset of this group's `##CG` block (used to resolve MDF 4.2
    /// remote-master links, which reference a channel group by address).
    pub addr: u64,
    pub block: ChannelGroupBlock,
    pub raw_channels: Vec<RawChannel>,
}
