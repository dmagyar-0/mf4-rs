//! MDF File Indexing System
//!
//! This module provides functionality to create lightweight indexes of MDF files
//! that can be serialized to JSON and used later to read specific channel data
//! without parsing the entire file structure.

use serde::{Deserialize, Serialize};
use crate::api::mdf::MDF;
use crate::blocks::common::{DataType, BlockParse};
use crate::blocks::conversion::ConversionBlock;
use crate::error::MdfError;
use crate::parsing::decoder::{decode_channel_value, DecodedValue};

/// Represents the location and metadata of data blocks in the file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataBlockInfo {
    /// File offset where the data block starts
    pub file_offset: u64,
    /// Size of the data block in bytes
    pub size: u64,
    /// Whether this is a compressed block (DZ)
    pub is_compressed: bool,
}

/// Channel metadata needed for decoding values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedChannel {
    /// Channel name
    pub name: Option<String>,
    /// Physical unit
    pub unit: Option<String>,
    /// Data type of the channel
    pub data_type: DataType,
    /// Byte offset within each record
    pub byte_offset: u32,
    /// Bit offset within the byte
    pub bit_offset: u8,
    /// Number of bits for this channel
    pub bit_count: u32,
    /// Channel type (0=data, 1=VLSD, 2=master, etc.)
    pub channel_type: u8,
    /// Conversion block for unit conversion (if any)
    pub conversion: Option<ConversionBlock>,
    /// For VLSD channels: address of signal data blocks
    pub vlsd_data_address: Option<u64>,
}

/// Channel group metadata and layout information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedChannelGroup {
    /// Group name
    pub name: Option<String>,
    /// Comment
    pub comment: Option<String>,
    /// Size of record ID in bytes
    pub record_id_len: u8,
    /// Total size of each record in bytes (excluding record ID)
    pub record_size: u32,
    /// Number of records in this group
    pub record_count: u64,
    /// Channels in this group
    pub channels: Vec<IndexedChannel>,
    /// Data block locations for this channel group
    pub data_blocks: Vec<DataBlockInfo>,
}

/// Complete MDF file index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdfIndex {
    /// File size for validation
    pub file_size: u64,
    /// Channel groups in the file
    pub channel_groups: Vec<IndexedChannelGroup>,
}

/// Trait for reading byte ranges from different sources (files, HTTP, etc.)
pub trait ByteRangeReader {
    type Error;
    
    /// Read bytes from the specified range
    /// Returns the requested bytes or an error
    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, Self::Error>;
}

/// Local file reader implementation
pub struct FileRangeReader {
    file: std::fs::File,
}

impl FileRangeReader {
    pub fn new(file_path: &str) -> Result<Self, MdfError> {
        let file = std::fs::File::open(file_path)
            .map_err(|e| MdfError::IOError(e))?;
        Ok(Self { file })
    }
}

impl ByteRangeReader for FileRangeReader {
    type Error = MdfError;
    
    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, Self::Error> {
        use std::io::{Read, Seek, SeekFrom};
        
        self.file.seek(SeekFrom::Start(offset))
            .map_err(|e| MdfError::IOError(e))?;
        
        let mut buffer = vec![0u8; length as usize];
        self.file.read_exact(&mut buffer)
            .map_err(|e| MdfError::IOError(e))?;
        
        Ok(buffer)
    }
}

/// Example HTTP range reader (would be implemented in production)
/// ```rust,ignore
/// use mf4_rs::index::ByteRangeReader;
/// use mf4_rs::error::MdfError;
/// 
/// pub struct HttpRangeReader {
///     client: reqwest::blocking::Client,
///     url: String,
/// }
/// 
/// impl HttpRangeReader {
///     pub fn new(url: String) -> Self {
///         Self {
///             client: reqwest::blocking::Client::new(),
///             url,
///         }
///     }
/// }
/// 
/// impl ByteRangeReader for HttpRangeReader {
///     type Error = MdfError;
///     
///     fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, Self::Error> {
///         let range_header = format!("bytes={}-{}", offset, offset + length - 1);
///         
///         let response = self.client
///             .get(&self.url)
///             .header("Range", range_header)
///             .send()
///             .map_err(|e| MdfError::BlockSerializationError(format!("HTTP error: {}", e)))?;
///         
///         if !response.status().is_success() {
///             return Err(MdfError::BlockSerializationError(
///                 format!("HTTP error: {}", response.status())
///             ));
///         }
///         
///         let bytes = response.bytes()
///             .map_err(|e| MdfError::BlockSerializationError(format!("Response error: {}", e)))?;
///         
///         Ok(bytes.to_vec())
///     }
/// }
/// ```
pub struct _HttpRangeReaderExample;

