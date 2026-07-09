// Handling of DT blocks and record writing
use super::*;
use std::io::Write;
use crate::blocks::common::{BlockHeader, DataType};
use crate::blocks::data_list_block::DataListBlock;
use crate::parsing::decoder::DecodedValue;

/// Column data for use with [`MdfWriter::write_columns`].
///
/// Each variant holds a slice of typed values for a single channel. All
/// columns passed to `write_columns` must have the same length (number of
/// records). The encoder for each channel must match the corresponding
/// `ColumnData` variant.
pub enum ColumnData<'a> {
    /// 64-bit IEEE 754 float values.
    F64(&'a [f64]),
    /// 32-bit IEEE 754 float values.
    F32(&'a [f32]),
    /// Unsigned 64-bit integer values.
    U64(&'a [u64]),
    /// Signed 64-bit integer values.
    I64(&'a [i64]),
}

pub(super) enum ChannelEncoder {
    UInt { offset: usize, bytes: usize },
    Int { offset: usize, bytes: usize },
    F32 { offset: usize },
    F64 { offset: usize },
    Bytes { offset: usize, bytes: usize },
    /// VLSD channel: writes a 64-bit running offset into the DT record at
    /// `offset`, and appends `[u32 length][payload]` to
    /// `OpenDataBlock::vlsd_payloads[channel_index]`. Encoded by an inline
    /// loop in `write_record` / `write_records` (not by `encode()` since
    /// that takes only `&[u8]` and can't access the payload buffer).
    VlsdOffset { offset: usize, channel_index: usize },
    Skip,
}

impl ChannelEncoder {
    /// Human-readable encoder kind, used in type-mismatch error messages.
    fn kind(&self) -> &'static str {
        match self {
            ChannelEncoder::UInt { .. } => "unsigned integer",
            ChannelEncoder::Int { .. } => "signed integer",
            ChannelEncoder::F32 { .. } => "32-bit float",
            ChannelEncoder::F64 { .. } => "64-bit float",
            ChannelEncoder::Bytes { .. } => "byte array",
            ChannelEncoder::VlsdOffset { .. } => "VLSD",
            ChannelEncoder::Skip => "unsupported (no encoder)",
        }
    }

    fn encode(&self, buf: &mut [u8], value: &DecodedValue) -> Result<(), MdfError> {
        match (self, value) {
            (ChannelEncoder::UInt { offset, bytes }, DecodedValue::UnsignedInteger(v)) => {
                let b = v.to_le_bytes();
                buf[*offset..*offset + *bytes].copy_from_slice(&b[..*bytes]);
                Ok(())
            }
            // Signed values are accepted into unsigned channels only when
            // non-negative; a negative value cannot be represented.
            (ChannelEncoder::UInt { offset, bytes }, DecodedValue::SignedInteger(v)) => {
                if *v < 0 {
                    return Err(MdfError::BlockSerializationError(format!(
                        "cannot encode negative value {} into an unsigned integer channel",
                        v
                    )));
                }
                let b = (*v as u64).to_le_bytes();
                buf[*offset..*offset + *bytes].copy_from_slice(&b[..*bytes]);
                Ok(())
            }
            (ChannelEncoder::Int { offset, bytes }, DecodedValue::SignedInteger(v)) => {
                let b = v.to_le_bytes();
                buf[*offset..*offset + *bytes].copy_from_slice(&b[..*bytes]);
                Ok(())
            }
            // Unsigned values are accepted into signed channels only when they
            // fit in the signed range.
            (ChannelEncoder::Int { offset, bytes }, DecodedValue::UnsignedInteger(v)) => {
                if *v > i64::MAX as u64 {
                    return Err(MdfError::BlockSerializationError(format!(
                        "cannot encode unsigned value {} into a signed integer channel (out of range)",
                        v
                    )));
                }
                let b = (*v as i64).to_le_bytes();
                buf[*offset..*offset + *bytes].copy_from_slice(&b[..*bytes]);
                Ok(())
            }
            (ChannelEncoder::F32 { offset }, DecodedValue::Float(v)) => {
                buf[*offset..*offset + 4].copy_from_slice(&(*v as f32).to_le_bytes());
                Ok(())
            }
            (ChannelEncoder::F64 { offset }, DecodedValue::Float(v)) => {
                buf[*offset..*offset + 8].copy_from_slice(&v.to_le_bytes());
                Ok(())
            }
            (ChannelEncoder::Bytes { offset, bytes }, DecodedValue::ByteArray(data))
            | (ChannelEncoder::Bytes { offset, bytes }, DecodedValue::MimeSample(data))
            | (ChannelEncoder::Bytes { offset, bytes }, DecodedValue::MimeStream(data)) => {
                buf[*offset..*offset + *bytes].fill(0);
                let n = data.len().min(*bytes);
                buf[*offset..*offset + n].copy_from_slice(&data[..n]);
                Ok(())
            }
            // VLSD slots hold a per-record running offset; encoding them from
            // a template is a no-op (the actual payload is handled per record
            // by `encode_record`).
            (ChannelEncoder::VlsdOffset { .. }, _) => Ok(()),
            (enc, val) => Err(MdfError::BlockSerializationError(format!(
                "value {:?} does not match channel encoder type ({})",
                val,
                enc.kind()
            ))),
        }
    }

    fn encode_u64(&self, buf: &mut [u8], value: u64) {
        if let ChannelEncoder::UInt { offset, bytes } = self {
            let b = value.to_le_bytes();
            buf[*offset..*offset + *bytes].copy_from_slice(&b[..*bytes]);
        }
    }

}

const MAX_DT_BLOCK_SIZE: usize = 4 * 1024 * 1024;


fn encode_values(
    encoders: &[ChannelEncoder],
    buf: &mut [u8],
    values: &[DecodedValue],
) -> Result<(), MdfError> {
    for (enc, val) in encoders.iter().zip(values.iter()) {
        enc.encode(buf, val)?;
    }
    Ok(())
}

/// Encode a record, handling VLSD channels by appending payloads to the
/// per-channel buffers in `dt.vlsd_payloads` and writing the running offset
/// into `dt.record_buf`. Non-VLSD channels are encoded in-place via
/// `ChannelEncoder::encode`.
fn encode_record(dt: &mut super::OpenDataBlock, values: &[DecodedValue]) -> Result<(), MdfError> {
    for (i, val) in values.iter().enumerate() {
        match &dt.encoders[i] {
            ChannelEncoder::VlsdOffset { offset, channel_index } => {
                let off = *offset;
                let ch_idx = *channel_index;
                let bytes: &[u8] = match val {
                    DecodedValue::ByteArray(b)
                    | DecodedValue::MimeSample(b)
                    | DecodedValue::MimeStream(b) => b.as_slice(),
                    DecodedValue::String(s) => s.as_bytes(),
                    other => {
                        return Err(MdfError::BlockSerializationError(format!(
                            "value {:?} cannot be written to a VLSD channel (expected String or ByteArray)",
                            other
                        )));
                    }
                };
                let buf = dt.vlsd_payloads[ch_idx]
                    .as_mut()
                    .expect("VLSD encoder requires payload buffer");
                let cur = buf.len() as u64;
                dt.record_buf[off..off + 8].copy_from_slice(&cur.to_le_bytes());
                buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                buf.extend_from_slice(bytes);
            }
            enc => enc.encode(&mut dt.record_buf, val)?,
        }
    }
    Ok(())
}

impl MdfWriter {
    /// Start writing a DTBLOCK for the given data group.
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

        // Validate the channel layout before writing anything to the file so
        // an error leaves the writer state untouched. The encoder-based write
        // path only supports little-endian integer/float types, byte arrays
        // and VLSD channels; everything else used to be silently written as
        // zeros (see FINDINGS A7/A10/D3).
        for (i, ch) in channels.iter().enumerate() {
            let name = ch
                .name
                .clone()
                .unwrap_or_else(|| format!("channel #{}", i));
            if ch.bit_offset != 0 {
                return Err(MdfError::BlockSerializationError(format!(
                    "channel '{}' has bit_offset {} — the writer does not support bit-shifted encoding",
                    name, ch.bit_offset
                )));
            }
            // VLSD channels carry a u64 offset slot; the payload data type is
            // free-form.
            if ch.channel_type == 1 {
                continue;
            }
            match ch.data_type {
                DataType::UnsignedIntegerLE
                | DataType::SignedIntegerLE
                | DataType::ByteArray
                | DataType::MimeSample
                | DataType::MimeStream => {}
                DataType::FloatLE => {
                    if ch.bit_count != 32 && ch.bit_count != 64 {
                        return Err(MdfError::BlockSerializationError(format!(
                            "channel '{}' has FloatLE bit_count {} — only 32 or 64 are supported",
                            name, ch.bit_count
                        )));
                    }
                }
                ref other => {
                    return Err(MdfError::BlockSerializationError(format!(
                        "channel '{}' has data type {:?} which the record encoder cannot write \
                         (fixed-length string, big-endian, CANopen and complex types are unsupported); \
                         use a VLSD channel for strings or start_data_block_for_cg_raw for raw copies",
                        name, other
                    )));
                }
            }
        }

        let mut record_bytes = 0usize;
        for ch in channels {
            let byte_end = ch.byte_offset as usize + ((ch.bit_offset as usize + ch.bit_count as usize + 7) / 8);
            record_bytes = record_bytes.max(byte_end);
        }
        // Records must include the channel group's invalidation bytes
        // (zero-filled), otherwise the declared record stride
        // (samples_byte_nr + invalidation_bytes_nr) would not match the
        // written stride.
        let invalidation_bytes = self.cg_inval_bytes.get(cg_id).copied().unwrap_or(0) as usize;
        let record_size = record_bytes + record_id_len as usize + invalidation_bytes;

        let cg_channel_ids = self.cg_channel_ids.get(cg_id).cloned().unwrap_or_default();

        let header = BlockHeader { id: "##DT".to_string(), reserved0: 0, block_len: 24, links_nr: 0 };
        let header_bytes = header.to_bytes()?;
        let dt_id = format!("dt_{}", self.dt_counter);
        self.dt_counter += 1;
        let dt_pos = self.write_block_with_id(&header_bytes, &dt_id)?;

        let dg_data_link_offset = 40;
        self.update_block_link(dg_id, dg_data_link_offset, &dt_id)?;
        self.update_block_u8(dg_id, 56, record_id_len)?;
        self.update_block_u32(cg_id, 96, record_bytes as u32)?;
        self.update_block_u32(cg_id, 100, invalidation_bytes as u32)?;

        let mut encoders = Vec::new();
        let mut vlsd_payloads: Vec<Option<Vec<u8>>> = Vec::with_capacity(channels.len());
        let mut vlsd_channel_ids: Vec<Option<String>> = Vec::with_capacity(channels.len());
        for (i, ch) in channels.iter().enumerate() {
            let offset = record_id_len as usize + ch.byte_offset as usize;
            let bytes = ((ch.bit_count + 7) / 8) as usize;
            // A channel_type of 1 alone marks a VLSD channel; the legacy
            // `data != 0` sentinel is no longer required (but still implies
            // channel_type == 1 in files produced by this writer).
            let is_vlsd = ch.channel_type == 1;
            let enc = if is_vlsd {
                ChannelEncoder::VlsdOffset { offset, channel_index: i }
            } else {
                match ch.data_type {
                    DataType::UnsignedIntegerLE => ChannelEncoder::UInt { offset, bytes },
                    DataType::SignedIntegerLE => ChannelEncoder::Int { offset, bytes },
                    DataType::FloatLE => {
                        if ch.bit_count == 32 {
                            ChannelEncoder::F32 { offset }
                        } else {
                            ChannelEncoder::F64 { offset }
                        }
                    }
                    DataType::ByteArray | DataType::MimeSample | DataType::MimeStream => {
                        ChannelEncoder::Bytes { offset, bytes }
                    }
                    _ => ChannelEncoder::Skip,
                }
            };
            encoders.push(enc);
            if is_vlsd {
                vlsd_payloads.push(Some(Vec::new()));
                vlsd_channel_ids.push(cg_channel_ids.get(i).cloned());
            } else {
                vlsd_payloads.push(None);
                vlsd_channel_ids.push(None);
            }
        }

        self.open_dts.insert(
            cg_id.to_string(),
            OpenDataBlock {
                dg_id: dg_id.to_string(),
                dt_id: dt_id.clone(),
                start_pos: dt_pos,
                record_size,
                record_count: 0,
                total_record_count: 0,
                channels: channels.to_vec(),
                dt_ids: vec![dt_id],
                dt_positions: vec![dt_pos],
                dt_sizes: Vec::new(),
                record_buf: vec![0u8; record_size],
                record_template: vec![0u8; record_size],
                encoders,
                vlsd_payloads,
                vlsd_channel_ids,
            },
        );
        Ok(())
    }

    /// Convenience wrapper to start a data block for a channel group without specifying its data group explicitly.
    pub fn start_data_block_for_cg(
        &mut self,
        cg_id: &str,
        record_id_len: u8,
    ) -> Result<(), MdfError> {
        let dg = self.cg_to_dg.get(cg_id).ok_or_else(|| MdfError::BlockSerializationError("unknown channel group".into()))?.clone();
        let channels = self.cg_channels.get(cg_id).ok_or_else(|| MdfError::BlockSerializationError("no channels for channel group".into()))?.clone();
        self.start_data_block(&dg, cg_id, record_id_len, &channels)
    }

    /// Open a DT block for raw byte-level record writing.
    ///
    /// Unlike [`start_data_block_for_cg`], this does NOT derive `record_size`
    /// from the channel layout. Instead the caller supplies the source file's
    /// `data_bytes` (= `samples_byte_nr` of the source channel group) and
    /// `invalidation_bytes` (= `invalidation_bytes_nr`). Both fields are
    /// patched onto the new channel group block, and [`write_raw_record`]
    /// expects byte slices of length `record_id_len + data_bytes + invalidation_bytes`.
    ///
    /// This entry point exists for the cut/merge code paths which copy raw
    /// records (including invalidation bits and any unencoded layout
    /// trailers) verbatim from a source file.
    pub fn start_data_block_for_cg_raw(
        &mut self,
        cg_id: &str,
        record_id_len: u8,
        data_bytes: u32,
        invalidation_bytes: u32,
    ) -> Result<(), MdfError> {
        if self.open_dts.contains_key(cg_id) {
            return Err(MdfError::BlockSerializationError(
                "data block already open for this channel group".into(),
            ));
        }
        let dg_id = self
            .cg_to_dg
            .get(cg_id)
            .ok_or_else(|| MdfError::BlockSerializationError("unknown channel group".into()))?
            .clone();
        let channels = self
            .cg_channels
            .get(cg_id)
            .ok_or_else(|| MdfError::BlockSerializationError("no channels for channel group".into()))?
            .clone();

        let record_size =
            record_id_len as usize + data_bytes as usize + invalidation_bytes as usize;

        let header = BlockHeader { id: "##DT".to_string(), reserved0: 0, block_len: 24, links_nr: 0 };
        let header_bytes = header.to_bytes()?;
        let dt_id = format!("dt_{}", self.dt_counter);
        self.dt_counter += 1;
        let dt_pos = self.write_block_with_id(&header_bytes, &dt_id)?;

        let dg_data_link_offset = 40;
        self.update_block_link(&dg_id, dg_data_link_offset, &dt_id)?;
        self.update_block_u8(&dg_id, 56, record_id_len)?;
        // Patch CG.samples_byte_nr (offset 96) and CG.invalidation_bytes_nr (offset 100).
        self.update_block_u32(cg_id, 96, data_bytes)?;
        self.update_block_u32(cg_id, 100, invalidation_bytes)?;

        // Encoders are unused on the raw-write path but the OpenDataBlock type
        // requires the field; populate with `Skip` placeholders for parity.
        let channel_count = channels.len();
        let encoders: Vec<ChannelEncoder> =
            channels.iter().map(|_| ChannelEncoder::Skip).collect();

        self.open_dts.insert(
            cg_id.to_string(),
            OpenDataBlock {
                dg_id,
                dt_id: dt_id.clone(),
                start_pos: dt_pos,
                record_size,
                record_count: 0,
                total_record_count: 0,
                channels,
                dt_ids: vec![dt_id],
                dt_positions: vec![dt_pos],
                dt_sizes: Vec::new(),
                record_buf: vec![0u8; record_size],
                record_template: vec![0u8; record_size],
                encoders,
                vlsd_payloads: vec![None; channel_count],
                vlsd_channel_ids: vec![None; channel_count],
            },
        );
        Ok(())
    }

    /// Precomputes constant values for a channel group. The provided slice must
    /// have the same length as the channel list and will be encoded into the
    /// internal record template used for each record.
    pub fn set_record_template(
        &mut self,
        cg_id: &str,
        values: &[DecodedValue],
    ) -> Result<(), MdfError> {
        let dt = self.open_dts.get_mut(cg_id).ok_or_else(|| {
            MdfError::BlockSerializationError("no open DT block for this channel group".into())
        })?;
        if values.len() != dt.channels.len() {
            return Err(MdfError::BlockSerializationError("value count mismatch".into()));
        }
        dt.record_template.fill(0);
        encode_values(&dt.encoders, &mut dt.record_template, values)?;
        Ok(())
    }

    /// Append one record to the currently open DTBLOCK for the given channel group.
    pub fn write_record(&mut self, cg_id: &str, values: &[DecodedValue]) -> Result<(), MdfError> {
        let potential_new_block = {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| MdfError::BlockSerializationError("no open DT block for this channel group".into()))?;
            if values.len() != dt.channels.len() {
                return Err(MdfError::BlockSerializationError("value count mismatch".into()));
            }
            dt.record_count > 0
                && 24 + dt.record_size * (dt.record_count as usize + 1) > MAX_DT_BLOCK_SIZE
        };

        if potential_new_block {
            let mut empty = Vec::new();
            self.split_dt_block(cg_id, &mut empty)?;
        }

        let dt = self.open_dts.get_mut(cg_id).unwrap();
        dt.record_buf.copy_from_slice(&dt.record_template);
        encode_record(dt, values)?;

        self.file.write_all(&dt.record_buf)?;
        dt.record_count += 1;
        self.offset += dt.record_buf.len() as u64;
        Ok(())
    }

    /// Append one record to the open DTBLOCK as a verbatim byte copy.
    ///
    /// Unlike [`write_record`], this bypasses per-channel encoders and writes
    /// the supplied bytes directly. The slice length must equal the channel
    /// group's `record_size` (record_id + data + invalidation bytes). This is
    /// used by the cut/merge code to preserve raw record content (including
    /// invalidation bits and any unencoded channel layouts) without going
    /// through the decode/re-encode round trip.
    pub fn write_raw_record(&mut self, cg_id: &str, raw: &[u8]) -> Result<(), MdfError> {
        let potential_new_block = {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                MdfError::BlockSerializationError(
                    "no open DT block for this channel group".into(),
                )
            })?;
            if raw.len() != dt.record_size {
                return Err(MdfError::BlockSerializationError(
                    "raw record size mismatch".into(),
                ));
            }
            dt.record_count > 0
                && 24 + dt.record_size * (dt.record_count as usize + 1) > MAX_DT_BLOCK_SIZE
        };

        if potential_new_block {
            let mut empty = Vec::new();
            self.split_dt_block(cg_id, &mut empty)?;
        }

        self.file.write_all(raw)?;
        let dt = self.open_dts.get_mut(cg_id).unwrap();
        dt.record_count += 1;
        self.offset += raw.len() as u64;
        Ok(())
    }

    /// Fast path for uniform unsigned integer channel groups.
    pub fn write_record_u64(&mut self, cg_id: &str, values: &[u64]) -> Result<(), MdfError> {
        let potential_new_block = {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                MdfError::BlockSerializationError("no open DT block for this channel group".into())
            })?;
            if values.len() != dt.encoders.len() {
                return Err(MdfError::BlockSerializationError("value count mismatch".into()));
            }
            if !dt.encoders.iter().all(|e| matches!(e, ChannelEncoder::UInt { .. })) {
                return Err(MdfError::BlockSerializationError("channel types not unsigned".into()));
            }
            dt.record_count > 0
                && 24 + dt.record_size * (dt.record_count as usize + 1) > MAX_DT_BLOCK_SIZE
        };

        if potential_new_block {
            let mut empty = Vec::new();
            self.split_dt_block(cg_id, &mut empty)?;
        }

        let dt = self.open_dts.get_mut(cg_id).unwrap();
        dt.record_buf.copy_from_slice(&dt.record_template);
        for (enc, &v) in dt.encoders.iter().zip(values.iter()) {
            enc.encode_u64(&mut dt.record_buf, v);
        }
        self.file.write_all(&dt.record_buf)?;
        dt.record_count += 1;
        self.offset += dt.record_buf.len() as u64;
        Ok(())
    }

    /// Append multiple records sequentially for the specified channel group.
    /// The provided iterator yields record value slices. All encoded bytes are
    /// buffered and written in a single call to reduce I/O overhead.
    pub fn write_records<'a, I>(&mut self, cg_id: &str, records: I) -> Result<(), MdfError>
    where
        I: IntoIterator<Item = &'a [DecodedValue]>,
    {
        let record_size = {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                MdfError::BlockSerializationError("no open DT block for this channel group".into())
            })?.record_size;
            dt
        };
        let max_records = (MAX_DT_BLOCK_SIZE - 24) / record_size;
        let mut buffer = Vec::with_capacity(record_size * max_records);
        for record in records {
            let potential_new_block = {
                let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                    MdfError::BlockSerializationError("no open DT block for this channel group".into())
                })?;
                if record.len() != dt.channels.len() {
                    return Err(MdfError::BlockSerializationError("value count mismatch".into()));
                }
                dt.record_count > 0
                    && 24 + dt.record_size * (dt.record_count as usize + 1) > MAX_DT_BLOCK_SIZE
            };

            if potential_new_block {
                self.split_dt_block(cg_id, &mut buffer)?;
            }

            let dt = self.open_dts.get_mut(cg_id).unwrap();
            dt.record_buf.copy_from_slice(&dt.record_template);
            encode_record(dt, record)?;
            buffer.extend_from_slice(&dt.record_buf);
            dt.record_count += 1;
        }

        if !buffer.is_empty() {
            self.file.write_all(&buffer)?;
            self.offset += buffer.len() as u64;
        }
        Ok(())
    }

    /// Batch write for uniform unsigned integer channel groups.
    pub fn write_records_u64<'a, I>(&mut self, cg_id: &str, records: I) -> Result<(), MdfError>
    where
        I: IntoIterator<Item = &'a [u64]>,
    {
        let record_size = {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                MdfError::BlockSerializationError("no open DT block for this channel group".into())
            })?.record_size;
            dt
        };
        // Check ONCE that all encoders are unsigned integer type
        {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                MdfError::BlockSerializationError("no open DT block for this channel group".into())
            })?;
            if !dt.encoders.iter().all(|e| matches!(e, ChannelEncoder::UInt { .. })) {
                return Err(MdfError::BlockSerializationError("channel types not unsigned".into()));
            }
        }
        let max_records = (MAX_DT_BLOCK_SIZE - 24) / record_size;
        let mut buffer = Vec::with_capacity(record_size * max_records);
        for rec in records {
            let potential_new_block = {
                let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                    MdfError::BlockSerializationError("no open DT block for this channel group".into())
                })?;
                if rec.len() != dt.encoders.len() {
                    return Err(MdfError::BlockSerializationError("value count mismatch".into()));
                }
                dt.record_count > 0
                    && 24 + dt.record_size * (dt.record_count as usize + 1) > MAX_DT_BLOCK_SIZE
            };

            if potential_new_block {
                self.split_dt_block(cg_id, &mut buffer)?;
            }

            let dt = self.open_dts.get_mut(cg_id).unwrap();
            dt.record_buf.copy_from_slice(&dt.record_template);
            for (enc, &v) in dt.encoders.iter().zip(rec.iter()) {
                enc.encode_u64(&mut dt.record_buf, v);
            }
            buffer.extend_from_slice(&dt.record_buf);
            dt.record_count += 1;
        }

        if !buffer.is_empty() {
            self.file.write_all(&buffer)?;
            self.offset += buffer.len() as u64;
        }
        Ok(())
    }

    /// Helper: finalize the current DT block fragment, update its size, and start a new one.
    /// Called internally when a DT block would exceed MAX_DT_BLOCK_SIZE.
    fn split_dt_block(&mut self, cg_id: &str, buffer: &mut Vec<u8>) -> Result<(), MdfError> {
        // Flush pending bytes first
        if !buffer.is_empty() {
            self.file.write_all(buffer)?;
            self.offset += buffer.len() as u64;
            buffer.clear();
        }
        let (start_pos, record_count, record_size) = {
            let dt = self.open_dts.get(cg_id).unwrap();
            (dt.start_pos, dt.record_count, dt.record_size)
        };
        let size = 24 + record_size * record_count as usize;
        self.update_link(start_pos + 8, size as u64)?;
        {
            let dt = self.open_dts.get_mut(cg_id).unwrap();
            dt.total_record_count += record_count;
            dt.dt_sizes.push(size as u64);
        }
        let header = BlockHeader { id: "##DT".to_string(), reserved0: 0, block_len: 24, links_nr: 0 };
        let header_bytes = header.to_bytes()?;
        let new_dt_id = format!("dt_{}", self.dt_counter);
        self.dt_counter += 1;
        let new_dt_pos = self.write_block_with_id(&header_bytes, &new_dt_id)?;

        let dt = self.open_dts.get_mut(cg_id).unwrap();
        dt.dt_id = new_dt_id.clone();
        dt.start_pos = new_dt_pos;
        dt.record_count = 0;
        dt.dt_ids.push(new_dt_id);
        dt.dt_positions.push(new_dt_pos);
        Ok(())
    }

    /// Batch write for uniform f64/f32 channel groups.
    ///
    /// Each item yielded by `records` is a slice of `f64` values — one per
    /// channel — for a single record. This avoids the `DecodedValue`
    /// allocation overhead of `write_records` while still supporting mixed
    /// 32-/64-bit float groups: channels whose encoder is `F32` will have
    /// their value narrowed to `f32` automatically.
    ///
    /// Returns an error if any encoder is not a float type.
    pub fn write_records_f64<'a, I>(&mut self, cg_id: &str, records: I) -> Result<(), MdfError>
    where
        I: IntoIterator<Item = &'a [f64]>,
    {
        let record_size = {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                MdfError::BlockSerializationError("no open DT block for this channel group".into())
            })?.record_size;
            dt
        };
        // Check ONCE that all encoders are float types (F32 or F64).
        {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                MdfError::BlockSerializationError("no open DT block for this channel group".into())
            })?;
            if !dt.encoders.iter().all(|e| matches!(e, ChannelEncoder::F32 { .. } | ChannelEncoder::F64 { .. })) {
                return Err(MdfError::BlockSerializationError("channel types not float".into()));
            }
        }
        let max_records = (MAX_DT_BLOCK_SIZE - 24) / record_size;
        let mut buffer = Vec::with_capacity(record_size * max_records);
        for rec in records {
            let potential_new_block = {
                let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                    MdfError::BlockSerializationError("no open DT block for this channel group".into())
                })?;
                if rec.len() != dt.encoders.len() {
                    return Err(MdfError::BlockSerializationError("value count mismatch".into()));
                }
                dt.record_count > 0
                    && 24 + dt.record_size * (dt.record_count as usize + 1) > MAX_DT_BLOCK_SIZE
            };

            if potential_new_block {
                self.split_dt_block(cg_id, &mut buffer)?;
            }

            let dt = self.open_dts.get_mut(cg_id).unwrap();
            dt.record_buf.copy_from_slice(&dt.record_template);
            for (enc, &v) in dt.encoders.iter().zip(rec.iter()) {
                match enc {
                    ChannelEncoder::F64 { offset } => {
                        dt.record_buf[*offset..*offset + 8].copy_from_slice(&v.to_le_bytes());
                    }
                    ChannelEncoder::F32 { offset } => {
                        dt.record_buf[*offset..*offset + 4].copy_from_slice(&(v as f32).to_le_bytes());
                    }
                    _ => {}
                }
            }
            buffer.extend_from_slice(&dt.record_buf);
            dt.record_count += 1;
        }

        if !buffer.is_empty() {
            self.file.write_all(&buffer)?;
            self.offset += buffer.len() as u64;
        }
        Ok(())
    }

    /// Columnar write for uniform f64 channel groups.
    ///
    /// `columns` holds one `&[f64]` slice per channel; all slices must have
    /// the same length (= number of records to write). All encoders must be
    /// `F64`. Values are written channel-by-column into a pre-allocated
    /// record buffer, then flushed to disk in one `write_all` per DT chunk.
    /// This eliminates per-record overhead and is the fastest available write
    /// path for f64-only groups.
    pub fn write_columns_f64(&mut self, cg_id: &str, columns: &[&[f64]]) -> Result<(), MdfError> {
        // Validate inputs and extract metadata once.
        let (offsets, record_size, nrows, need_template, template) = {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                MdfError::BlockSerializationError("no open DT block for this channel group".into())
            })?;
            if columns.len() != dt.encoders.len() {
                return Err(MdfError::BlockSerializationError("column count does not match encoder count".into()));
            }
            if !dt.encoders.iter().all(|e| matches!(e, ChannelEncoder::F64 { .. })) {
                return Err(MdfError::BlockSerializationError("channel types not f64".into()));
            }
            let nrows = columns.first().map(|c| c.len()).unwrap_or(0);
            if columns.iter().any(|c| c.len() != nrows) {
                return Err(MdfError::BlockSerializationError("column length mismatch".into()));
            }
            let offsets: Vec<usize> = dt.encoders.iter().map(|e| match e {
                ChannelEncoder::F64 { offset } => *offset,
                _ => 0,
            }).collect();
            // Skip template stamping when all record bytes are covered by f64 channels.
            let need_template = columns.len() * 8 < dt.record_size;
            let template = dt.record_template.clone();
            (offsets, dt.record_size, nrows, need_template, template)
        };

        if nrows == 0 {
            return Ok(());
        }

        // Fragments always hold at least one record so a record larger than
        // MAX_DT_BLOCK_SIZE cannot degenerate into an infinite split loop or
        // an empty first fragment.
        let max_per_dt = ((MAX_DT_BLOCK_SIZE - 24) / record_size).max(1);

        // Pre-allocate the write buffer once at maximum chunk size.
        let mut buf = vec![0u8; max_per_dt * record_size];

        let mut row = 0usize;
        while row < nrows {
            let records_in_current = {
                let dt = &self.open_dts[cg_id];
                let capacity = ((MAX_DT_BLOCK_SIZE - 24) / dt.record_size).max(1);
                capacity.saturating_sub(dt.record_count as usize)
            };
            let chunk_size = (nrows - row).min(records_in_current).min(max_per_dt);
            if chunk_size == 0 {
                let mut empty = Vec::new();
                self.split_dt_block(cg_id, &mut empty)?;
                continue;
            }

            let buf_len = chunk_size * record_size;

            // Stamp template if needed, then write columns at their offsets.
            // Byte-level copies avoid the alignment UB of casting the byte
            // buffer to `&mut [f64]`; `copy_from_slice` of 8 bytes compiles
            // to a single unaligned store.
            if need_template {
                for r in 0..chunk_size {
                    buf[r * record_size..(r + 1) * record_size].copy_from_slice(&template);
                }
            }
            for (col_idx, col) in columns.iter().enumerate() {
                let off = offsets[col_idx];
                for r in 0..chunk_size {
                    let base = r * record_size + off;
                    buf[base..base + 8].copy_from_slice(&col[row + r].to_le_bytes());
                }
            }

            self.file.write_all(&buf[..buf_len])?;
            self.offset += buf_len as u64;
            {
                let dt = self.open_dts.get_mut(cg_id).unwrap();
                dt.record_count += chunk_size as u64;
            }
            row += chunk_size;
        }
        Ok(())
    }

    /// Columnar write for mixed-type channel groups.
    ///
    /// `columns` holds one [`ColumnData`] per channel. All columns must have
    /// the same length. Each `ColumnData` variant must match the corresponding
    /// channel's encoder type. Values are written column-by-column into a
    /// pre-allocated record buffer and flushed in large chunks, avoiding
    /// per-record dispatch overhead.
    pub fn write_columns(&mut self, cg_id: &str, columns: &[ColumnData<'_>]) -> Result<(), MdfError> {
        // Validate and extract metadata once.
        let (nrows, enc_info, record_size, need_template, template) = {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| {
                MdfError::BlockSerializationError("no open DT block for this channel group".into())
            })?;
            if columns.len() != dt.encoders.len() {
                return Err(MdfError::BlockSerializationError("column count does not match encoder count".into()));
            }
            let nrows = match columns.first() {
                Some(ColumnData::F64(s)) => s.len(),
                Some(ColumnData::F32(s)) => s.len(),
                Some(ColumnData::U64(s)) => s.len(),
                Some(ColumnData::I64(s)) => s.len(),
                None => 0,
            };
            let mut total_channel_bytes = 0usize;
            for (col, enc) in columns.iter().zip(dt.encoders.iter()) {
                let col_len = match col {
                    ColumnData::F64(s) => s.len(),
                    ColumnData::F32(s) => s.len(),
                    ColumnData::U64(s) => s.len(),
                    ColumnData::I64(s) => s.len(),
                };
                if col_len != nrows {
                    return Err(MdfError::BlockSerializationError("column length mismatch".into()));
                }
                let type_ok = match (col, enc) {
                    (ColumnData::F64(_), ChannelEncoder::F64 { .. }) => true,
                    (ColumnData::F32(_), ChannelEncoder::F32 { .. }) => true,
                    (ColumnData::U64(_), ChannelEncoder::UInt { .. }) => true,
                    (ColumnData::I64(_), ChannelEncoder::Int { .. }) => true,
                    _ => false,
                };
                if !type_ok {
                    return Err(MdfError::BlockSerializationError("column type does not match encoder type".into()));
                }
            }
            let enc_info: Vec<(usize, usize)> = dt.encoders.iter().map(|e| match e {
                ChannelEncoder::F64 { offset } => (*offset, 8usize),
                ChannelEncoder::F32 { offset } => (*offset, 4usize),
                ChannelEncoder::UInt { offset, bytes } => (*offset, *bytes),
                ChannelEncoder::Int { offset, bytes } => (*offset, *bytes),
                ChannelEncoder::Bytes { offset, bytes } => (*offset, *bytes),
                ChannelEncoder::VlsdOffset { .. } | ChannelEncoder::Skip => (0, 0),
            }).collect();
            for &(_, nbytes) in &enc_info {
                total_channel_bytes += nbytes;
            }
            let need_template = total_channel_bytes < dt.record_size;
            let template = dt.record_template.clone();
            (nrows, enc_info, dt.record_size, need_template, template)
        };

        if nrows == 0 {
            return Ok(());
        }

        let max_per_dt = ((MAX_DT_BLOCK_SIZE - 24) / record_size).max(1);
        let mut buf = vec![0u8; max_per_dt * record_size];

        let mut row = 0usize;
        while row < nrows {
            let records_in_current = {
                let dt = &self.open_dts[cg_id];
                let capacity = ((MAX_DT_BLOCK_SIZE - 24) / dt.record_size).max(1);
                capacity.saturating_sub(dt.record_count as usize)
            };
            let chunk_size = (nrows - row).min(records_in_current).min(max_per_dt);
            if chunk_size == 0 {
                let mut empty = Vec::new();
                self.split_dt_block(cg_id, &mut empty)?;
                continue;
            }

            let buf_len = chunk_size * record_size;
            if need_template {
                for r in 0..chunk_size {
                    buf[r * record_size..(r + 1) * record_size].copy_from_slice(&template);
                }
            }

            for (col_idx, col) in columns.iter().enumerate() {
                let (off, nbytes) = enc_info[col_idx];
                if nbytes == 0 {
                    continue;
                }
                match col {
                    ColumnData::F64(vals) => {
                        for r in 0..chunk_size {
                            let base = r * record_size + off;
                            buf[base..base + 8].copy_from_slice(&vals[row + r].to_le_bytes());
                        }
                    }
                    ColumnData::F32(vals) => {
                        for r in 0..chunk_size {
                            let base = r * record_size + off;
                            buf[base..base + 4].copy_from_slice(&vals[row + r].to_le_bytes());
                        }
                    }
                    ColumnData::U64(vals) => {
                        for r in 0..chunk_size {
                            let base = r * record_size + off;
                            let b = vals[row + r].to_le_bytes();
                            buf[base..base + nbytes].copy_from_slice(&b[..nbytes]);
                        }
                    }
                    ColumnData::I64(vals) => {
                        for r in 0..chunk_size {
                            let base = r * record_size + off;
                            let b = vals[row + r].to_le_bytes();
                            buf[base..base + nbytes].copy_from_slice(&b[..nbytes]);
                        }
                    }
                }
            }

            self.file.write_all(&buf[..buf_len])?;
            self.offset += buf_len as u64;
            {
                let dt = self.open_dts.get_mut(cg_id).unwrap();
                dt.record_count += chunk_size as u64;
            }
            row += chunk_size;
        }
        Ok(())
    }

    /// Finalize the currently open DTBLOCK for a given channel group and patch its size field.
    pub fn finish_data_block(&mut self, cg_id: &str) -> Result<(), MdfError> {
        let mut dt = self.open_dts.remove(cg_id).ok_or_else(|| MdfError::BlockSerializationError("no open DT block for this channel group".into()))?;
        let size = 24 + dt.record_size as u64 * dt.record_count;
        self.update_link(dt.start_pos + 8, size)?;
        dt.dt_sizes.push(size);
        dt.total_record_count += dt.record_count;
        self.update_block_u64(cg_id, 80, dt.total_record_count)?;

        if dt.dt_ids.len() > 1 {
            let dl_count = self.block_positions.keys().filter(|k| k.starts_with("dl_")).count();
            let dl_id = format!("dl_{}", dl_count);
            // Per MDF 4.1, dl_equal_length is the length of each fragment's
            // DATA SECTION, i.e. the block length minus the 24-byte header.
            let common_len = dt.dt_sizes.first().copied().unwrap_or(size).saturating_sub(24);
            let dl_block = DataListBlock::new_equal(dt.dt_positions.clone(), common_len);
            let dl_bytes = dl_block.to_bytes()?;
            let _pos = self.write_block_with_id(&dl_bytes, &dl_id)?;
            let dg_data_link_offset = 40;
            self.update_block_link(&dt.dg_id, dg_data_link_offset, &dl_id)?;
        }

        for i in 0..dt.vlsd_payloads.len() {
            let payload = match dt.vlsd_payloads[i].take() {
                Some(p) => p,
                None => continue,
            };
            let cn_id = match dt.vlsd_channel_ids[i].clone() {
                Some(id) => id,
                None => continue,
            };
            let block_len = 24u64 + payload.len() as u64;
            let header = BlockHeader { id: "##SD".to_string(), reserved0: 0, block_len, links_nr: 0 };
            let mut sd_bytes = header.to_bytes()?;
            sd_bytes.extend_from_slice(&payload);

            let sd_count = self.block_positions.keys().filter(|k| k.starts_with("sd_")).count();
            let sd_id = format!("sd_{}", sd_count);
            self.write_block_with_id(&sd_bytes, &sd_id)?;
            let cn_data_offset = 64u64;
            self.update_block_link(&cn_id, cn_data_offset, &sd_id)?;
        }
        Ok(())
    }
}
