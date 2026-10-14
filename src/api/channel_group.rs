use crate::blocks::common::read_string_block;
use crate::parsing::raw_data_group::RawDataGroup;
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::parsing::source_info::SourceInfo;
use crate::api::channel::Channel;
use crate::error::MdfError;
use crate::signal::Signal;

/// High level wrapper for a channel group.
///
/// The struct references raw channel group data and provides ergonomic access
/// to its metadata and channels without decoding any actual samples.
pub struct ChannelGroup<'a> {
    raw_data_group:    &'a RawDataGroup,
    raw_channel_group: &'a RawChannelGroup,
    mmap:              &'a [u8],
}

impl<'a> ChannelGroup<'a> {
    /// Create a new [`ChannelGroup`] referencing the underlying raw blocks.
    ///
    /// # Arguments
    /// * `raw_data_group` - Parent data group containing this channel group
    /// * `raw_channel_group` - The raw channel group block
    /// * `mmap` - Memory mapped file backing all data
    ///
    /// # Returns
    /// A [`ChannelGroup`] handle with no decoded data.
    pub fn new(
        raw_data_group: &'a RawDataGroup,
        raw_channel_group: &'a RawChannelGroup,
        mmap: &'a [u8],
    ) -> Self {
        ChannelGroup { raw_data_group, raw_channel_group, mmap }
    }

