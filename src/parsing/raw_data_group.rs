use crate::error::MdfError;
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::blocks::{
    data_block::DataBlock,
    data_group_block::DataGroupBlock,
    data_list_block::DataListBlock,
    common::BlockHeader,
    common::BlockParse,
};

#[derive(Debug)]
pub struct RawDataGroup {
    pub block: DataGroupBlock,
    pub channel_groups: Vec<RawChannelGroup>,
}
impl RawDataGroup {

    /// Collect all data blocks referenced by this data group.
    ///
    /// The returned vector contains the `DT` or `DV` blocks in the order they
    /// appear on disk, transparently following any `DL` list chains.
    ///
    /// # Arguments
    /// * `mmap` - Memory mapped file containing the MDF data
    ///
    /// # Returns
    /// A vector of [`DataBlock`] objects or an [`MdfError`] if parsing fails.
    pub fn data_blocks<'a>(
        &self,
        mmap: &'a [u8],
    ) -> Result<Vec<DataBlock<'a>>, MdfError> {
        // Unsorted data groups interleave records of several channel groups
        // (each prefixed by a record id). Framing them as fixed-size records
        // of a single group silently mis-decodes the data, so refuse loudly.
        // Metadata access (names, channels) is unaffected — only record/data
        // access goes through here.
        if self.block.record_id_len > 0 && self.channel_groups.len() > 1 {
            return Err(MdfError::BlockSerializationError(
                "unsorted data groups (multiple channel groups per data group) are not supported"
                    .to_string(),
            ));
        }

        let mut collected_blocks = Vec::new();

        // Start at the group’s primary data pointer
        let mut current_block_address = self.block.data_block_addr;
        let mut visited = std::collections::HashSet::new();
        while current_block_address != 0 {
            if !visited.insert(current_block_address) {
                return Err(MdfError::BlockLinkError(format!(
                    "cycle detected in data block chain at address {:#x}",
                    current_block_address
                )));
            }
            let byte_offset = current_block_address as usize;

            // Read the block header (bounds-checked)
            let header_bytes = mmap
                .get(byte_offset..byte_offset.saturating_add(24))
                .ok_or(MdfError::TooShortBuffer {
                    actual:   mmap.len(),
                    expected: byte_offset.saturating_add(24),
                    file:     file!(),
                    line:     line!(),
                })?;
            let block_header = BlockHeader::from_bytes(header_bytes)?;

            match block_header.id.as_str() {
                "##DT" | "##DV" => {
                    // Single contiguous DataBlock
                    let data_block = DataBlock::from_bytes(&mmap[byte_offset..])?;
                    collected_blocks.push(data_block);
                    // No list to follow, we’re done
                    current_block_address = 0;
                }
                "##DL" => {
                    // Fragmented list of data blocks
                    let data_list_block = DataListBlock::from_bytes(&mmap[byte_offset..])?;

                    // Parse each fragment in this list
                    for &fragment_address in &data_list_block.data_links {
                        if fragment_address == 0 {
                            continue; // null link
                        }
                        let fragment_offset = fragment_address as usize;
                        let fragment_bytes =
                            mmap.get(fragment_offset..).ok_or(MdfError::TooShortBuffer {
                                actual:   mmap.len(),
                                expected: fragment_offset.saturating_add(24),
                                file:     file!(),
                                line:     line!(),
                            })?;
                        let fragment_block = DataBlock::from_bytes(fragment_bytes)?;

                        collected_blocks.push(fragment_block);
                    }

                    // Move to the next DLBLOCK in the chain (0 = end)
                    current_block_address = data_list_block.next;
                }

                unexpected_id => {
                    return Err(MdfError::BlockIDError {
                        actual: unexpected_id.to_string(),
                        expected: "##DT / ##DV / ##DL".to_string(),
                    });
                }
            }
        }

        Ok(collected_blocks)
    }
}