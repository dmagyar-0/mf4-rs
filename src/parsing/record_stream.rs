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

/// A lazy cursor over the records of a [`RecordSections`] stream, for
/// iterators that must not collect every record up front. A record split
/// between sections is reassembled in a small reusable buffer, so each record
/// is lent to a closure ([`next_with`](Self::next_with)) rather than returned.
pub(crate) struct RecordCursor<'a> {
    stream: RecordSections<'a>,
    next: u64,
    section: usize,
    buf: Vec<u8>,
}

impl<'a> RecordCursor<'a> {
    pub(crate) fn new(stream: RecordSections<'a>) -> Self {
        RecordCursor { stream, next: 0, section: 0, buf: Vec::new() }
    }

    /// `f` applied to the next record, or `None` past the last one.
    pub(crate) fn next_with<T>(&mut self, f: impl FnOnce(&[u8]) -> T) -> Option<T> {
        if self.next >= self.stream.records {
            return None;
        }
        let rs = self.stream.record_size;
        let pos = self.next * rs as u64;
        self.next += 1;
        // Sections are non-empty and the record lies within the stream.
        while self.stream.starts[self.section] + self.stream.sections[self.section].len() as u64 <= pos {
            self.section += 1;
        }
        let section = self.stream.sections[self.section];
        let offset = (pos - self.stream.starts[self.section]) as usize;
        if offset + rs <= section.len() {
            return Some(f(&section[offset..offset + rs]));
        }
        self.buf.clear();
        self.buf.extend_from_slice(&section[offset..]);
        let mut i = self.section + 1;
        while self.buf.len() < rs {
            let next = self.stream.sections[i];
            let take = (rs - self.buf.len()).min(next.len());
            self.buf.extend_from_slice(&next[..take]);
            i += 1;
        }
        Some(f(&self.buf))
    }
}

/// Reassembles fixed-size records from data sections fed in stream order,
/// for readers that receive the sections one at a time (the index's range
/// reads). A section need not hold whole records: [`feed`](Self::feed)
/// passes on runs of whole records — slices of the section where possible —
/// and keeps the start of a record split between sections until its end
/// arrives.
pub(crate) struct RecordCarry {
    record_size: usize,
    carry: Vec<u8>,
}

impl RecordCarry {
    pub(crate) fn new(record_size: usize) -> Self {
        RecordCarry { record_size, carry: Vec::new() }
    }

    /// Feed the next section; `f` receives whole records only (each slice a
    /// non-zero multiple of the record size), in order.
    pub(crate) fn feed(
        &mut self,
        mut data: &[u8],
        mut f: impl FnMut(&[u8]) -> Result<(), MdfError>,
    ) -> Result<(), MdfError> {
        let rs = self.record_size;
        if rs == 0 {
            return Ok(());
        }
        if !self.carry.is_empty() {
            let take = (rs - self.carry.len()).min(data.len());
            self.carry.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.carry.len() < rs {
                return Ok(());
            }
            f(&self.carry)?;
            self.carry.clear();
        }
        let whole = data.len() / rs * rs;
        if whole > 0 {
            f(&data[..whole])?;
        }
        self.carry.extend_from_slice(&data[whole..]);
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
