use crate::error::MdfError;
use crate::blocks::channel_block::ChannelBlock;
use crate::parsing::decoder::{ DecodedValue, decode_channel_value };
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::parsing::raw_data_group::RawDataGroup;
use crate::parsing::raw_channel::RawChannel;
use crate::parsing::source_info::SourceInfo;
use crate::blocks::common::read_string_block;

/// High level handle for a single channel within a group.
///
/// It holds references to the raw blocks and allows convenient access to
/// metadata and decoded values.
pub struct Channel<'a> {
    block:          &'a ChannelBlock,
    raw_data_group:   &'a RawDataGroup,
    raw_channel_group:     &'a RawChannelGroup,
    raw_channel:    &'a RawChannel,
    mmap:           &'a [u8],
}

impl<'a> Channel<'a> {
    /// Construct a new [`Channel`] from raw block references.
    ///
    /// # Arguments
    /// * `block` - Channel block containing metadata
    /// * `raw_data_group` - Parent data group
    /// * `raw_channel_group` - Parent channel group
    /// * `raw_channel` - Raw channel helper used to iterate samples
    /// * `mmap` - Memory mapped file backing all data
    ///
    /// # Returns
    /// A [`Channel`] handle with no samples decoded yet.
    pub fn new(
        block: &'a ChannelBlock,
        raw_data_group: &'a RawDataGroup,
        raw_channel_group: &'a RawChannelGroup,
        raw_channel: &'a RawChannel,
        mmap: &'a [u8],
    ) -> Self {
        Channel { block, raw_data_group, raw_channel_group, raw_channel, mmap }
    }
    /// Retrieve the channel name if present.
    pub fn name(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.block.name_addr)
    }

    /// Retrieve the physical unit description.
    pub fn unit(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.block.unit_addr)
    }

    /// Retrieve the channel comment if present.
    pub fn comment(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.block.comment_addr)
    }

    /// Get the acquisition source for this channel if available.
    pub fn source(&self) -> Result<Option<SourceInfo>, MdfError> {
        let addr = self.block.source_addr;
        SourceInfo::from_mmap(self.mmap, addr)
    }

    /// Decode and convert all samples of this channel.
    ///
    /// # Returns
    /// A vector with one [`DecodedValue`] per record.
    pub fn values(&self) -> Result<Vec<DecodedValue>, MdfError> {
        let record_id_len = self.raw_data_group.block.record_id_len as usize;
        let mut out = Vec::new();
        
        let records_iter = self
            .raw_channel
            .records(self.raw_data_group, self.raw_channel_group, self.mmap)?;
        
        for rec_res in records_iter {
            let ref rec = rec_res?;
            let dv = decode_channel_value(rec, record_id_len, self.block)
                .unwrap_or(DecodedValue::Unknown);
            let phys = self.block.apply_conversion_value(dv, self.mmap)?;
            out.push(phys);
        }
        Ok(out)
    }
}
