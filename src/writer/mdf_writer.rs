//! Implementation of the MdfWriter struct for MDF 4.1-compliant file writing

use std::fs::File;
use std::io::{Write, Seek, SeekFrom};
use std::collections::HashMap;
use byteorder::{LittleEndian, WriteBytesExt};

use crate::error::MdfError;
use crate::blocks::identification_block::IdentificationBlock;
use crate::blocks::header_block::HeaderBlock;
use crate::blocks::data_group_block::DataGroupBlock;
use crate::blocks::channel_group_block::ChannelGroupBlock;
use crate::blocks::channel_block::ChannelBlock;
use crate::blocks::text_block::TextBlock;

/// Writer for MDF blocks, ensuring 8-byte alignment and zero padding.
/// Tracks block positions and supports updating links at a later stage.
pub struct MdfWriter {
    /// The file being written to
    file: File,
    /// Current write offset in the file
    offset: u64,
    /// Maps block IDs (user-provided keys) to their file offsets
    /// This allows retrieving positions later for link updates
    block_positions: HashMap<String, u64>,
}

impl MdfWriter {
    /// Creates a new MdfWriter for the given file path (overwrites existing).
    /// Initializes with an empty block position tracker.
    pub fn new(path: &str) -> Result<Self, MdfError> {
        let file = File::create(path)?;
        Ok(MdfWriter { 
            file, 
            offset: 0,
            block_positions: HashMap::new(),
        })
    }

    /// Writes a block to the file, aligning to 8 bytes and zero-padding as needed.
    /// Returns the starting offset of the block in the file.
    pub fn write_block(&mut self, block_bytes: &[u8]) -> Result<u64, MdfError> {
        // Align the current offset to 8 bytes before writing
        let align = (8 - (self.offset % 8)) % 8;
        if align != 0 {
            let padding = vec![0u8; align as usize];
            self.file.write_all(&padding)?;
            self.offset += align;
        }

        // Write the block
        self.file.write_all(block_bytes)?;
        let block_start = self.offset;
        self.offset += block_bytes.len() as u64;

        Ok(block_start)
    }

    /// Writes a block to the file and tracks its position with the given ID.
    /// This allows retrieving the position later with get_block_position().
    /// 
    /// # Arguments
    /// * `block_bytes` - The block data to write
    /// * `block_id` - A unique identifier for this block (e.g., "header", "dg1", etc.)
    ///
    /// # Returns
    /// The starting offset of the block in the file
    pub fn write_block_with_id(&mut self, block_bytes: &[u8], block_id: &str) -> Result<u64, MdfError> {
        // Write the block and get its position
        let block_start = self.write_block(block_bytes)?;
        
        // Store the position with the given ID
        self.block_positions.insert(block_id.to_string(), block_start);
        
        Ok(block_start)
    }

    /// Retrieves the file position of a previously written block.
    /// 
    /// # Arguments
    /// * `block_id` - The ID that was used with write_block_with_id()
    ///
    /// # Returns
    /// Some(position) if the block was found, None otherwise
    pub fn get_block_position(&self, block_id: &str) -> Option<u64> {
        self.block_positions.get(block_id).copied()
    }

    /// Updates a link (u64 address) at a specific offset in the file.
    /// This is useful for updating block links after all blocks have been written.
    /// 
    /// # Arguments
    /// * `offset` - The file offset where the link should be written
    /// * `address` - The address value to write (usually another block's position)
    ///
    /// # Returns
    /// Result with () on success or MdfError on failure
    pub fn update_link(&mut self, offset: u64, address: u64) -> Result<(), MdfError> {
        // Save current position to restore it later
        let current_pos = self.offset;
        
        // Seek to the offset where we want to write the link
        self.file.seek(SeekFrom::Start(offset))?;
        
        // Write the 8-byte address in little-endian format
        self.file.write_u64::<LittleEndian>(address)?;
        
        // Restore the original position
        self.file.seek(SeekFrom::Start(current_pos))?;
        
        Ok(())
    }