    /// Retrieve the human readable group name.
    pub fn name(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.raw_channel_group.block.acq_name_addr)
    }

    /// Retrieve the group comment if present.
    pub fn comment(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.raw_channel_group.block.comment_addr)
    }

    /// Get the acquisition source information if available.
    pub fn source(&self) -> Result<Option<SourceInfo>, MdfError> {
        let addr = self.raw_channel_group.block.acq_source_addr;
        SourceInfo::from_mmap(self.mmap, addr)
    }

    /// Build all [`Channel`] objects for this group.
    ///
    /// No channel data is decoded; the returned channels simply reference the
    /// raw blocks.
    pub fn channels(&self) -> Vec<Channel<'a>> {

        let mut channels = Vec::new();
        for raw_channel in &self.raw_channel_group.raw_channels {
            let channel = Channel::new(
                &raw_channel.block,
                self.raw_data_group,
                self.raw_channel_group,
                raw_channel,
                self.mmap,
            );
            channels.push(channel);
        }

        channels
    }

    /// Find a channel in this group by name (first match).
    pub fn channel(&self, name: &str) -> Option<Channel<'a>> {
        self.channels()
            .into_iter()
            .find(|c| c.name().ok().flatten().as_deref() == Some(name))
    }

    /// Read a channel by name as a [`Signal`] (values paired with the group's
    /// master/time axis).
    ///
    /// Returns `Ok(None)` if no channel with that name exists in this group.
    /// `timestamps` is empty when the group has no master channel or when the
    /// requested channel *is* the master.
    pub fn signal(&self, name: &str) -> Result<Option<Signal>, MdfError> {
        let channels = self.channels();
        let mut target: Option<usize> = None;
        let mut master: Option<usize> = None;
        for (i, ch) in channels.iter().enumerate() {
            // First master wins (matches MdfIndex's master selection)
            if master.is_none() && ch.block().channel_type == 2 {
                master = Some(i);
            }
            if target.is_none() && ch.name()?.as_deref() == Some(name) {
                target = Some(i);
            }
        }
        let Some(ci) = target else { return Ok(None) };

        let values = channels[ci].values()?;
        let timestamps = match master {
            Some(mi) if mi != ci => channels[mi].values_as_f64()?,
            _ => Vec::new(),
        };
        Ok(Some(Signal {
            name: name.to_string(),
            unit: channels[ci].unit()?,
            timestamps,
            values,
        }))
    }

    /// Index of the first master channel (`channel_type == 2`), if any.
    pub fn master_index(&self) -> Option<usize> {
        self.raw_channel_group
            .raw_channels
            .iter()
            .position(|c| c.block.channel_type == 2)
    }

    /// Decode several channels of this group as physical `f64` in **one
    /// sweep** over the record data.
    ///
    /// `indices` are positions in [`channels`](Self::channels). Semantics per
    /// channel match [`Channel::values_as_f64`]. Fixed-length channels share
    /// a single pass (tiled, multi-threaded); VLSD channels fall back to
    /// their own iterator. Output order follows `indices`.
    pub fn values_f64_many(&self, indices: &[usize]) -> Result<Vec<Vec<f64>>, MdfError> {
        let raw = &self.raw_channel_group.raw_channels;
        if let Some(&bad) = indices.iter().find(|&&i| i >= raw.len()) {
            return Err(MdfError::BlockSerializationError(format!(
                "channel index {} out of range ({} channels)",
                bad,
                raw.len()
            )));
        }
        let is_vlsd = |i: usize| raw[i].block.channel_type == 1 && raw[i].block.data != 0;
        let fixed: Vec<usize> = indices.iter().copied().filter(|&i| !is_vlsd(i)).collect();
        let blocks: Vec<&crate::blocks::channel_block::ChannelBlock> =
            fixed.iter().map(|&i| &raw[i].block).collect();
        let mut decoded = crate::parsing::columnar::decode_columns_f64(
            self.raw_data_group,
            self.raw_channel_group,
            &blocks,
            self.mmap,
        )?
        .into_iter();

        let channels = self.channels();
        let mut out = Vec::with_capacity(indices.len());
        for &i in indices {
            if is_vlsd(i) {
                out.push(channels[i].values_as_f64()?);
            } else {
                out.push(decoded.next().expect("one column per fixed channel"));
            }
        }
        Ok(out)
    }

    /// Raw bytes of every sample of a byte-oriented channel (frame payloads
    /// such as `CAN_DataFrame.DataBytes` / `ETH_Frame.DataBytes`, MAC
    /// addresses, strings) as one contiguous buffer plus `n + 1` offsets:
    /// sample `i` is `data[offsets[i]..offsets[i + 1]]` (Arrow layout).
    ///
    /// Works for fixed-width channels (copied straight out of the records)
    /// and VLSD channels (`##SD` chains or VLSD channel groups). No
    /// conversion is applied and invalidation bits are not consulted.
    pub fn bytes_column(&self, index: usize) -> Result<(Vec<u8>, Vec<u64>), MdfError> {
        self.bytes_column_range(index, 0, usize::MAX)
    }

    /// [`bytes_column`](Self::bytes_column) restricted to samples
    /// `[start, stop)` (clamped to the available samples) — for streaming
    /// payloads of logs larger than memory in chunks. Seeking is O(1) for
    /// fixed-width channels and for VLSD payloads of unsorted logger files.
    pub fn bytes_column_range(
        &self,
        index: usize,
        start: usize,
        stop: usize,
    ) -> Result<(Vec<u8>, Vec<u64>), MdfError> {
        let raw = self.raw_channel_group.raw_channels.get(index).ok_or_else(|| {
            MdfError::BlockSerializationError(format!("channel index {} out of range", index))
        })?;
        let block = &raw.block;
        let mut data = Vec::new();
        let mut offsets = Vec::new();
        offsets.push(0u64);
        let stop = stop.max(start);

        if block.channel_type == 1 && block.data != 0 {
            use crate::parsing::raw_data_group::VlsdGroup;
            if let Some(gi) = crate::parsing::raw_channel::vlsd_channel_group_index(
                self.mmap, block.data, self.raw_data_group,
            )? {
                if let VlsdGroup::Offsets(src, offs) = self.raw_data_group.vlsd_group(gi, self.mmap)? {
                    let sel = &offs[start.min(offs.len())..stop.min(offs.len())];
                    offsets.reserve(sel.len());
                    for &o in sel {
                        let o = o as usize;
                        let len = u32::from_le_bytes(src[o..o + 4].try_into().unwrap()) as usize;
                        data.extend_from_slice(&src[o + 4..o + 4 + len]);
                        offsets.push(data.len() as u64);
                    }
                    return Ok((data, offsets));
                }
            }
            let iter = raw.records(self.raw_data_group, self.raw_channel_group, self.mmap)?;
            for payload in iter.skip(start).take(stop - start) {
                data.extend_from_slice(payload?);
                offsets.push(data.len() as u64);
            }
            return Ok((data, offsets));
        }

        if block.bit_offset != 0 || block.bit_count % 8 != 0 {
            return Err(MdfError::BlockSerializationError(format!(
                "bytes_column: channel is not byte-aligned (bit_offset {}, bit_count {})",
                block.bit_offset, block.bit_count
            )));
        }
        let rid = self.raw_data_group.block.record_id_len as usize;
        let cg = &self.raw_channel_group.block;
        let rs = rid + cg.samples_byte_nr as usize + cg.invalidation_bytes_nr as usize;
        let field = rid + block.byte_offset as usize;
        let width = (block.bit_count / 8) as usize;
        if rs == 0 || field + width > rs {
            return Err(MdfError::BlockSerializationError(
                "bytes_column: channel lies outside the record".into(),
            ));
        }
        let blocks = self.raw_data_group.data_blocks_for(self.raw_channel_group, self.mmap)?;
        let total: usize = blocks.iter().map(|b| b.data.len() / rs).sum();
        let (lo, hi) = (start.min(total), stop.min(total));
        data.reserve_exact((hi - lo) * width);
        let mut row = 0usize;
        for b in &blocks {
            let rows = b.data.len() / rs;
            let (a, z) = (lo.max(row), hi.min(row + rows));
            if a < z {
                for rec in b.data[(a - row) * rs..(z - row) * rs].chunks_exact(rs) {
                    data.extend_from_slice(&rec[field..field + width]);
                }
            }
            row += rows;
        }
        let n = hi - lo;
        offsets.extend((1..=n as u64).map(|i| i * width as u64));
        Ok((data, offsets))
    }

    /// Get the raw data group (for internal use)
    pub fn raw_data_group(&self) -> &RawDataGroup {
        self.raw_data_group
    }

    /// Get the raw channel group (for internal use) 
    pub fn raw_channel_group(&self) -> &RawChannelGroup {
        self.raw_channel_group
    }

    /// Get the memory mapped data (for internal use)
    pub fn mmap(&self) -> &[u8] {
        self.mmap
    }
}
