use std::borrow::Cow;

use crate::error::MdfError;
use crate::blocks::channel_block::ChannelBlock;
use crate::blocks::conversion::{ConversionBlock, ConversionType};
use crate::parsing::decoder::{ DecodedValue, decode_channel_value, decode_f64_from_record, check_value_validity, vlsd_offset };
use crate::parsing::raw_channel_group::RawChannelGroup;
use crate::parsing::raw_data_group::RawDataGroup;
use crate::parsing::raw_channel::RawChannel;
use crate::parsing::record_stream::{RecordSections, VlsdStream};
use crate::parsing::source_info::SourceInfo;
use crate::signal::decoded_opt_to_f64;
use crate::blocks::common::read_string_block;

/// High level handle for a single channel within a group.
///
/// It holds references to the raw blocks and allows convenient access to
/// metadata and decoded values.
pub struct Channel<'a> {
    block:          &'a ChannelBlock,
    raw_data_group:   &'a RawDataGroup,
    raw_channel_group:     &'a RawChannelGroup,
    #[allow(dead_code)] // kept for `Channel::new`'s signature; records are read via the group
    raw_channel:    &'a RawChannel,
    mmap:           &'a [u8],
}

impl<'a> Channel<'a> {
    /// Construct a new [`Channel`] from raw block references.
    ///
    /// # Arguments
    /// * `block` - Channel block containing metadata
    /// * `raw_data_group` - Parent data group
    /// * `raw_channel_group` - Parent channel group
    /// * `raw_channel` - Raw channel helper used to iterate samples
    /// * `mmap` - Memory mapped file backing all data
    ///
    /// # Returns
    /// A [`Channel`] handle with no samples decoded yet.
    pub fn new(
        block: &'a ChannelBlock,
        raw_data_group: &'a RawDataGroup,
        raw_channel_group: &'a RawChannelGroup,
        raw_channel: &'a RawChannel,
        mmap: &'a [u8],
    ) -> Self {
        Channel { block, raw_data_group, raw_channel_group, raw_channel, mmap }
    }
    /// Retrieve the channel name if present.
    pub fn name(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.block.name_addr)
    }

    /// Retrieve the physical unit description.
    pub fn unit(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.block.unit_addr)
    }

    /// Retrieve the channel comment if present.
    pub fn comment(&self) -> Result<Option<String>, MdfError> {
        read_string_block(self.mmap, self.block.comment_addr)
    }

    /// The channel's conversion with every referenced text and nested
    /// conversion resolved, or `None` when it has none.
    pub fn conversion(&self) -> Result<Option<crate::blocks::conversion::ConversionBlock>, MdfError> {
        let Some(mut conversion) = self.block.conversion.clone() else {
            return Ok(None);
        };
        conversion.resolve_all_dependencies(self.mmap)?;
        Ok(Some(conversion))
    }

    /// This channel's metadata as a plain [`ChannelInfo`](crate::types::ChannelInfo).
    pub fn info(&self) -> Result<crate::types::ChannelInfo, MdfError> {
        Ok(crate::types::ChannelInfo {
            name: self.name()?,
            unit: self.unit()?,
            comment: self.comment()?,
            data_type: self.block.data_type.name().to_string(),
            bit_count: self.block.bit_count,
            is_master: self.is_master(),
            is_vlsd: self.block.channel_type == 1 && self.block.data != 0,
            has_conversion: self.block.conversion_addr != 0,
        })
    }

    /// `true` when this is a master (time) channel (`cn_type` 2 or 3).
    pub fn is_master(&self) -> bool {
        matches!(self.block.channel_type, 2 | 3)
    }

    /// Get the acquisition source for this channel if available.
    pub fn source(&self) -> Result<Option<SourceInfo>, MdfError> {
        let addr = self.block.source_addr;
        SourceInfo::from_mmap(self.mmap, addr)
    }

    /// Decode and convert all samples of this channel.
    ///
    /// This method decodes all channel values and applies conversions.
    /// Invalid samples (as indicated by invalidation bits) are returned as `None`.
    ///
    /// # Returns
    /// A vector with one `Option<DecodedValue>` per record:
    /// - `Some(value)` for valid samples
    /// - `None` for invalid samples (invalidation bit set or decoding failed)
    pub fn decoded_values(&self) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        self.decoded_values_as(false)
    }

    /// [`decoded_values`](Self::decoded_values), or the stored values without
    /// the conversion when `raw`.
    pub(crate) fn decoded_values_as(&self, raw: bool) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        let prepared = self.prepare(raw)?;
        prepared.decoded(0, prepared.record_count())
    }

    /// Decode all samples as **physical** f64 values.
    ///
    /// This is significantly faster than `decoded_values()` for numeric channels
    /// because it avoids the `Option<DecodedValue>` wrapping. Conversions are
    /// applied (with a fast inline path for linear conversions) and samples
    /// flagged invalid via invalidation bits or the all-invalid channel flag
    /// are returned as NaN — matching `MdfIndex`'s `values_f64` semantics.
    /// Non-numeric results (e.g. text conversions) are returned as NaN.
    pub fn values(&self) -> Result<Vec<f64>, MdfError> {
        self.values_as(false)
    }

    /// [`values`](Self::values), or the stored values without the conversion
    /// when `raw`.
    pub(crate) fn values_as(&self, raw: bool) -> Result<Vec<f64>, MdfError> {
        let prepared = self.prepare(raw)?;
        prepared.values(0, prepared.record_count())
    }

    /// Locate this channel's records (and, for a VLSD channel, its signal
    /// data) and resolve its conversion — unless `raw` — so that any number
    /// of record windows can then be decoded without repeating that work.
    pub(crate) fn prepare(&self, raw: bool) -> Result<PreparedChannel<'a>, MdfError> {
        let block = self.block;
        let records = if matches!(block.channel_type, 3 | 6) {
            // Virtual channels store no bytes: the raw value is the record index.
            Records::Virtual(self.raw_channel_group.block.cycles_nr)
        } else {
            let sections = self.raw_data_group.record_sections(self.raw_channel_group, self.mmap)?;
            if block.channel_type == 1 && block.data != 0 {
                Records::Vlsd(sections, VlsdStream::new(self.mmap, block.data)?)
            } else {
                Records::Fixed(sections)
            }
        };
        let conversion = if raw { None } else { block.conversion.as_ref().map(|c| resolved(c, self.mmap)) };
        let linear = conversion.as_deref().and_then(|c| {
            (c.cc_type == ConversionType::Linear && c.cc_val.len() >= 2).then(|| (c.cc_val[0], c.cc_val[1]))
        });
        let has_conversion = conversion.as_deref().is_some_and(|c| c.cc_type != ConversionType::Identity);
        Ok(PreparedChannel {
            block,
            mmap: self.mmap,
            record_id_len: self.raw_data_group.block.record_id_len as usize,
            cg_data_bytes: self.raw_channel_group.block.samples_byte_nr,
            has_invalidation: self.raw_channel_group.block.invalidation_bytes_nr > 0,
            all_invalid: block.flags & 0x01 != 0,
            records,
            conversion,
            linear,
            has_conversion,
        })
    }

    /// Get the channel block (for internal use)
    pub fn block(&self) -> &ChannelBlock {
        self.block
    }
}