    /// Updates a link using block IDs instead of raw offsets.
    /// Links the 'source' block to the 'target' block by writing the target's
    /// address at the specified link_offset relative to the source block.
    ///
    /// # Arguments
    /// * `source_id` - ID of the source block
    /// * `link_offset` - Offset within the source block where the link should be written
    /// * `target_id` - ID of the target block (whose address will be written)
    ///
    /// # Returns
    /// Result with () on success or MdfError on failure
    pub fn update_block_link(&mut self, source_id: &str, link_offset: u64, target_id: &str) -> Result<(), MdfError> {
        // Get source and target positions
        let source_pos = self.get_block_position(source_id)
            .ok_or_else(|| MdfError::BlockLinkError(format!("Source block '{}' not found", source_id)))?;
        let target_pos = self.get_block_position(target_id)
            .ok_or_else(|| MdfError::BlockLinkError(format!("Target block '{}' not found", target_id)))?;
        
        // Calculate absolute file offset for the link
        let link_pos = source_pos + link_offset;
        
        // Update the link
        self.update_link(link_pos, target_pos)
    }

    /// Returns the current file offset (for block address calculation).
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Finalizes the file (flushes all data to disk).
    pub fn finalize(mut self) -> Result<(), MdfError> {
        self.file.flush()?;
        Ok(())
    }
    
    /// Initializes a new MDF 4.1 file with identification and header blocks.
    /// 
    /// This method writes the ID and HD blocks to the file and returns their positions.
    /// It also tracks them with the IDs "id_block" and "hd_block" for future reference.
    /// 
    /// # Returns
    /// A tuple with the positions of (id_block, hd_block)
    pub fn init_mdf_file(&mut self) -> Result<(u64, u64), MdfError> {
        // Create and write identification block
        let id_block = IdentificationBlock::default();
        let id_bytes = id_block.to_bytes()?;
        let id_pos = self.write_block_with_id(&id_bytes, "id_block")?;
        
        // Create and write header block
        let hd_block = HeaderBlock::default();
        let hd_bytes = hd_block.to_bytes()?;
        let hd_pos = self.write_block_with_id(&hd_bytes, "hd_block")?;
        
        Ok((id_pos, hd_pos))
    }
    
    /// Adds a data group block to the file and links it from the header block.
    /// If a previous data group exists, the new one is linked to the chain.
    /// 
    /// # Arguments
    /// * `prev_dg_id` - ID of the previous data group (if any, None for the first DG)
    /// 
    /// # Returns
    /// The ID assigned to the new data group block (for future reference)
    pub fn add_data_group(&mut self, prev_dg_id: Option<&str>) -> Result<String, MdfError> {
        // Generate a unique ID for this data group
        let dg_count = self.block_positions.keys()
            .filter(|k| k.starts_with("dg_"))
            .count();
        let dg_id = format!("dg_{}", dg_count);
        
        // Create a new data group block
        let dg_block = DataGroupBlock::default();
        let dg_bytes = dg_block.to_bytes()?;
        let _dg_pos = self.write_block_with_id(&dg_bytes, &dg_id)?;
        
        // Link from header block to the first data group
        if prev_dg_id.is_none() {
            // This is the first DG, link from header block
            let hd_dg_link_offset = 24; // Offset of first_dg_addr within header block
            self.update_block_link("hd_block", hd_dg_link_offset, &dg_id)?;
        } else {
            // Link from previous data group
            let prev_dg_id = prev_dg_id.unwrap();
            let prev_dg_next_link_offset = 24; // Offset of next_dg_addr within DG block
            self.update_block_link(prev_dg_id, prev_dg_next_link_offset, &dg_id)?;
        }
        
        Ok(dg_id)
    }
    
    /// Adds a channel group block to the specified data group and links it properly.
    /// If a previous channel group exists in this data group, the new one is linked to the chain.
    /// 
    /// # Arguments
    /// * `dg_id` - ID of the parent data group
    /// * `prev_cg_id` - ID of the previous channel group in this DG (if any, None for the first CG)
    /// 
    /// # Returns
    /// The ID assigned to the new channel group block (for future reference)
    pub fn add_channel_group(
        &mut self, 
        dg_id: &str, 
        prev_cg_id: Option<&str>
    ) -> Result<String, MdfError> {
        // Generate a unique ID for this channel group
        let cg_count = self.block_positions.keys()
            .filter(|k| k.starts_with("cg_"))
            .count();
        let cg_id = format!("cg_{}", cg_count);
        
        // Create a new channel group block
        let cg_block = ChannelGroupBlock::default();
        let cg_bytes = cg_block.to_bytes()?;
        let _cg_pos = self.write_block_with_id(&cg_bytes, &cg_id)?;
        
        // Link from data group to the first channel group
        if prev_cg_id.is_none() {
            // This is the first CG in this DG, link from data group block
            let dg_cg_link_offset = 32; // Offset of first_cg_addr within DG block
            self.update_block_link(dg_id, dg_cg_link_offset, &cg_id)?;
        } else {
            // Link from previous channel group
            let prev_cg_id = prev_cg_id.unwrap();
            let prev_cg_next_link_offset = 24; // Offset of next_cg_addr within CG block
            self.update_block_link(prev_cg_id, prev_cg_next_link_offset, &cg_id)?;
        }
        
        Ok(cg_id)
    }
    
