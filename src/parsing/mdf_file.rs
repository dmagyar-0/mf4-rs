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
    /// Backing byte store. On native targets this is a memory-mapped file;
    /// on wasm32 (and when using `parse_from_bytes`) it is an owned `Vec<u8>`.
    #[cfg(not(target_arch = "wasm32"))]
    pub mmap: memmap2::Mmap,
    #[cfg(target_arch = "wasm32")]
    pub mmap: Vec<u8>,
}

impl MdfFile {
    /// Parse an MDF file from a given file path.
    ///
    /// Not available on `wasm32-unknown-unknown`; use [`parse_from_bytes`] instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn parse_from_file(path: &str) -> Result<Self, MdfError> {
        use memmap2::Mmap;
        use std::fs::File;

        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        Self::parse_from_slice(&mmap[..]).map(|(identification, header, data_groups)| Self {
            identification,
            header,
            data_groups,
            mmap,
        })
    }

    /// Parse an MDF file from an owned byte buffer.
    ///
    /// On native targets the bytes are copied into an anonymous memory mapping
    /// so that the rest of the codebase can continue to use `Mmap` references.
    /// On `wasm32-unknown-unknown` the `Vec<u8>` is stored directly.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn parse_from_bytes(data: Vec<u8>) -> Result<Self, MdfError> {
        use memmap2::MmapMut;
        let mut mmap_mut = MmapMut::map_anon(data.len())?;
        mmap_mut.copy_from_slice(&data);
        let mmap = mmap_mut.make_read_only()?;
        Self::parse_from_slice(&mmap[..]).map(|(identification, header, data_groups)| Self {
            identification,
            header,
            data_groups,
            mmap,
        })
    }

    /// Parse an MDF file from an owned byte buffer (WASM entry point).
    #[cfg(target_arch = "wasm32")]
    pub fn parse_from_bytes(data: Vec<u8>) -> Result<Self, MdfError> {
        let (identification, header, data_groups) = Self::parse_from_slice(&data)?;
        Ok(Self {
            identification,
            header,
            data_groups,
            mmap: data,
        })
    }

    /// Core parsing logic that operates on a plain byte slice.
    fn parse_from_slice(
        data: &[u8],
    ) -> Result<(IdentificationBlock, HeaderBlock, Vec<RawDataGroup>), MdfError> {
        let identification = IdentificationBlock::from_bytes(&data[0..64])?;
        let header = HeaderBlock::from_bytes(&data[64..64 + 104])?;

        let mut data_groups = Vec::new();
        let mut dg_addr = header.first_dg_addr;
        while dg_addr != 0 {
            let dg_offset = dg_addr as usize;
            let data_group_block = DataGroupBlock::from_bytes(&data[dg_offset..])?;
            let next_dg_addr = data_group_block.next_dg_addr;

            let mut next_cg_addr = data_group_block.first_cg_addr;
            let mut raw_channel_groups = Vec::new();
            while next_cg_addr != 0 {
                let offset = next_cg_addr as usize;
                let mut channel_group_block = ChannelGroupBlock::from_bytes(&data[offset..])?;
                next_cg_addr = channel_group_block.next_cg_addr;
                let channels = channel_group_block.read_channels(data)?;

                let raw_channels: Vec<RawChannel> = channels
                    .into_iter()
                    .map(|channel_block| RawChannel { block: channel_block })
                    .collect();

                raw_channel_groups.push(RawChannelGroup {
                    block: channel_group_block,
                    raw_channels,
                });
            }
            data_groups.push(RawDataGroup {
                block: data_group_block,
                channel_groups: raw_channel_groups,
            });

            dg_addr = next_dg_addr;
        }

        Ok((identification, header, data_groups))
    }
}