impl MdfIndex {
    /// Create an index from an MDF file
    pub fn from_file(file_path: &str) -> Result<Self, MdfError> {
        let mdf = MDF::from_file(file_path)?;
        let file_size = std::fs::metadata(file_path)
            .map_err(|e| MdfError::IOError(e))?
            .len();

        let mut indexed_groups = Vec::new();

        for group in mdf.channel_groups() {
            let mut indexed_channels = Vec::new();
            
            // Index each channel in the group
            for channel in group.channels() {
                let block = channel.block();
                let indexed_channel = IndexedChannel {
                    name: channel.name()?,
                    unit: channel.unit()?,
                    data_type: block.data_type.clone(),
                    byte_offset: block.byte_offset,
                    bit_offset: block.bit_offset,
                    bit_count: block.bit_count,
                    channel_type: block.channel_type,
                    conversion: block.conversion.clone(),
                    vlsd_data_address: if block.channel_type == 1 && block.data != 0 {
                        Some(block.data)
                    } else {
                        None
                    },
                };
                indexed_channels.push(indexed_channel);
            }

            // Get data block information
            let data_blocks = Self::extract_data_blocks(&group)?;

            let indexed_group = IndexedChannelGroup {
                name: group.name()?,
                comment: group.comment()?,
                record_id_len: group.raw_data_group().block.record_id_len,
                record_size: group.raw_channel_group().block.samples_byte_nr,
                record_count: group.raw_channel_group().block.cycles_nr,
                channels: indexed_channels,
                data_blocks,
            };
            indexed_groups.push(indexed_group);
        }

        Ok(MdfIndex {
            file_size,
            channel_groups: indexed_groups,
        })
    }

    /// Extract data block information from a channel group
    fn extract_data_blocks(group: &crate::api::channel_group::ChannelGroup) -> Result<Vec<DataBlockInfo>, MdfError> {
        let mut data_blocks = Vec::new();
        let raw_data_group = group.raw_data_group();
        let mmap = group.mmap();
        
        // Start at the group's primary data pointer
        let mut current_block_address = raw_data_group.block.data_block_addr;
        while current_block_address != 0 {
            let byte_offset = current_block_address as usize;

            // Read the block header
            let block_header = crate::blocks::common::BlockHeader::from_bytes(&mmap[byte_offset..byte_offset + 24])?;

            match block_header.id.as_str() {
                "##DT" | "##DV" => {
                    // Single contiguous DataBlock
                    let data_block_info = DataBlockInfo {
                        file_offset: current_block_address,
                        size: block_header.block_len,
                        is_compressed: false,
                    };
                    data_blocks.push(data_block_info);
                    // No list to follow, we're done
                    current_block_address = 0;
                }
                "##DZ" => {
                    // Compressed data block  
                    let data_block_info = DataBlockInfo {
                        file_offset: current_block_address,
                        size: block_header.block_len,
                        is_compressed: true,
                    };
                    data_blocks.push(data_block_info);
                    current_block_address = 0;
                }
                "##DL" => {
                    // Fragmented list of data blocks
                    let data_list_block = crate::blocks::data_list_block::DataListBlock::from_bytes(&mmap[byte_offset..])?;

                    // Parse each fragment in this list
                    for &fragment_address in &data_list_block.data_links {
                        let fragment_offset = fragment_address as usize;
                        let fragment_header = crate::blocks::common::BlockHeader::from_bytes(&mmap[fragment_offset..fragment_offset + 24])?;
                        
                        let is_compressed = fragment_header.id == "##DZ";
                        let data_block_info = DataBlockInfo {
                            file_offset: fragment_address,
                            size: fragment_header.block_len,
                            is_compressed,
                        };
                        data_blocks.push(data_block_info);
                    }

                    // Move to the next DLBLOCK in the chain (0 = end)
                    current_block_address = data_list_block.next;
                }

                unexpected_id => {
                    return Err(MdfError::BlockIDError {
                        actual: unexpected_id.to_string(),
                        expected: "##DT / ##DV / ##DL / ##DZ".to_string(),
                    });
                }
            }
        }
        
        Ok(data_blocks)
    }