    /// Adds a channel block to the specified channel group and links it properly.
    /// If previous channels exist in this channel group, the new one is linked to the chain.
    /// 
    /// # Arguments
    /// * `cg_id` - ID of the parent channel group
    /// * `prev_cn_id` - ID of the previous channel in this CG (if any, None for the first CN)
    /// * `name` - Optional name for this channel
    /// * `byte_offset` - Byte offset within the record for this channel's data
    /// * `bit_count` - Number of bits used by this channel's data
    /// 
    /// # Returns
    /// The ID assigned to the new channel block (for future reference)
    pub fn add_channel(
        &mut self, 
        cg_id: &str, 
        prev_cn_id: Option<&str>,
        name: Option<&str>,
        byte_offset: u32,
        bit_count: u32
    ) -> Result<String, MdfError> {
        // Generate a unique ID for this channel
        let cn_count = self.block_positions.keys()
            .filter(|k| k.starts_with("cn_"))
            .count();
        let cn_id = format!("cn_{}", cn_count);
        
        // Create a new channel block with custom settings
        let mut cn_block = ChannelBlock::default();
        
        // Set the provided parameters
        cn_block.byte_offset = byte_offset;
        cn_block.bit_count = bit_count;
        
        // Write the channel block first to get its position
        let cn_bytes = cn_block.to_bytes()?;
        let cn_pos = self.write_block_with_id(&cn_bytes, &cn_id)?;
        
        // If a name is provided, create a TextBlock for it
        if let Some(channel_name) = name {
            // Generate ID for the text block
            let tx_id = format!("tx_name_{}", cn_id);
            
            // Create and write the text block
            let tx_block = TextBlock::new(channel_name);
            let tx_bytes = tx_block.to_bytes()?;
            let tx_pos = self.write_block_with_id(&tx_bytes, &tx_id)?;
            
            // Update the name_addr link in the channel block
            // The name_addr field is at offset 40 within the channel block
            let name_link_offset = 40; 
            self.update_link(cn_pos + name_link_offset, tx_pos)?;
        }
        
        // Link from channel group to the first channel
        if prev_cn_id.is_none() {
            // This is the first CN in this CG, link from channel group block
            let cg_cn_link_offset = 32; // Offset of first_ch_addr within CG block
            self.update_block_link(cg_id, cg_cn_link_offset, &cn_id)?;
        } else {
            // Link from previous channel
            let prev_cn_id = prev_cn_id.unwrap();
            let prev_cn_next_link_offset = 24; // Offset of next_ch_addr within CN block
            self.update_block_link(prev_cn_id, prev_cn_next_link_offset, &cn_id)?;
        }
        
        Ok(cn_id)
    }
    
    /// Writes a complete simple MDF file with a single data group, channel group, and two channels.
    /// This is a convenient method for quickly creating a basic MDF file structure.
    /// 
    /// # Arguments
    /// * `file_path` - Path where the MDF file should be created
    /// 
    /// # Returns
    /// Result with () on success or MdfError on failure
    pub fn write_simple_mdf_file(file_path: &str) -> Result<(), MdfError> {
        // Create a new MDF writer
        let mut writer = MdfWriter::new(file_path)?;
        
        // Initialize with ID and HD blocks
        let (_id_pos, _hd_pos) = writer.init_mdf_file()?;
        
        // Add a data group
        let dg_id = writer.add_data_group(None)?;
        
        // Add a channel group to the data group
        let cg_id = writer.add_channel_group(&dg_id, None)?;
        
        // Add two channels to the channel group
        let cn1_id = writer.add_channel(&cg_id, None, Some("Channel 1"), 0, 32)?;
        let _cn2_id = writer.add_channel(&cg_id, Some(&cn1_id), Some("Channel 2"), 4, 32)?;
        
        // Finalize the file
        writer.finalize()
    }
}
