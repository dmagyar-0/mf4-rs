use byteorder::{ByteOrder, LittleEndian};

use crate::blocks::common::BlockHeader;
use crate::blocks::common::BlockParse;
use crate::blocks::channel_block::ChannelBlock;
use crate::error::MdfError;

#[derive(Debug)]
pub struct ChannelGroupBlock {
    pub header: BlockHeader, // Common header
    pub next_cg_addr: u64,                // 8 bytes
    pub first_ch_addr: u64,               // 8 bytes
    pub acq_name_addr: u64,               // 8 bytes
    pub acq_source_addr: u64,             // 8 bytes
    pub first_sample_reduction_addr: u64, // 8 bytes
    pub comment_addr: u64,                // 8 bytes
    pub record_id: u64,                   // 8 bytes
    pub cycles_nr: u64,                   // 8 bytes
    pub flags: u16,                       // 2 bytes
    pub path_separator: u16,              // 2 bytes
    pub reserved1: u32,                   // 4 bytes
    pub samples_byte_nr: u32,             // 4 bytes
    pub invalidation_bytes_nr: u32,       // 4 bytes
    /// `cg_cg_master` (MDF 4.2): channel group holding this group's master
    /// channel when the *remote master* flag (bit 3 of `flags`) is set; 0
    /// otherwise. Stored as a 7th link, so such a block is 112 bytes long.
    pub master_cg_addr: u64,
}

/// Channel-group flag (MDF 4.2): the master channel is in the channel group
/// referenced by [`ChannelGroupBlock::master_cg_addr`].
pub const CG_FLAG_REMOTE_MASTER: u16 = 1 << 3;

impl BlockParse<'_> for ChannelGroupBlock {
    const ID: &'static str = "##CG";
    /// Creates a ChannelGroupBlock from a 104-byte slice (112 bytes for an
    /// MDF 4.2 block carrying the `cg_cg_master` link).
    fn from_bytes(bytes: &[u8]) -> Result<Self, MdfError> {

        let header = Self::parse_header(bytes)?;

        // The data section follows the links; MDF 4.2 appends `cg_cg_master`
        // as a 7th link, shifting it by 8 bytes.
        let d = header
            .links_nr
            .checked_mul(8)
            .and_then(|l| l.checked_add(24))
            .filter(|&d| header.links_nr >= 6 && d.checked_add(32).is_some_and(|end| end <= header.block_len))
            .and_then(|d| usize::try_from(d).ok())
            .ok_or_else(|| {
                MdfError::BlockSerializationError(format!(
                    "##CG block has {} links but is {} bytes long",
                    header.links_nr, header.block_len
                ))
            })?;
        let links = header.links_nr as usize;
        let expected_bytes = d + 32;
        if bytes.len() < expected_bytes {
            return Err(MdfError::TooShortBuffer {
                actual:   bytes.len(),
                expected: expected_bytes,
                file:     file!(),
                line:     line!(),
            });
        }

        Ok(Self {
            header: header,
            next_cg_addr: LittleEndian::read_u64(&bytes[24..32]),
            first_ch_addr: LittleEndian::read_u64(&bytes[32..40]),
            acq_name_addr: LittleEndian::read_u64(&bytes[40..48]),
            acq_source_addr: LittleEndian::read_u64(&bytes[48..56]),
            first_sample_reduction_addr: LittleEndian::read_u64(&bytes[56..64]),
            comment_addr: LittleEndian::read_u64(&bytes[64..72]),
            record_id: LittleEndian::read_u64(&bytes[d..d + 8]),
            cycles_nr: LittleEndian::read_u64(&bytes[d + 8..d + 16]),
            flags: LittleEndian::read_u16(&bytes[d + 16..d + 18]),
            path_separator: LittleEndian::read_u16(&bytes[d + 18..d + 20]),
            reserved1: LittleEndian::read_u32(&bytes[d + 20..d + 24]),
            samples_byte_nr: LittleEndian::read_u32(&bytes[d + 24..d + 28]),
            invalidation_bytes_nr: LittleEndian::read_u32(&bytes[d + 28..d + 32]),
            master_cg_addr: if links >= 7 && LittleEndian::read_u16(&bytes[d + 16..d + 18]) & CG_FLAG_REMOTE_MASTER != 0 {
                LittleEndian::read_u64(&bytes[72..80])
            } else {
                0
            },
        })
    }
}
impl ChannelGroupBlock {
    
