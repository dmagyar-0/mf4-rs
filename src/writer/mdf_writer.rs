//! Implementation of the MdfWriter struct for MDF 4.1-compliant file writing

use std::fs::File;
use std::io::{Write, Seek, SeekFrom};
use std::collections::HashMap;
use byteorder::{LittleEndian, WriteBytesExt};

use crate::blocks::common::{BlockHeader, DataType};
use crate::parsing::decoder::DecodedValue;

use crate::error::MdfError;
use crate::blocks::identification_block::IdentificationBlock;
use crate::blocks::header_block::HeaderBlock;
use crate::blocks::data_group_block::DataGroupBlock;
use crate::blocks::channel_group_block::ChannelGroupBlock;
use crate::blocks::channel_block::ChannelBlock;
use crate::blocks::text_block::TextBlock;
use crate::blocks::data_list_block::DataListBlock;

/// Maximum size of a DTBLOCK including header (4 MiB)
const MAX_DT_BLOCK_SIZE: usize = 4 * 1024 * 1024;

/// Helper structure tracking an open DTBLOCK during writing
struct OpenDataBlock {
    dg_id: String,
    dt_id: String,
    start_pos: u64,
    record_size: usize,
    record_count: u64,
    record_id_len: usize,
    channels: Vec<ChannelBlock>,
    dt_ids: Vec<String>,
    dt_positions: Vec<u64>,
}

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
    /// Track open DT blocks per channel group
    open_dts: HashMap<String, OpenDataBlock>,
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
            open_dts: HashMap::new(),
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

    /// Updates a 32-bit value at the given file offset, restoring the current
    /// position afterwards.
    fn update_u32(&mut self, offset: u64, value: u32) -> Result<(), MdfError> {
        let current_pos = self.offset;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_u32::<LittleEndian>(value)?;
        self.file.seek(SeekFrom::Start(current_pos))?;
        Ok(())
    }

    /// Updates an 8-bit value at the given file offset, restoring the current
    /// position afterwards.
    fn update_u8(&mut self, offset: u64, value: u8) -> Result<(), MdfError> {
        let current_pos = self.offset;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_u8(value)?;
        self.file.seek(SeekFrom::Start(current_pos))?;
        Ok(())
    }

    /// Convenience wrapper around [`update_u32`] that updates a field within a
    /// previously written block identified by `block_id`.
    fn update_block_u32(&mut self, block_id: &str, field_offset: u64, value: u32) -> Result<(), MdfError> {
        let block_pos = self.get_block_position(block_id)
            .ok_or_else(|| MdfError::BlockLinkError(format!("Block '{}' not found", block_id)))?;
        self.update_u32(block_pos + field_offset, value)
    }

    /// Convenience wrapper around [`update_u8`] that updates a field within a
    /// previously written block identified by `block_id`.
    fn update_block_u8(&mut self, block_id: &str, field_offset: u64, value: u8) -> Result<(), MdfError> {
        let block_pos = self.get_block_position(block_id)
            .ok_or_else(|| MdfError::BlockLinkError(format!("Block '{}' not found", block_id)))?;
        self.update_u8(block_pos + field_offset, value)
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
    /// * `cg_block` - Fully configured ChannelGroupBlock to write
    /// 
    /// # Returns
    /// The ID assigned to the new channel group block (for future reference)
    pub fn add_channel_group(
        &mut self,
        dg_id: &str,
        prev_cg_id: Option<&str>,
        cg_block: &ChannelGroupBlock,
    ) -> Result<String, MdfError> {
        // Generate a unique ID for this channel group
        let cg_count = self.block_positions.keys()
            .filter(|k| k.starts_with("cg_"))
            .count();
        let cg_id = format!("cg_{}", cg_count);
        
        // Serialize the provided channel group block
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
    /// * `channel` - Fully configured ChannelBlock describing the new channel
    /// 
    /// # Returns
    /// The ID assigned to the new channel block (for future reference)
    pub fn add_channel(
        &mut self,
        cg_id: &str,
        prev_cn_id: Option<&str>,
        channel: &ChannelBlock,
    ) -> Result<String, MdfError> {
        // Generate a unique ID for this channel
        let cn_count = self.block_positions.keys()
            .filter(|k| k.starts_with("cn_"))
            .count();
        let cn_id = format!("cn_{}", cn_count);
        
        // Serialize the provided channel block
        let cn_bytes = channel.to_bytes()?;
        let cn_pos = self.write_block_with_id(&cn_bytes, &cn_id)?;
        // If a channel name is provided, create a TextBlock for it
        if let Some(channel_name) = &channel.name {
            let tx_id = format!("tx_name_{}", cn_id);
            let tx_block = TextBlock::new(channel_name);
            let tx_bytes = tx_block.to_bytes()?;
            let tx_pos = self.write_block_with_id(&tx_bytes, &tx_id)?;
            let name_link_offset = 40; // name_addr field offset
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

    /// Start writing a DTBLOCK for the given data group.
    /// `channels` describes the fixed layout of one record.
    pub fn start_data_block(
        &mut self,
        dg_id: &str,
        cg_id: &str,
        record_id_len: u8,
        channels: &[ChannelBlock],
    ) -> Result<(), MdfError> {
        if self.open_dts.contains_key(cg_id) {
            return Err(MdfError::BlockSerializationError("data block already open for this channel group".into()));
        }

        let mut record_bytes = 0usize;
        for ch in channels {
            let byte_end = ch.byte_offset as usize + ((ch.bit_offset as usize + ch.bit_count as usize + 7) / 8);
            record_bytes = record_bytes.max(byte_end);
        }
        let record_size = record_bytes + record_id_len as usize;

        // Write DT header with placeholder length
        let header = BlockHeader {
            id: "##DT".to_string(),
            reserved0: 0,
            block_len: 24,
            links_nr: 0,
        };
        let header_bytes = header.to_bytes()?;

        let dt_count = self.block_positions.keys().filter(|k| k.starts_with("dt_")).count();
        let dt_id = format!("dt_{}", dt_count);
        let dt_pos = self.write_block_with_id(&header_bytes, &dt_id)?;

        // Patch DG's data pointer to this DT block
        let dg_data_link_offset = 40; // data_block_addr field within DG
        self.update_block_link(dg_id, dg_data_link_offset, &dt_id)?;

        // Update metadata in DG and CG blocks
        // record_id_len field resides at offset 56 within the DG block
        self.update_block_u8(dg_id, 56, record_id_len)?;
        // samples_byte_nr field resides at offset 96 within the CG block
        self.update_block_u32(cg_id, 96, record_bytes as u32)?;

        self.open_dts.insert(
            cg_id.to_string(),
            OpenDataBlock {
                dg_id: dg_id.to_string(),
                dt_id: dt_id.clone(),
                start_pos: dt_pos,
                record_size,
                record_count: 0,
                record_id_len: record_id_len as usize,
                channels: channels.to_vec(),
                dt_ids: vec![dt_id],
                dt_positions: vec![dt_pos],
            },
        );
        Ok(())
    }

    /// Append one record to the currently open DTBLOCK for the given channel group.
    pub fn write_record(&mut self, cg_id: &str, values: &[DecodedValue]) -> Result<(), MdfError> {
        // first check block size without holding a mutable borrow on self
        let potential_new_block = {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                MdfError::BlockSerializationError("no open DT block for this channel group".into())
            })?;
            if values.len() != dt.channels.len() {
                return Err(MdfError::BlockSerializationError("value count mismatch".into()));
            }
            24 + dt.record_size * (dt.record_count as usize + 1) > MAX_DT_BLOCK_SIZE
        };

        if potential_new_block {
            // retrieve info needed to finalize the current block
            let (start_pos, record_count, record_size) = {
                let dt = self.open_dts.get(cg_id).unwrap();
                (dt.start_pos, dt.record_count, dt.record_size)
            };
            let size = 24 + record_size * record_count as usize;
            self.update_link(start_pos + 8, size as u64)?;

            // start new DT block
            let header = BlockHeader {
                id: "##DT".to_string(),
                reserved0: 0,
                block_len: 24,
                links_nr: 0,
            };
            let header_bytes = header.to_bytes()?;
            let dt_count = self
                .block_positions
                .keys()
                .filter(|k| k.starts_with("dt_"))
                .count();
            let new_dt_id = format!("dt_{}", dt_count);
            let new_dt_pos = self.write_block_with_id(&header_bytes, &new_dt_id)?;

            let dt = self.open_dts.get_mut(cg_id).unwrap();
            dt.dt_id = new_dt_id.clone();
            dt.start_pos = new_dt_pos;
            dt.record_count = 0;
            dt.dt_ids.push(new_dt_id);
            dt.dt_positions.push(new_dt_pos);
        }

        let dt = self.open_dts.get_mut(cg_id).unwrap();
        if values.len() != dt.channels.len() {
            return Err(MdfError::BlockSerializationError("value count mismatch".into()));
        }

        let mut buf = vec![0u8; dt.record_size];

        for (ch, val) in dt.channels.iter().zip(values.iter()) {
            let offset = dt.record_id_len + ch.byte_offset as usize;
            match (&ch.data_type, val) {
                (DataType::UnsignedIntegerLE, DecodedValue::UnsignedInteger(v)) => {
                    let bytes = (*v).to_le_bytes();
                    let n = ((ch.bit_count + 7) / 8) as usize;
                    buf[offset..offset + n].copy_from_slice(&bytes[..n]);
                }
                (DataType::SignedIntegerLE, DecodedValue::SignedInteger(v)) => {
                    let bytes = (*v as i64).to_le_bytes();
                    let n = ((ch.bit_count + 7) / 8) as usize;
                    buf[offset..offset + n].copy_from_slice(&bytes[..n]);
                }
                (DataType::FloatLE, DecodedValue::Float(v)) => {
                    if ch.bit_count == 32 {
                        buf[offset..offset + 4].copy_from_slice(&(*v as f32).to_le_bytes());
                    } else if ch.bit_count == 64 {
                        buf[offset..offset + 8].copy_from_slice(&v.to_le_bytes());
                    }
                }
                _ => {}
            }
        }

        self.file.write_all(&buf)?;
        dt.record_count += 1;
        self.offset += buf.len() as u64;
        Ok(())
    }

    /// Finalize the currently open DTBLOCK for a given channel group and patch its size field.
    pub fn finish_data_block(&mut self, cg_id: &str) -> Result<(), MdfError> {
        let mut dt = self.open_dts.remove(cg_id).ok_or_else(|| {
            MdfError::BlockSerializationError("no open DT block for this channel group".into())
        })?;
        // finalize the current DT block
        let size = 24 + dt.record_size as u64 * dt.record_count;
        self.update_link(dt.start_pos + 8, size)?;

        // If multiple DT blocks were created, generate a DLBLOCK referencing them all
        if dt.dt_ids.len() > 1 {
            let dl_count = self
                .block_positions
                .keys()
                .filter(|k| k.starts_with("dl_"))
                .count();
            let dl_id = format!("dl_{}", dl_count);
            let dl_block = DataListBlock::new(dt.dt_positions.clone());
            let dl_bytes = dl_block.to_bytes()?;
            let _pos = self.write_block_with_id(&dl_bytes, &dl_id)?;

            // Patch DG to point to the DL instead of the first DT
            let dg_data_link_offset = 40;
            self.update_block_link(&dt.dg_id, dg_data_link_offset, &dl_id)?;
        }

        Ok(())
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
        let cg_block = ChannelGroupBlock::default();
        let cg_id = writer.add_channel_group(&dg_id, None, &cg_block)?;

        // Add two channels to the channel group
        let mut ch1 = ChannelBlock::default();
        ch1.byte_offset = 0;
        ch1.bit_count = 32;
        ch1.data_type = DataType::UnsignedIntegerLE;
        ch1.name = Some("Channel 1".to_string());
        let cn1_id = writer.add_channel(&cg_id, None, &ch1)?;
        let mut ch2 = ChannelBlock::default();
        ch2.byte_offset = 4;
        ch2.bit_count = 32;
        ch2.data_type = DataType::UnsignedIntegerLE;
        ch2.name = Some("Channel 2".to_string());
        let _cn2_id = writer.add_channel(&cg_id, Some(&cn1_id), &ch2)?;
        
        // Finalize the file
        writer.finalize()
    }
}
