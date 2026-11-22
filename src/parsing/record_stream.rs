//! Record-level access to sample data held in memory (a memory map or an
//! owned buffer): the fixed-length records of a channel group, and the
//! variable-length (VLSD) signal-data stream of a channel.
//!
//! Both views borrow from the file bytes; nothing is copied except a record
//! that straddles a data-block boundary, which is reassembled in a small
//! buffer.

use crate::blocks::common::{BlockHeader, BlockParse};
use crate::blocks::data_block::DataBlock;
use crate::blocks::data_list_block::DataListBlock;
use crate::blocks::signal_data_block::SignalDataBlock;
use crate::error::MdfError;

/// The data sections of a channel group's data blocks, read as one
/// continuous stream of fixed-size records.
///
/// The blocks of a `##DL` list form one logical byte stream, so a record may
/// start in one block and end in the next (or a later) one; such records are
/// reassembled. A trailing partial record (a truncated recording) is ignored.
pub(crate) struct RecordSections<'a> {
    /// Non-empty data sections, in stream order.
    sections: Vec<&'a [u8]>,
    /// Stream offset of each section's first byte.
    starts: Vec<u64>,
    record_size: usize,
    records: u64,
}

impl<'a> RecordSections<'a> {
    /// View the data sections of `blocks` as records of `record_size` bytes
    /// (record id + data bytes + invalidation bytes). A record size of 0
    /// holds no records.
    pub(crate) fn new(blocks: Vec<DataBlock<'a>>, record_size: usize) -> Self {
        let mut sections = Vec::with_capacity(blocks.len());
        let mut starts = Vec::with_capacity(blocks.len());
        let mut total = 0u64;
        for block in blocks {
            if block.data.is_empty() {
                continue;
            }
            starts.push(total);
            total += block.data.len() as u64;
            sections.push(block.data);
        }
        let records = if record_size == 0 { 0 } else { total / record_size as u64 };
        RecordSections { sections, starts, record_size, records }
    }

    /// Number of whole records in the stream.
    pub(crate) fn record_count(&self) -> u64 {
        self.records
    }

    /// Call `f` with every record of `[start, end)` (clamped to the record
    /// count), in order. Records inside one block are passed as slices of
    /// the file; a record split across blocks is passed reassembled.
    pub(crate) fn for_each(
        &self,
        start: u64,
        end: u64,
        mut f: impl FnMut(&[u8]) -> Result<(), MdfError>,
    ) -> Result<(), MdfError> {
        let end = end.min(self.records);
        if start >= end {
            return Ok(());
        }
        let rs = self.record_size;
        let mut pos = start * rs as u64;
        let end_pos = end * rs as u64;
        // Last section starting at or before `pos` (sections are non-empty).
        let mut i = self.starts.partition_point(|&s| s <= pos) - 1;
        let mut carry: Vec<u8> = Vec::new();
        while pos < end_pos {
            let section = self.sections[i];
            let offset = (pos - self.starts[i]) as usize;
            let available = section.len() - offset;
            if carry.is_empty() {
                // `pos` is at a record boundary: pass the section's whole
                // records straight through, keep a trailing partial one.
                let wanted = usize::try_from(end_pos - pos).unwrap_or(usize::MAX);
                let span = available.min(wanted);
                let whole = span / rs * rs;
                for record in section[offset..offset + whole].chunks_exact(rs) {
                    f(record)?;
                }
                carry.extend_from_slice(&section[offset + whole..offset + span]);
                pos += span as u64;
            } else {
                let take = (rs - carry.len()).min(available);
                carry.extend_from_slice(&section[offset..offset + take]);
                pos += take as u64;
                if carry.len() == rs {
                    f(&carry)?;
                    carry.clear();
                }
            }
            if pos - self.starts[i] == section.len() as u64 {
                i += 1;
            }
        }
        Ok(())
    }
}

/// A VLSD channel's signal data: the data sections of its `##SD` blocks (one
/// `##SD`, or a `##DL` chain of them) as one virtual stream of
/// `[u32 length][payload]` entries, addressed by the offset each record
/// stores in the channel's fixed-length field.
pub(crate) struct VlsdStream<'a> {
    fragments: Vec<&'a [u8]>,
    /// Virtual stream offset of each fragment's first byte.
    starts: Vec<u64>,
    len: u64,
}

impl<'a> VlsdStream<'a> {
    /// Locate the signal data a VLSD channel's `data` link points at.
    pub(crate) fn new(mmap: &'a [u8], data_addr: u64) -> Result<Self, MdfError> {
        let mut stream = VlsdStream { fragments: Vec::new(), starts: Vec::new(), len: 0 };
        let mut visited = std::collections::HashSet::new();
        let mut next = data_addr;
        while next != 0 {
            if !visited.insert(next) {
                return Err(MdfError::BlockLinkError(format!(
                    "cycle detected in VLSD data chain at address {next:#x}"
                )));
            }
            let at = block_at(mmap, next)?;
            match BlockHeader::from_bytes(at)?.id.as_str() {
                "##SD" => {
                    stream.push(SignalDataBlock::from_bytes(at)?.data);
                    next = 0;
                }
                "##DL" => {
                    let list = DataListBlock::from_bytes(at)?;
                    for &fragment in &list.data_links {
                        if fragment != 0 {
                            stream.push(SignalDataBlock::from_bytes(block_at(mmap, fragment)?)?.data);
                        }
                    }
                    next = list.next;
                }
                other => {
                    return Err(MdfError::BlockIDError {
                        actual: other.to_string(),
                        expected: "##DL or ##SD".to_string(),
                    });
                }
            }
        }
        Ok(stream)
    }

    fn push(&mut self, data: &'a [u8]) {
        self.starts.push(self.len);
        self.len += data.len() as u64;
        self.fragments.push(data);
    }

    /// The payload of the entry at virtual `offset`. An entry must lie within
    /// one fragment (as the index reader requires).
    pub(crate) fn entry(&self, offset: u64) -> Result<&'a [u8], MdfError> {
        if offset >= self.len {
            return Err(MdfError::BlockSerializationError(format!(
                "VLSD inline offset {offset} is past the end of the {}-byte signal-data stream",
                self.len
            )));
        }
        let index = self.starts.partition_point(|&s| s <= offset) - 1;
        let fragment = self.fragments[index];
        let pos = (offset - self.starts[index]) as usize;
        let Some(prefix) = fragment.get(pos..pos + 4) else {
            return Err(MdfError::BlockSerializationError(format!(
                "VLSD entry at virtual offset {offset} is truncated: no room for its length \
                 prefix in fragment {index}"
            )));
        };
        let len = u32::from_le_bytes(prefix.try_into().unwrap()) as usize;
        fragment.get(pos + 4..).and_then(|rest| rest.get(..len)).ok_or_else(|| {
            MdfError::BlockSerializationError(format!(
                "VLSD entry at virtual offset {offset} declares length {len} which exceeds \
                 fragment {index} (size {})",
                fragment.len()
            ))
        })
    }
}

/// The file bytes from `addr` on, or an error when `addr` is past the end.
fn block_at(mmap: &[u8], addr: u64) -> Result<&[u8], MdfError> {
    usize::try_from(addr).ok().and_then(|a| mmap.get(a..)).filter(|b| b.len() >= 24).ok_or(
        MdfError::TooShortBuffer {
            actual: mmap.len(),
            expected: (addr as usize).saturating_add(24),
            file: file!(),
            line: line!(),
        },
    )
}