/// `conversion` with every referenced text and nested conversion resolved
/// up front, so applying it does not re-parse blocks for every sample. A
/// conversion without references (linear, rational, tables, …) needs no
/// resolution and is borrowed; one whose resolution fails is used as it is
/// (applying it then reads its references from the file, as before).
fn resolved<'a>(conversion: &'a ConversionBlock, mmap: &[u8]) -> Cow<'a, ConversionBlock> {
    if conversion.cc_ref.iter().all(|&r| r == 0) {
        return Cow::Borrowed(conversion);
    }
    let mut resolved = conversion.clone();
    match resolved.resolve_all_dependencies(mmap) {
        Ok(()) => Cow::Owned(resolved),
        Err(_) => Cow::Borrowed(conversion),
    }
}

/// Where a prepared channel's records come from.
enum Records<'a> {
    /// A virtual channel (`cn_type` 3 / 6): `count` records, no bytes.
    Virtual(u64),
    /// Fixed-length values inside the records.
    Fixed(RecordSections<'a>),
    /// VLSD: each record holds an offset into the signal-data stream.
    Vlsd(RecordSections<'a>, VlsdStream<'a>),
}

/// A [`Channel`] ready to decode any window of its records: records located,
/// VLSD stream located and conversion resolved once (see
/// [`Channel::prepare`]). Used for whole reads, windows and the single-record
/// probes of a time-window search alike, so they all decode identically.
pub(crate) struct PreparedChannel<'a> {
    block: &'a ChannelBlock,
    mmap: &'a [u8],
    record_id_len: usize,
    cg_data_bytes: u32,
    has_invalidation: bool,
    all_invalid: bool,
    records: Records<'a>,
    conversion: Option<Cow<'a, ConversionBlock>>,
    linear: Option<(f64, f64)>,
    has_conversion: bool,
}

