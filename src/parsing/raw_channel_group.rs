use crate::blocks::channel_group_block::ChannelGroupBlock;
use crate::parsing::raw_channel::RawChannel;

#[derive(Debug)]
pub struct RawChannelGroup {
    pub block: ChannelGroupBlock,
    pub raw_channels: Vec<RawChannel>,
}
