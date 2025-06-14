// Handling of DT blocks and record writing
use super::*;
use std::io::Write;
use crate::blocks::common::{BlockHeader, DataType};
use crate::blocks::data_list_block::DataListBlock;
use crate::parsing::decoder::DecodedValue;

const MAX_DT_BLOCK_SIZE: usize = 4 * 1024 * 1024;

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

        self.open_dts.insert(
            cg_id.to_string(),
            OpenDataBlock {
                dg_id: dg_id.to_string(),
                dt_id: dt_id.clone(),
                start_pos: dt_pos,
                record_size,
                record_count: 0,
                total_record_count: 0,
                record_id_len: record_id_len as usize,
                channels: channels.to_vec(),
                dt_ids: vec![dt_id],
                dt_positions: vec![dt_pos],
                dt_sizes: Vec::new(),
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

        let mut buf = vec![0u8; dt.record_size];
        for (ch, val) in dt.channels.iter().zip(values.iter()) {
            let offset = dt.record_id_len + ch.byte_offset as usize;
            match (&ch.data_type, val) {
                (DataType::UnsignedIntegerLE, DecodedValue::UnsignedInteger(v)) => {
                    let bytes = (*v).to_le_bytes();
                    let n = ((ch.bit_count + 7) / 8) as usize;
                    buf[offset..offset + n].copy_from_slice(&bytes[..n]);
                }
                (DataType::SignedIntegerLE, DecodedValue::SignedInteger(v)) => {
                    let bytes = (*v as i64).to_le_bytes();
                    let n = ((ch.bit_count + 7) / 8) as usize;
                    buf[offset..offset + n].copy_from_slice(&bytes[..n]);
                }
                (DataType::FloatLE, DecodedValue::Float(v)) => {
                    if ch.bit_count == 32 {
                        buf[offset..offset + 4].copy_from_slice(&(*v as f32).to_le_bytes());
                    } else if ch.bit_count == 64 {
                        buf[offset..offset + 8].copy_from_slice(&v.to_le_bytes());
                    }
                }
                (DataType::ByteArray, DecodedValue::ByteArray(bytes))
                | (DataType::MimeSample, DecodedValue::MimeSample(bytes))
                | (DataType::MimeStream, DecodedValue::MimeStream(bytes)) => {
                    let n = ((ch.bit_count + 7) / 8) as usize;
                    for (i, b) in bytes.iter().take(n).enumerate() {
                        buf[offset + i] = *b;
                    }
                }
                _ => {}
            }
        }

        self.file.write_all(&buf)?;
        dt.record_count += 1;
        self.offset += buf.len() as u64;
        Ok(())
    }

    /// Append multiple records sequentially for the specified channel group.
    pub fn write_records<'a, I>(&mut self, cg_id: &str, records: I) -> Result<(), MdfError>
    where
        I: IntoIterator<Item = &'a [DecodedValue]>,
    {
        for record in records {
            self.write_record(cg_id, record)?;
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
