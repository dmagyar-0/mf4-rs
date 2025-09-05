// Low level file and block handling utilities for MdfWriter
use super::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write, BufWriter};
use memmap2::MmapMut;
use byteorder::{LittleEndian, WriteBytesExt};

struct MmapWriter {
    mmap: MmapMut,
    pos: usize,
}

impl MmapWriter {
    fn new(path: &str, size: usize) -> Result<Self, MdfError> {
        use std::fs::OpenOptions;
        let file = OpenOptions::new().read(true).write(true).create(true).open(path)?;
        file.set_len(size as u64)?;
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(MmapWriter { mmap, pos: 0 })
    }
}

impl Write for MmapWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let end = self.pos + buf.len();
        if end > self.mmap.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::WriteZero, "mmap overflow"));
        }
        self.mmap[self.pos..end].copy_from_slice(buf);
        self.pos = end;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.mmap.flush()
    }
}

impl Seek for MmapWriter {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new_pos: i64 = match pos {
            SeekFrom::Start(x) => x as i64,
            SeekFrom::End(x) => self.mmap.len() as i64 + x,
            SeekFrom::Current(x) => self.pos as i64 + x,
        };
        if new_pos < 0 || new_pos as usize > self.mmap.len() {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "invalid seek"));
        }
        self.pos = new_pos as usize;
        Ok(self.pos as u64)
    }
}

impl MdfWriter {
    /// Creates a new MdfWriter for the given file path using a 1 MB internal
    /// buffer. Use [`new_with_capacity`] to customize the buffer size.
    pub fn new(path: &str) -> Result<Self, MdfError> {
        Self::new_with_capacity(path, 1_048_576)
    }

    /// Creates a new MdfWriter with the specified `BufWriter` capacity.
    pub fn new_with_capacity(path: &str, capacity: usize) -> Result<Self, MdfError> {
        let file = File::create(path)?;
        let file = BufWriter::with_capacity(capacity, file);
        Ok(MdfWriter {
            file: Box::new(file),
            offset: 0,
            block_positions: HashMap::new(),
            open_dts: HashMap::new(),
            dt_counter: 0,
            last_dg: None,
            cg_to_dg: HashMap::new(),
            cg_offsets: HashMap::new(),
            cg_channels: HashMap::new(),
            channel_map: HashMap::new(),
        })
    }

    /// Creates a new MdfWriter backed by a memory-mapped file of the given size.
    pub fn new_mmap(path: &str, size: usize) -> Result<Self, MdfError> {
        let writer = MmapWriter::new(path, size)?;
        Ok(MdfWriter {
            file: Box::new(writer),
            offset: 0,
            block_positions: HashMap::new(),
            open_dts: HashMap::new(),
            dt_counter: 0,
            last_dg: None,
            cg_to_dg: HashMap::new(),
            cg_offsets: HashMap::new(),
            cg_channels: HashMap::new(),
            channel_map: HashMap::new(),
        })
    }

    /// Writes a block to the file, aligning to 8 bytes and zero-padding as needed.
    /// Returns the starting offset of the block in the file.
    pub fn write_block(&mut self, block_bytes: &[u8]) -> Result<u64, MdfError> {
        let align = (8 - (self.offset % 8)) % 8;
        if align != 0 {
            let padding = vec![0u8; align as usize];
            self.file.write_all(&padding)?;
            self.offset += align;
        }

        self.file.write_all(block_bytes)?;
        let block_start = self.offset;
        self.offset += block_bytes.len() as u64;
        Ok(block_start)
    }

    /// Writes a block to the file and tracks its position with the given ID.
    pub fn write_block_with_id(&mut self, block_bytes: &[u8], block_id: &str) -> Result<u64, MdfError> {
        let block_start = self.write_block(block_bytes)?;
        self.block_positions.insert(block_id.to_string(), block_start);
        Ok(block_start)
    }

    /// Retrieves the file position of a previously written block.
    pub fn get_block_position(&self, block_id: &str) -> Option<u64> {
        self.block_positions.get(block_id).copied()
    }

    /// Updates a link (u64 address) at a specific offset in the file.
    pub fn update_link(&mut self, offset: u64, address: u64) -> Result<(), MdfError> {
        let current_pos = self.offset;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_u64::<LittleEndian>(address)?;
        self.file.seek(SeekFrom::Start(current_pos))?;
        Ok(())
    }

    /// Updates a link using block IDs instead of raw offsets.
    pub fn update_block_link(&mut self, source_id: &str, link_offset: u64, target_id: &str) -> Result<(), MdfError> {
        let source_pos = self.get_block_position(source_id)
            .ok_or_else(|| MdfError::BlockLinkError(format!("Source block '{}' not found", source_id)))?;
        let target_pos = self.get_block_position(target_id)
            .ok_or_else(|| MdfError::BlockLinkError(format!("Target block '{}' not found", target_id)))?;
        let link_pos = source_pos + link_offset;
        self.update_link(link_pos, target_pos)
    }

    fn update_u32(&mut self, offset: u64, value: u32) -> Result<(), MdfError> {
        let current_pos = self.offset;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_u32::<LittleEndian>(value)?;
        self.file.seek(SeekFrom::Start(current_pos))?;
        Ok(())
    }

    fn update_u64(&mut self, offset: u64, value: u64) -> Result<(), MdfError> {
        let current_pos = self.offset;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_u64::<LittleEndian>(value)?;
        self.file.seek(SeekFrom::Start(current_pos))?;
        Ok(())
    }

    fn update_u8(&mut self, offset: u64, value: u8) -> Result<(), MdfError> {
        let current_pos = self.offset;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_u8(value)?;
        self.file.seek(SeekFrom::Start(current_pos))?;
        Ok(())
    }

    pub(super) fn update_block_u32(&mut self, block_id: &str, field_offset: u64, value: u32) -> Result<(), MdfError> {
        let block_pos = self.get_block_position(block_id)
            .ok_or_else(|| MdfError::BlockLinkError(format!("Block '{}' not found", block_id)))?;
        self.update_u32(block_pos + field_offset, value)
    }

    pub(super) fn update_block_u8(&mut self, block_id: &str, field_offset: u64, value: u8) -> Result<(), MdfError> {
        let block_pos = self.get_block_position(block_id)
            .ok_or_else(|| MdfError::BlockLinkError(format!("Block '{}' not found", block_id)))?;
        self.update_u8(block_pos + field_offset, value)
    }

    pub(super) fn update_block_u64(&mut self, block_id: &str, field_offset: u64, value: u64) -> Result<(), MdfError> {
        let block_pos = self.get_block_position(block_id)
            .ok_or_else(|| MdfError::BlockLinkError(format!("Block '{}' not found", block_id)))?;
        self.update_u64(block_pos + field_offset, value)
    }

    /// Returns the current file offset (for block address calculation).
    pub fn offset(&self) -> u64 { self.offset }

    /// Finalizes the file (flushes all data to disk).
    pub fn finalize(mut self) -> Result<(), MdfError> {
        self.file.flush()?;
        Ok(())
    }
}