    /// Save the index to a JSON file
    pub fn save_to_file(&self, index_path: &str) -> Result<(), MdfError> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| MdfError::BlockSerializationError(format!("JSON serialization failed: {}", e)))?;
        
        std::fs::write(index_path, json)
            .map_err(|e| MdfError::IOError(e))?;
        
        Ok(())
    }

    /// Load an index from a JSON file
    pub fn load_from_file(index_path: &str) -> Result<Self, MdfError> {
        let json = std::fs::read_to_string(index_path)
            .map_err(|e| MdfError::IOError(e))?;
        
        let index: MdfIndex = serde_json::from_str(&json)
            .map_err(|e| MdfError::BlockSerializationError(format!("JSON deserialization failed: {}", e)))?;
        
        Ok(index)
    }

    /// Read channel values using the index and a byte range reader
    pub fn read_channel_values<R: ByteRangeReader<Error = MdfError>>(
        &self, 
        group_index: usize, 
        channel_index: usize,
        reader: &mut R
    ) -> Result<Vec<DecodedValue>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        // Handle VLSD channels differently
        if channel.channel_type == 1 && channel.vlsd_data_address.is_some() {
            return self.read_vlsd_channel_values(group, channel, reader);
        }

        // For regular channels, read from data blocks
        self.read_regular_channel_values(group, channel, reader)
    }

    /// Read values for a regular (non-VLSD) channel using byte range reader
    fn read_regular_channel_values<R: ByteRangeReader<Error = MdfError>>(
        &self,
        group: &IndexedChannelGroup,
        channel: &IndexedChannel,
        reader: &mut R,
    ) -> Result<Vec<DecodedValue>, MdfError> {
        let record_size = group.record_id_len as usize + group.record_size as usize;
        let mut values = Vec::new();

        // Read from each data block
        for data_block in &group.data_blocks {
            // Handle compression if needed
            if data_block.is_compressed {
                // TODO: Implement decompression for DZ blocks
                return Err(MdfError::BlockSerializationError(
                    "Compressed blocks not yet supported in index reader".to_string()
                ));
            }

            // Read the block data (skip 24-byte block header)
            let block_data = reader.read_range(
                data_block.file_offset + 24, 
                data_block.size - 24
            )?;

            // Process records in this block
            let record_count = block_data.len() / record_size;
            for i in 0..record_count {
                let record_start = i * record_size;
                let record_end = record_start + record_size;
                let record = &block_data[record_start..record_end];

                // Create a ChannelBlock for decoding
                let temp_channel_block = crate::blocks::channel_block::ChannelBlock {
                    header: crate::blocks::common::BlockHeader {
                        id: "##CN".to_string(),
                        reserved0: 0,
                        block_len: 160,
                        links_nr: 8,
                    },
                    next_ch_addr: 0,
                    component_addr: 0,
                    name_addr: 0,
                    source_addr: 0,
                    conversion_addr: 0,
                    data: 0,
                    unit_addr: 0,
                    comment_addr: 0,
                    channel_type: channel.channel_type,
                    sync_type: 0,
                    data_type: channel.data_type.clone(),
                    bit_offset: channel.bit_offset,
                    byte_offset: channel.byte_offset,
                    bit_count: channel.bit_count,
                    flags: 0,
                    pos_invalidation_bit: 0,
                    precision: 0,
                    reserved1: 0,
                    attachment_nr: 0,
                    min_raw_value: 0.0,
                    max_raw_value: 0.0,
                    lower_limit: 0.0,
                    upper_limit: 0.0,
                    lower_ext_limit: 0.0,
                    upper_ext_limit: 0.0,
                    name: channel.name.clone(),
                    conversion: channel.conversion.clone(),
                };

                if let Some(raw_value) = decode_channel_value(
                    record, 
                    group.record_id_len as usize, 
                    &temp_channel_block
                ) {
                    // Apply conversion if present
                    let final_value = if let Some(conversion) = &channel.conversion {
                        conversion.apply_decoded(raw_value, &[])?
                    } else {
                        raw_value
                    };
                    values.push(final_value);
                }
            }
        }

        Ok(values)
    }

    /// Read values for a VLSD channel
    fn read_vlsd_channel_values<R: ByteRangeReader<Error = MdfError>>(
        &self,
        _group: &IndexedChannelGroup,
        _channel: &IndexedChannel,
        _reader: &mut R,
    ) -> Result<Vec<DecodedValue>, MdfError> {
        // TODO: Implement VLSD channel reading
        Err(MdfError::BlockSerializationError(
            "VLSD channels not yet supported in index reader".to_string()
        ))
    }

    /// Get channel information for a specific group and channel
    pub fn get_channel_info(&self, group_index: usize, channel_index: usize) -> Option<&IndexedChannel> {
        self.channel_groups
            .get(group_index)?
            .channels
            .get(channel_index)
    }

    /// List all channel groups with their basic information
    pub fn list_channel_groups(&self) -> Vec<(usize, &str, usize)> {
        self.channel_groups
            .iter()
            .enumerate()
            .map(|(i, group)| {
                (i, group.name.as_deref().unwrap_or("<unnamed>"), group.channels.len())
            })
            .collect()
    }

    /// List all channels in a specific group
    pub fn list_channels(&self, group_index: usize) -> Option<Vec<(usize, &str, &DataType)>> {
        let group = self.channel_groups.get(group_index)?;
        Some(
            group.channels
                .iter()
                .enumerate()
                .map(|(i, ch)| (i, ch.name.as_deref().unwrap_or("<unnamed>"), &ch.data_type))
                .collect()
        )
    }

    /// Get the exact byte ranges needed to read all data for a specific channel
    /// 
    /// Returns a vector of (file_offset, length) tuples representing the byte ranges
    /// that need to be read from the file to get all data for the specified channel.
    /// 
    /// # Arguments
    /// * `group_index` - Index of the channel group
    /// * `channel_index` - Index of the channel within the group
    /// 
    /// # Returns
    /// * `Ok(Vec<(u64, u64)>)` - Vector of (offset, length) byte ranges
    /// * `Err(MdfError)` - If indices are invalid or channel type not supported
    pub fn get_channel_byte_ranges(
        &self,
        group_index: usize,
        channel_index: usize,
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        // Handle VLSD channels differently
        if channel.channel_type == 1 && channel.vlsd_data_address.is_some() {
            return Err(MdfError::BlockSerializationError(
                "VLSD channels not yet supported for byte range calculation".to_string()
            ));
        }

        // For regular channels, calculate byte ranges from data blocks
        self.calculate_regular_channel_byte_ranges(group, channel)
    }

    /// Get the exact byte ranges for a specific record range of a channel
    /// 
    /// This is useful when you only want to read a subset of records rather than all data.
    /// 
    /// # Arguments
    /// * `group_index` - Index of the channel group
    /// * `channel_index` - Index of the channel within the group
    /// * `start_record` - Starting record index (0-based)
    /// * `record_count` - Number of records to read
    /// 
    /// # Returns
    /// * `Ok(Vec<(u64, u64)>)` - Vector of (offset, length) byte ranges
    /// * `Err(MdfError)` - If indices are invalid, range is out of bounds, or channel type not supported
    pub fn get_channel_byte_ranges_for_records(
        &self,
        group_index: usize,
        channel_index: usize,
        start_record: u64,
        record_count: u64,
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        let group = self.channel_groups.get(group_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid group index".to_string()))?;
        
        let channel = group.channels.get(channel_index)
            .ok_or_else(|| MdfError::BlockSerializationError("Invalid channel index".to_string()))?;

        // Validate record range
        if start_record + record_count > group.record_count {
            return Err(MdfError::BlockSerializationError(
                format!("Record range {}-{} exceeds total records {}", 
                    start_record, start_record + record_count - 1, group.record_count)
            ));
        }

        // Handle VLSD channels differently
        if channel.channel_type == 1 && channel.vlsd_data_address.is_some() {
            return Err(MdfError::BlockSerializationError(
                "VLSD channels not yet supported for byte range calculation".to_string()
            ));
        }

        self.calculate_channel_byte_ranges_for_records(group, channel, start_record, record_count)
    }

    /// Calculate byte ranges for a regular (non-VLSD) channel for all records
    fn calculate_regular_channel_byte_ranges(
        &self,
        group: &IndexedChannelGroup,
        channel: &IndexedChannel,
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        self.calculate_channel_byte_ranges_for_records(group, channel, 0, group.record_count)
    }

    /// Calculate byte ranges for a regular channel for a specific record range
    fn calculate_channel_byte_ranges_for_records(
        &self,
        group: &IndexedChannelGroup,
        channel: &IndexedChannel,
        start_record: u64,
        record_count: u64,
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        let record_size = group.record_id_len as usize + group.record_size as usize;
        let channel_offset_in_record = group.record_id_len as usize + channel.byte_offset as usize;
        
        // Calculate how many bytes this channel needs per record
        let channel_bytes_per_record = if matches!(channel.data_type,
            DataType::StringLatin1 | DataType::StringUtf8 | DataType::StringUtf16LE | 
            DataType::StringUtf16BE | DataType::ByteArray | DataType::MimeSample | DataType::MimeStream)
        {
            channel.bit_count as usize / 8
        } else {
            ((channel.bit_offset as usize + channel.bit_count as usize + 7) / 8).max(1)
        };

        let mut byte_ranges = Vec::new();
        let mut records_processed = 0u64;
        
        for data_block in &group.data_blocks {
            if data_block.is_compressed {
                return Err(MdfError::BlockSerializationError(
                    "Compressed blocks not supported for byte range calculation".to_string()
                ));
            }

            let block_data_start = data_block.file_offset + 24; // Skip block header
            let block_data_size = data_block.size - 24;
            let records_in_block = block_data_size / record_size as u64;
            
            // Determine which records from this block we need
            let block_start_record = records_processed;
            let block_end_record = records_processed + records_in_block;
            
            let need_start = start_record.max(block_start_record);
            let need_end = (start_record + record_count).min(block_end_record);
            
            if need_start < need_end {
                // We need some records from this block
                let first_record_in_block = need_start - block_start_record;
                let last_record_in_block = need_end - block_start_record - 1;
                
                // Calculate byte range for the channel data in these records
                let first_channel_byte = block_data_start + 
                    first_record_in_block * record_size as u64 + 
                    channel_offset_in_record as u64;
                
                let last_channel_byte = block_data_start + 
                    last_record_in_block * record_size as u64 + 
                    channel_offset_in_record as u64 + 
                    channel_bytes_per_record as u64 - 1;
                
                let range_length = last_channel_byte - first_channel_byte + 1;
                byte_ranges.push((first_channel_byte, range_length));
            }
            
            records_processed = block_end_record;
            
            // Early exit if we've processed all needed records
            if records_processed >= start_record + record_count {
                break;
            }
        }
        
        Ok(byte_ranges)
    }

    /// Get a summary of byte ranges for a channel (total bytes, number of ranges)
    /// 
    /// This is useful for understanding the I/O pattern before actually reading.
    /// 
    /// # Returns
    /// * `(total_bytes, number_of_ranges)` - Total bytes to read and number of separate ranges
    pub fn get_channel_byte_summary(
        &self,
        group_index: usize,
        channel_index: usize,
    ) -> Result<(u64, usize), MdfError> {
        let ranges = self.get_channel_byte_ranges(group_index, channel_index)?;
        let total_bytes: u64 = ranges.iter().map(|(_, len)| len).sum();
        Ok((total_bytes, ranges.len()))
    }

    /// Find a channel group index by name
    /// 
    /// # Arguments
    /// * `group_name` - Name of the channel group to find
    /// 
    /// # Returns
    /// * `Some(group_index)` if found
    /// * `None` if not found
    pub fn find_channel_group_by_name(&self, group_name: &str) -> Option<usize> {
        self.channel_groups
            .iter()
            .enumerate()
            .find(|(_, group)| {
                group.name.as_deref() == Some(group_name)
            })
            .map(|(index, _)| index)
    }

    /// Find a channel index by name within a specific group
    /// 
    /// # Arguments
    /// * `group_index` - Index of the channel group to search in
    /// * `channel_name` - Name of the channel to find
    /// 
    /// # Returns
    /// * `Some(channel_index)` if found
    /// * `None` if group doesn't exist or channel not found
    pub fn find_channel_by_name(&self, group_index: usize, channel_name: &str) -> Option<usize> {
        let group = self.channel_groups.get(group_index)?;
        
        group.channels
            .iter()
            .enumerate()
            .find(|(_, channel)| {
                channel.name.as_deref() == Some(channel_name)
            })
            .map(|(index, _)| index)
    }

    /// Find a channel by name across all groups
    /// 
    /// # Arguments
    /// * `channel_name` - Name of the channel to find
    /// 
    /// # Returns
    /// * `Some((group_index, channel_index))` if found
    /// * `None` if not found
    pub fn find_channel_by_name_global(&self, channel_name: &str) -> Option<(usize, usize)> {
        for (group_index, group) in self.channel_groups.iter().enumerate() {
            for (channel_index, channel) in group.channels.iter().enumerate() {
                if channel.name.as_deref() == Some(channel_name) {
                    return Some((group_index, channel_index));
                }
            }
        }
        None
    }

    /// Find all channels with a given name across all groups
    /// 
    /// This is useful when the same channel name appears in multiple groups.
    /// 
    /// # Arguments
    /// * `channel_name` - Name of the channels to find
    /// 
    /// # Returns
    /// * `Vec<(group_index, channel_index)>` - All matching channels
    pub fn find_all_channels_by_name(&self, channel_name: &str) -> Vec<(usize, usize)> {
        let mut matches = Vec::new();
        
        for (group_index, group) in self.channel_groups.iter().enumerate() {
            for (channel_index, channel) in group.channels.iter().enumerate() {
                if channel.name.as_deref() == Some(channel_name) {
                    matches.push((group_index, channel_index));
                }
            }
        }
        
        matches
    }

    /// Read channel values by name using a byte range reader
    /// 
    /// Convenience method that finds the channel by name and reads its values.
    /// If multiple channels have the same name, uses the first one found.
    /// 
    /// # Arguments
    /// * `channel_name` - Name of the channel to read
    /// * `reader` - Byte range reader implementation
    /// 
    /// # Returns
    /// * `Ok(Vec<DecodedValue>)` - Channel values
    /// * `Err(MdfError)` - If channel not found or reading fails
    pub fn read_channel_values_by_name<R: ByteRangeReader<Error = MdfError>>(
        &self,
        channel_name: &str,
        reader: &mut R,
    ) -> Result<Vec<DecodedValue>, MdfError> {
        let (group_index, channel_index) = self.find_channel_by_name_global(channel_name)
            .ok_or_else(|| MdfError::BlockSerializationError(
                format!("Channel '{}' not found", channel_name)
            ))?;
        
        self.read_channel_values(group_index, channel_index, reader)
    }

    /// Get byte ranges for a channel by name
    /// 
    /// # Arguments
    /// * `channel_name` - Name of the channel
    /// 
    /// # Returns
    /// * `Ok(Vec<(u64, u64)>)` - Byte ranges as (offset, length) tuples
    /// * `Err(MdfError)` - If channel not found or calculation fails
    pub fn get_channel_byte_ranges_by_name(
        &self,
        channel_name: &str,
    ) -> Result<Vec<(u64, u64)>, MdfError> {
        let (group_index, channel_index) = self.find_channel_by_name_global(channel_name)
            .ok_or_else(|| MdfError::BlockSerializationError(
                format!("Channel '{}' not found", channel_name)
            ))?;
        
        self.get_channel_byte_ranges(group_index, channel_index)
    }

    /// Get channel information by name
    /// 
    /// # Arguments
    /// * `channel_name` - Name of the channel
    /// 
    /// # Returns
    /// * `Some((group_index, channel_index, &IndexedChannel))` - Channel info if found
    /// * `None` - If channel not found
    pub fn get_channel_info_by_name(&self, channel_name: &str) -> Option<(usize, usize, &IndexedChannel)> {
        let (group_index, channel_index) = self.find_channel_by_name_global(channel_name)?;
        let channel = self.get_channel_info(group_index, channel_index)?;
        Some((group_index, channel_index, channel))
    }
}