impl PreparedChannel<'_> {
    /// Number of records the channel holds.
    pub(crate) fn record_count(&self) -> u64 {
        match &self.records {
            Records::Virtual(count) => *count,
            Records::Fixed(sections) | Records::Vlsd(sections, _) => sections.record_count(),
        }
    }

    /// The conversion applied to `value` (none for a raw read).
    fn physical(&self, value: DecodedValue) -> Result<DecodedValue, MdfError> {
        match &self.conversion {
            Some(conversion) => conversion.apply_decoded(value, self.mmap),
            None => Ok(value),
        }
    }

    /// `false` when `record`'s sample is flagged invalid.
    #[inline(always)]
    fn is_valid(&self, record: &[u8]) -> bool {
        !self.all_invalid
            && (!self.has_invalidation
                || check_value_validity(record, self.record_id_len, self.cg_data_bytes, self.block))
    }

    /// Decoded (and converted) values of records `[start, end)`; `None` marks
    /// an invalid or undecodable sample.
    pub(crate) fn decoded(&self, start: u64, end: u64) -> Result<Vec<Option<DecodedValue>>, MdfError> {
        let end = end.min(self.record_count());
        let mut out = Vec::with_capacity(end.saturating_sub(start).min(1 << 24) as usize);
        match &self.records {
            Records::Virtual(_) => {
                for i in start..end {
                    out.push(Some(self.physical(DecodedValue::UnsignedInteger(i))?));
                }
            }
            Records::Fixed(sections) => sections.for_each(start, end, |record| {
                out.push(match self.is_valid(record) {
                    true => match decode_channel_value(record, self.record_id_len, self.block) {
                        Some(value) => Some(self.physical(value)?),
                        None => None,
                    },
                    false => None,
                });
                Ok(())
            })?,
            Records::Vlsd(sections, stream) => sections.for_each(start, end, |record| {
                out.push(self.vlsd_value(record, stream)?);
                Ok(())
            })?,
        }
        Ok(out)
    }

    /// The value a VLSD record's offset points at, `None` when invalid.
    fn vlsd_value(&self, record: &[u8], stream: &VlsdStream<'_>) -> Result<Option<DecodedValue>, MdfError> {
        if !self.is_valid(record) {
            return Ok(None);
        }
        let payload = stream.entry(vlsd_offset(self.block, record, self.record_id_len)?)?;
        match decode_channel_value(payload, 0, self.block) {
            Some(value) => Ok(Some(self.physical(value)?)),
            None => Ok(None),
        }
    }

    /// Physical `f64` values of records `[start, end)`; invalid and
    /// non-numeric samples are `NaN`.
    pub(crate) fn values(&self, start: u64, end: u64) -> Result<Vec<f64>, MdfError> {
        let Records::Fixed(sections) = &self.records else {
            // Virtual and VLSD values go through the decoded path.
            return Ok(self.decoded(start, end)?.iter().map(decoded_opt_to_f64).collect());
        };
        let end = end.min(self.record_count());
        let mut out = Vec::with_capacity(end.saturating_sub(start).min(1 << 24) as usize);
        let (record_id_len, block) = (self.record_id_len, self.block);
        match (self.linear, self.has_conversion) {
            (Some((a, b)), _) => sections.for_each(start, end, |record| {
                out.push(match self.is_valid(record) {
                    true => a + b * decode_f64_from_record(record, record_id_len, block),
                    false => f64::NAN,
                });
                Ok(())
            })?,
            (None, true) => sections.for_each(start, end, |record| {
                let value = match self.is_valid(record) {
                    true => decode_channel_value(record, record_id_len, block),
                    false => None,
                };
                out.push(match value {
                    Some(value) => match self.physical(value)? {
                        DecodedValue::Float(v) => v,
                        DecodedValue::UnsignedInteger(v) => v as f64,
                        DecodedValue::SignedInteger(v) => v as f64,
                        _ => f64::NAN,
                    },
                    None => f64::NAN,
                });
                Ok(())
            })?,
            (None, false) => sections.for_each(start, end, |record| {
                out.push(match self.is_valid(record) {
                    true => decode_f64_from_record(record, record_id_len, block),
                    false => f64::NAN,
                });
                Ok(())
            })?,
        }
        Ok(out)
    }

    /// Physical `f64` value of record `i` (`NaN` past the end).
    pub(crate) fn value_at(&self, i: u64) -> Result<f64, MdfError> {
        Ok(self.values(i, i + 1)?.first().copied().unwrap_or(f64::NAN))
    }
}
