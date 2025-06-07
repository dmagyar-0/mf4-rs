use crate::error::MdfError;
use crate::blocks::channel_block::ChannelBlock;
use crate::parsing::decoder::{ DecodedValue, decode_channel_value };
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::parsing::raw_data_group::RawDataGroup;
use crate::parsing::raw_channel::RawChannel;
use crate::parsing::source_info::SourceInfo;
use crate::blocks::common::read_string_block;

pub struct Channel<'a> {
    block:          &'a ChannelBlock,
    raw_data_group:   &'a RawDataGroup,
    raw_channel_group:     &'a RawChannelGroup,
    raw_channel:    &'a RawChannel,
    mmap:           &'a [u8],
}

impl<'a> Channel<'a> {
    /// Build the bare minimum: pointers to raw blocks + sizes.
    pub fn new(
        block: &'a ChannelBlock,
        raw_data_group: &'a RawDataGroup,
        raw_channel_group: &'a RawChannelGroup,
        raw_channel: &'a RawChannel,
        mmap: &'a [u8],
    ) -> Self {
        Channel { block, raw_data_group, raw_channel_group, raw_channel, mmap }
    }
    /// Human‐readable name
    pub fn name(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.block.name_addr)
    }

    /// Unit, if any
    pub fn unit(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.block.unit_addr)
    }

    /// Comment, if any
    pub fn comment(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.block.comment_addr)
    }

    /// The signal source for this channel, if present.
    pub fn source(&self) -> Result<Option<SourceInfo>, MdfError> {
        let addr = self.block.source_addr;
        SourceInfo::from_mmap(self.mmap, addr)
    }

    /// Decode *and* convert every sample now—returns one element per record.
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
