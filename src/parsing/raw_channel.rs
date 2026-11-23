use crate::blocks::channel_block::ChannelBlock;
use crate::error::MdfError;
use crate::parsing::decoder::{check_value_validity, vlsd_offset};
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::parsing::raw_data_group::RawDataGroup;
use crate::parsing::record_stream::{RecordCursor, VlsdStream};

/// A channel with lazy access to its raw record bytes (fixed-length or VLSD).
#[derive(Debug)]
pub struct RawChannel {
    pub block:  ChannelBlock,
}

impl<'a> RawChannel {

    /// Return an iterator over raw record bytes for this channel.
    ///
    /// The iterator yields one `Result` per record:
    /// * a fixed-length channel yields each whole record (record id, data
    ///   and invalidation bytes). A record split between two data blocks of
    ///   a `##DL` list cannot be returned as one slice of the file: the
    ///   iterator then yields an error and stops (read the channel through
    ///   [`Channel`](crate::api::channel::Channel) / `Mdf`, which reassemble
    ///   such records);
    /// * a VLSD channel yields the payload each record's offset points at in
    ///   the `##SD` stream (an invalid record whose offset does not resolve
    ///   yields an empty payload).
    ///
    /// # Arguments
    /// * `data_group` - Parent data group owning the records
    /// * `channel_group` - Channel group this channel belongs to
    /// * `mmap` - Memory mapped MDF data
    ///
    /// # Returns
    /// An iterator over byte slices containing each raw record, or an
    /// [`MdfError`] if the underlying blocks could not be parsed.
    pub fn records(
        &self,
        data_group: &'a RawDataGroup,
        channel_group: &'a RawChannelGroup,
        mmap: &'a [u8],
    ) -> Result<Box<dyn Iterator<Item = Result<&'a [u8], MdfError>> + 'a>, MdfError> {
        // VLSD: follow each record's offset into the SD stream, one record
        // at a time (nothing is collected up front).
        if self.block.channel_type == 1 && self.block.data != 0 {
            let record_id_len = data_group.block.record_id_len as usize;
            let cg_data_bytes = channel_group.block.samples_byte_nr;
            let mut records = RecordCursor::new(data_group.record_sections(channel_group, mmap)?);
            let stream = VlsdStream::new(mmap, self.block.data)?;
            // The iterator outlives `&self`: keep a copy of the layout fields
            // the validity check and the offset field need.
            let mut layout = ChannelBlock::default();
            layout.flags = self.block.flags;
            layout.pos_invalidation_bit = self.block.pos_invalidation_bit;
            layout.byte_offset = self.block.byte_offset;
            layout.bit_offset = self.block.bit_offset;
            layout.bit_count = self.block.bit_count;
            let mut failed = false;
            let payloads = std::iter::from_fn(move || {
                if failed {
                    return None;
                }
                let item = records.next_with(|record| {
                    let valid = check_value_validity(record, record_id_len, cg_data_bytes, &layout);
                    match vlsd_offset(&layout, record, record_id_len).and_then(|o| stream.entry(o)) {
                        Ok(payload) => Ok(payload),
                        // An invalid record whose offset does not resolve.
                        Err(_) if !valid => Ok(&[][..]),
                        Err(e) => Err(e),
                    }
                })?;
                failed = item.is_err();
                Some(item)
            });
            return Ok(Box::new(payloads));
        }

        // Compute the size of each record:
        // Record structure: record_id + data_bytes + invalidation_bytes
        let record_id_len       = data_group.block.record_id_len as usize;
        let sample_byte_len     = channel_group.block.samples_byte_nr as usize;
        let invalidation_bytes  = channel_group.block.invalidation_bytes_nr as usize;
        let record_size         = record_id_len + sample_byte_len + invalidation_bytes;

        // A record size of zero (malformed channel group) would make the
        // chunking below divide by zero / panic — there are no records.
        if record_size == 0 {
            return Ok(Box::new(std::iter::empty()));
        }

        // Gather all DataBlock fragments (DT or DV):
        let blocks: Vec<_> = data_group
            .data_blocks(mmap)?
            .into_iter()
            .filter(|b| !b.data.is_empty())
            .collect();
        let last = blocks.len().saturating_sub(1);

        // Whole records block by block. A block other than the last that
        // ends in a partial record means a record straddles two blocks: it
        // cannot be returned as one slice, so report it (instead of skipping
        // it and misaligning every later record) and stop.
        let iter = blocks
            .into_iter()
            .enumerate()
            .flat_map(move |(i, data_block)| {
                let raw = data_block.data;
                let whole = (raw.len() / record_size) * record_size;
                let split = (i < last && whole < raw.len()).then(|| {
                    Err(MdfError::BlockSerializationError(
                        "a record spans two data blocks of the ##DL list; RawChannel::records \
                         cannot return it as one slice — read the channel through \
                         Channel::decoded_values / Mdf::read, which reassemble it"
                            .to_string(),
                    ))
                });
                raw[..whole].chunks_exact(record_size).map(Ok).chain(split)
            })
            .scan(false, |failed, item| {
                if *failed {
                    return None;
                }
                *failed = item.is_err();
                Some(item)
            });

        Ok(Box::new(iter))
    }
}