    /// Serializes the ChannelGroupBlock to bytes according to MDF 4.1 specification
    /// (112 bytes with the MDF 4.2 `cg_cg_master` 7th link when
    /// `master_cg_addr` is set or the block was parsed with 7 links; the
    /// header's length and link count are derived, not taken from `header`).
    /// 
    /// # Structure (104 bytes total):
    /// - BlockHeader (24 bytes): Standard block header with id="##CG"
    /// - Link section (48 bytes): Six 8-byte links to other blocks
    ///   * next_cg_addr: Link to next channel group block
    ///   * first_ch_addr: Link to first channel block
    ///   * acq_name_addr: Link to acquisition name text block
    ///   * acq_source_addr: Link to acquisition source block
    ///   * first_sample_reduction_addr: Link to first sample reduction block
    ///   * comment_addr: Link to comment text block
    /// - Data section (32 bytes): Information about the channel group data
    ///   * record_id: Record ID (u64)
    ///   * cycles_nr: Number of cycles (u64)
    ///   * flags: Flags (u16)
    ///   * path_separator: Path separator character (u16)
    ///   * reserved1: Reserved space (u32)
    ///   * samples_byte_nr: Number of bytes for samples (u32)
    ///   * invalidation_bytes_nr: Number of bytes for invalidation bits (u32)
    /// 
    /// # Returns
    /// - `Ok(Vec<u8>)` containing the serialized channel group block
    /// - `Err(MdfError)` if serialization fails
    pub fn to_bytes(&self) -> Result<Vec<u8>, MdfError> {
        // Validate the header before serializing
        if self.header.id != "##CG" {
            return Err(MdfError::BlockSerializationError(
                format!("ChannelGroupBlock must have ID '##CG', found '{}'", self.header.id)
            ));
        }
        
        // The link count follows from `master_cg_addr`: a remote master needs
        // the MDF 4.2 7th link (112 bytes). A block parsed with 7 links keeps
        // them (writing a null master link); otherwise it is the 4.1 104 bytes.
        let remote = self.master_cg_addr != 0;
        let seven = remote || self.header.links_nr >= 7;
        let expected_len: u64 = if seven { 112 } else { 104 };
        let header = BlockHeader {
            id: self.header.id.clone(),
            reserved0: self.header.reserved0,
            block_len: expected_len,
            links_nr: if seven { 7 } else { 6 },
        };
        let flags = if remote { self.flags | CG_FLAG_REMOTE_MASTER } else { self.flags & !CG_FLAG_REMOTE_MASTER };
        
        // Create a buffer with exact capacity for efficiency
        let mut buffer = Vec::with_capacity(expected_len as usize);
        
        // 1. Write the block header (24 bytes)
        buffer.extend_from_slice(&header.to_bytes()?);
        
        // 2. Write the link addresses (48 bytes total, 6 links at 8 bytes each)
        buffer.extend_from_slice(&self.next_cg_addr.to_le_bytes());           // Next channel group
        buffer.extend_from_slice(&self.first_ch_addr.to_le_bytes());          // First channel 
        buffer.extend_from_slice(&self.acq_name_addr.to_le_bytes());          // Acquisition name
        buffer.extend_from_slice(&self.acq_source_addr.to_le_bytes());        // Acquisition source
        buffer.extend_from_slice(&self.first_sample_reduction_addr.to_le_bytes()); // Sample reduction
        buffer.extend_from_slice(&self.comment_addr.to_le_bytes());           // Comment
        if seven {
            buffer.extend_from_slice(&self.master_cg_addr.to_le_bytes());     // Remote master (4.2)
        }
        
        // 3. Write the data section (32 bytes)
        buffer.extend_from_slice(&self.record_id.to_le_bytes());              // Record ID (8 bytes)
        buffer.extend_from_slice(&self.cycles_nr.to_le_bytes());              // Cycles count (8 bytes)
        buffer.extend_from_slice(&flags.to_le_bytes());                       // Flags (2 bytes)
        buffer.extend_from_slice(&self.path_separator.to_le_bytes());         // Path separator (2 bytes)
        buffer.extend_from_slice(&self.reserved1.to_le_bytes());              // Reserved (4 bytes)
        buffer.extend_from_slice(&self.samples_byte_nr.to_le_bytes());        // Sample bytes (4 bytes)
        buffer.extend_from_slice(&self.invalidation_bytes_nr.to_le_bytes());  // Invalidation bytes (4 bytes)
        
        if buffer.len() as u64 != expected_len {
            return Err(MdfError::BlockSerializationError(
                format!("ChannelGroupBlock must be exactly {} bytes, got {}", expected_len, buffer.len())
            ));
        }
        
        // Ensure 8-byte alignment (should always be true since 104 is divisible by 8)
        debug_assert_eq!(buffer.len() % 8, 0, "ChannelGroupBlock size is not 8-byte aligned");
        
        Ok(buffer)
    }
    
    /// Read all channels linked to this channel group.
    ///
    /// # Arguments
    /// * `mmap` - Memory mapped MDF data used to follow the channel chain.
    ///
    /// # Returns
    /// A vector of fully parsed [`ChannelBlock`]s or an [`MdfError`] if any
    /// channel cannot be decoded.
    pub fn read_channels(&mut self, mmap: &[u8]) -> Result<Vec<ChannelBlock>, MdfError> {
        let mut channels = Vec::new();
        let mut current_ch_addr = self.first_ch_addr;
        let mut visited = std::collections::HashSet::new();

        while current_ch_addr != 0 {
            if !visited.insert(current_ch_addr) {
                return Err(MdfError::BlockLinkError(format!(
                    "cycle detected in channel linked list at address {:#x}",
                    current_ch_addr
                )));
            }
            let ch_offset = current_ch_addr as usize;
            let bytes = mmap.get(ch_offset..).ok_or(MdfError::TooShortBuffer {
                actual:   mmap.len(),
                expected: ch_offset.saturating_add(160),
                file:     file!(),
                line:     line!(),
            })?;
            let mut channel = ChannelBlock::from_bytes(bytes)?;
            channel.resolve_conversion(mmap)?;
            current_ch_addr = channel.next_ch_addr;
            channels.push(channel);
        }

        Ok(channels)
    }
}

impl Default for ChannelGroupBlock {
    fn default() -> Self {
        let header = BlockHeader {
            id: String::from("##CG"),
            reserved0: 0,
            block_len: 104,
            links_nr: 6,
        };

        ChannelGroupBlock {
            header,
            next_cg_addr: 0,
            first_ch_addr: 0,
            acq_name_addr: 0,
            acq_source_addr: 0,
            first_sample_reduction_addr: 0,
            comment_addr: 0,
            record_id: 0,
            cycles_nr: 0,
            flags: 0,
            path_separator: 0,
            reserved1: 0,
            samples_byte_nr: 0,
            invalidation_bytes_nr: 0,
            master_cg_addr: 0,
        }
    }
}
