use memmap2::Mmap;
use std::fs::File;

use crate::error::MdfError;
use crate::parsing::raw_data_group::RawDataGroup;
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::parsing::raw_channel::RawChannel;
use crate::blocks::{
    common::BlockParse,
    channel_group_block::ChannelGroupBlock,
    data_group_block::DataGroupBlock,
    header_block::HeaderBlock,
    identification_block::IdentificationBlock,
};

#[derive(Debug)]
pub struct MdfFile {
    pub identification: IdentificationBlock,
    pub header: HeaderBlock,
    pub data_groups: Vec<RawDataGroup>,
    pub mmap: Mmap, // Keep the mmap in the MdfFile to guarantee lifetime for our slices.
}

impl MdfFile {
    /// Parse an MDF file from a given file path.
    ///
    /// # Arguments
    /// * `path` - Path to the `.mf4` file on disk.
    ///
    /// # Returns
    /// An [`MdfFile`] containing all parsed blocks or an [`MdfError`] if the
    /// file could not be read or decoded.
    pub fn parse_from_file(path: &str) -> Result<Self, MdfError> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };

        // Parse Identification block (first 64 bytes) and Header block (next 104 bytes)
        let identification = IdentificationBlock::from_bytes(&mmap[0..64])?;
        let header = HeaderBlock::from_bytes(&mmap[64..64 + 104])?;

        // Parse Data Groups, assume a linked list of data groups.
        let mut data_groups = Vec::new();
        let mut dg_addr = header.first_dg_addr;
        while dg_addr != 0 {
            let dg_offset = dg_addr as usize;
            let data_group_block = DataGroupBlock::from_bytes(&mmap[dg_offset..])?;
            // Save next dg address before moving data_group_block.
            let next_dg_addr = data_group_block.next_dg_addr;

            let mut next_cg_addr = data_group_block.first_cg_addr;
            let mut raw_channel_groups = Vec::new();
            while next_cg_addr != 0 {
                // Parse channel group
                let offset = next_cg_addr as usize;
                let mut channel_group_block = ChannelGroupBlock::from_bytes(&mmap[offset..])?;
                next_cg_addr = channel_group_block.next_cg_addr;
                let channels = channel_group_block.read_channels(&mmap)?;

                let raw_channels: Vec<RawChannel> = channels
                    .into_iter()
                    .map(|channel_block| {
                        RawChannel {
                            block: channel_block
                        }
                    })
                    .collect();

                let channel_group = RawChannelGroup {
                    block: channel_group_block,
                    raw_channels,
                };
                raw_channel_groups.push(channel_group);
                
            }
            let dg = RawDataGroup {
                    block: data_group_block,
                    channel_groups: raw_channel_groups,
                };
                data_groups.push(dg);

            dg_addr = next_dg_addr;
        }

        Ok(Self {
            identification,
            header,
            data_groups,
            mmap,
        })
    }
}
