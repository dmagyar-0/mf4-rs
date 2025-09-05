// Handling of DT blocks and record writing
use super::*;
use std::io::Write;
use crate::blocks::common::{BlockHeader, DataType};
use crate::blocks::data_list_block::DataListBlock;
use crate::parsing::decoder::DecodedValue;

pub(super) enum ChannelEncoder {
    UInt { offset: usize, bytes: usize },
    Int { offset: usize, bytes: usize },
    F32 { offset: usize },
    F64 { offset: usize },
    Bytes { offset: usize, bytes: usize },
    Skip,
}

impl ChannelEncoder {
    fn encode(&self, buf: &mut [u8], value: &DecodedValue) {
        match (self, value) {
            (ChannelEncoder::UInt { offset, bytes }, DecodedValue::UnsignedInteger(v)) => {
                let b = v.to_le_bytes();
                buf[*offset..*offset + *bytes].copy_from_slice(&b[..*bytes]);
            }
            (ChannelEncoder::Int { offset, bytes }, DecodedValue::SignedInteger(v)) => {
                let b = (*v as i64).to_le_bytes();
                buf[*offset..*offset + *bytes].copy_from_slice(&b[..*bytes]);
            }
            (ChannelEncoder::F32 { offset }, DecodedValue::Float(v)) => {
                buf[*offset..*offset + 4].copy_from_slice(&(*v as f32).to_le_bytes());
            }
            (ChannelEncoder::F64 { offset }, DecodedValue::Float(v)) => {
                buf[*offset..*offset + 8].copy_from_slice(&v.to_le_bytes());
            }
            (ChannelEncoder::Bytes { offset, bytes }, DecodedValue::ByteArray(data))
            | (ChannelEncoder::Bytes { offset, bytes }, DecodedValue::MimeSample(data))
            | (ChannelEncoder::Bytes { offset, bytes }, DecodedValue::MimeStream(data)) => {
                buf[*offset..*offset + *bytes].fill(0);
                let n = data.len().min(*bytes);
                buf[*offset..*offset + n].copy_from_slice(&data[..n]);
            }
            _ => {}
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


fn encode_values(encoders: &[ChannelEncoder], buf: &mut [u8], values: &[DecodedValue]) {
    for (enc, val) in encoders.iter().zip(values.iter()) {
        enc.encode(buf, val);
    }
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

        let mut record_bytes = 0usize;
        for ch in channels {
            let byte_end = ch.byte_offset as usize + ((ch.bit_offset as usize + ch.bit_count as usize + 7) / 8);
            record_bytes = record_bytes.max(byte_end);
        }
        let record_size = record_bytes + record_id_len as usize;

        let header = BlockHeader { id: "##DT".to_string(), reserved0: 0, block_len: 24, links_nr: 0 };
        let header_bytes = header.to_bytes()?;
        let dt_id = format!("dt_{}", self.dt_counter);
        self.dt_counter += 1;
        let dt_pos = self.write_block_with_id(&header_bytes, &dt_id)?;

        let dg_data_link_offset = 40;
        self.update_block_link(dg_id, dg_data_link_offset, &dt_id)?;
        self.update_block_u8(dg_id, 56, record_id_len)?;
        self.update_block_u32(cg_id, 96, record_bytes as u32)?;

        let mut encoders = Vec::new();
        for ch in channels {
            let offset = record_id_len as usize + ch.byte_offset as usize;
            let bytes = ((ch.bit_count + 7) / 8) as usize;
            let enc = match ch.data_type {
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
            };
            encoders.push(enc);
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
        encode_values(&dt.encoders, &mut dt.record_template, values);
        Ok(())
    }

    /// Append one record to the currently open DTBLOCK for the given channel group.
    pub fn write_record(&mut self, cg_id: &str, values: &[DecodedValue]) -> Result<(), MdfError> {
        let potential_new_block = {
            let dt = self.open_dts.get(cg_id).ok_or_else(|| MdfError::BlockSerializationError("no open DT block for this channel group".into()))?;
            if values.len() != dt.channels.len() {
                return Err(MdfError::BlockSerializationError("value count mismatch".into()));
            }
            24 + dt.record_size * (dt.record_count as usize + 1) > MAX_DT_BLOCK_SIZE
        };

        if potential_new_block {
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
        }

        let dt = self.open_dts.get_mut(cg_id).unwrap();
        if values.len() != dt.channels.len() {
            return Err(MdfError::BlockSerializationError("value count mismatch".into()));
        }

        dt.record_buf.copy_from_slice(&dt.record_template);
        encode_values(&dt.encoders, &mut dt.record_buf, values);

        self.file.write_all(&dt.record_buf)?;
        dt.record_count += 1;
        self.offset += dt.record_buf.len() as u64;
        Ok(())
    }

    /// Fast path for uniform unsigned integer channel groups.
    pub fn write_record_u64(&mut self, cg_id: &str, values: &[u64]) -> Result<(), MdfError> {
        let dt = self.open_dts.get_mut(cg_id).ok_or_else(|| {
            MdfError::BlockSerializationError("no open DT block for this channel group".into())
        })?;
        if values.len() != dt.encoders.len() {
            return Err(MdfError::BlockSerializationError("value count mismatch".into()));
        }
        if !dt.encoders.iter().all(|e| matches!(e, ChannelEncoder::UInt { .. })) {
            return Err(MdfError::BlockSerializationError("channel types not unsigned".into()));
        }
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
                24 + dt.record_size * (dt.record_count as usize + 1) > MAX_DT_BLOCK_SIZE
            };

            if potential_new_block {
                self.file.write_all(&buffer)?;
                self.offset += buffer.len() as u64;
                buffer.clear();

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
            }

            let dt = self.open_dts.get_mut(cg_id).unwrap();
            dt.record_buf.copy_from_slice(&dt.record_template);
            encode_values(&dt.encoders, &mut dt.record_buf, record);
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
                if !dt.encoders.iter().all(|e| matches!(e, ChannelEncoder::UInt { .. })) {
                    return Err(MdfError::BlockSerializationError("channel types not unsigned".into()));
                }
                24 + dt.record_size * (dt.record_count as usize + 1) > MAX_DT_BLOCK_SIZE
            };

            if potential_new_block {
                self.file.write_all(&buffer)?;
                self.offset += buffer.len() as u64;
                buffer.clear();

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
            let common_len = *dt.dt_sizes.first().unwrap_or(&size);
            let dl_block = DataListBlock::new_equal(dt.dt_positions.clone(), common_len);
            let dl_bytes = dl_block.to_bytes()?;
            let _pos = self.write_block_with_id(&dl_bytes, &dl_id)?;
            let dg_data_link_offset = 40;
            self.update_block_link(&dt.dg_id, dg_data_link_offset, &dl_id)?;
        }
        Ok(())
    }
}
