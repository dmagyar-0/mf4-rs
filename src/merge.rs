use byteorder::{ByteOrder, BigEndian, LittleEndian};

use crate::error::MdfError;
use crate::writer::MdfWriter;
use crate::parsing::mdf_file::MdfFile;
use crate::parsing::decoder::{decode_channel_value, DecodedValue};
use crate::blocks::common::{DataType, read_string_block};

#[derive(Debug, Clone)]
struct ChannelMeta {
    name: Option<String>,
    data_type: DataType,
    bit_offset: u8,
    byte_offset: u32,
    bit_count: u32,
    channel_type: u8,
    /// Whether the source channel was VLSD (channel_type == 1 && data != 0).
    /// The raw `data` address differs between source files, so equality is
    /// reduced to a boolean.
    is_vlsd: bool,
}

impl ChannelMeta {
    fn matches(&self, other: &Self) -> bool {
        self.name == other.name
            && self.data_type == other.data_type
            && self.bit_offset == other.bit_offset
            && self.byte_offset == other.byte_offset
            && self.bit_count == other.bit_count
            && self.channel_type == other.channel_type
            && self.is_vlsd == other.is_vlsd
    }
}

#[derive(Debug, Clone)]
struct GroupMeta {
    record_id_len: u8,
    channels: Vec<ChannelMeta>,
}

impl GroupMeta {
    fn matches(&self, other: &Self) -> bool {
        self.record_id_len == other.record_id_len
            && self.channels.len() == other.channels.len()
            && self.channels.iter().zip(other.channels.iter()).all(|(a, b)| a.matches(b))
    }
}

struct MergedGroup {
    meta: GroupMeta,
    data: Vec<Vec<DecodedValue>>, // per channel
}

fn vlsd_payload_to_value(bytes: &[u8], data_type: &DataType) -> DecodedValue {
    match data_type {
        DataType::StringUtf8 => match std::str::from_utf8(bytes) {
            Ok(s) => DecodedValue::String(s.trim_end_matches('\0').to_string()),
            Err(_) => DecodedValue::String(String::from("<Invalid UTF8>")),
        },
        DataType::StringLatin1 => {
            let s: String = bytes.iter().map(|&b| b as char).collect();
            DecodedValue::String(s.trim_end_matches('\0').to_string())
        }
        DataType::StringUtf16LE => {
            if bytes.len() % 2 != 0 {
                return DecodedValue::String(String::from("<Invalid UTF16LE>"));
            }
            let u16_data: Vec<u16> = bytes.chunks_exact(2).map(LittleEndian::read_u16).collect();
            match String::from_utf16(&u16_data) {
                Ok(s) => DecodedValue::String(s.trim_end_matches('\0').to_string()),
                Err(_) => DecodedValue::String(String::from("<Invalid UTF16LE>")),
            }
        }
        DataType::StringUtf16BE => {
            if bytes.len() % 2 != 0 {
                return DecodedValue::String(String::from("<Invalid UTF16BE>"));
            }
            let u16_data: Vec<u16> = bytes.chunks_exact(2).map(BigEndian::read_u16).collect();
            match String::from_utf16(&u16_data) {
                Ok(s) => DecodedValue::String(s.trim_end_matches('\0').to_string()),
                Err(_) => DecodedValue::String(String::from("<Invalid UTF16BE>")),
            }
        }
        DataType::MimeSample => DecodedValue::MimeSample(bytes.to_vec()),
        DataType::MimeStream => DecodedValue::MimeStream(bytes.to_vec()),
        _ => DecodedValue::ByteArray(bytes.to_vec()),
    }
}

fn collect_groups(file: &MdfFile) -> Result<Vec<MergedGroup>, MdfError> {
    let mut groups = Vec::new();
    let mmap = &file.mmap;
    for dg in &file.data_groups {
        let record_id_len = dg.block.record_id_len;
        for cg in &dg.channel_groups {
            let mut metas = Vec::new();
            for ch in &cg.raw_channels {
                let name = read_string_block(mmap, ch.block.name_addr)?;
                metas.push(ChannelMeta {
                    name,
                    data_type: ch.block.data_type.clone(),
                    bit_offset: ch.block.bit_offset,
                    byte_offset: ch.block.byte_offset,
                    bit_count: ch.block.bit_count,
                    channel_type: ch.block.channel_type,
                    is_vlsd: ch.block.channel_type == 1 && ch.block.data != 0,
                });
            }
            let mut data: Vec<Vec<DecodedValue>> = metas.iter().map(|_| Vec::new()).collect();
            for (idx, ch) in cg.raw_channels.iter().enumerate() {
                let is_vlsd = ch.block.channel_type == 1 && ch.block.data != 0;
                let mut iter = ch.records(dg, cg, mmap)?;
                while let Some(rec) = iter.next() {
                    let bytes = rec?;
                    let val = if is_vlsd {
                        vlsd_payload_to_value(bytes, &ch.block.data_type)
                    } else {
                        decode_channel_value(bytes, record_id_len as usize, &ch.block)
                            .unwrap_or(DecodedValue::Unknown)
                    };
                    data[idx].push(val);
                }
            }
            groups.push(MergedGroup { meta: GroupMeta { record_id_len, channels: metas }, data });
        }
    }
    Ok(groups)
}


/// Merge two MDF files into a new file.
///
/// All channel groups that share the same layout are concatenated. Groups that
/// do not match are appended as new channel groups. The resulting file is
/// written to `output`.
///
/// # Arguments
/// * `output` - Path for the merged file
/// * `first` - Path to the first input file
/// * `second` - Path to the second input file
///
/// # Returns
/// `Ok(())` on success or an [`MdfError`] otherwise.
pub fn merge_files(output: &str, first: &str, second: &str) -> Result<(), MdfError> {
    let mdf1 = MdfFile::parse_from_file(first)?;
    let mdf2 = MdfFile::parse_from_file(second)?;

    let mut groups = collect_groups(&mdf1)?;
    let other_groups = collect_groups(&mdf2)?;

    for og in other_groups {
        if let Some(g1) = groups.iter_mut().find(|g| g.meta.matches(&og.meta)) {
            for (vals1, vals2) in g1.data.iter_mut().zip(og.data.into_iter()) {
                vals1.extend(vals2);
            }
        } else {
            groups.push(og);
        }
    }

    let mut writer = MdfWriter::new(output)?;
    writer.init_mdf_file()?;

    for group in groups {
        let cg_id = writer.add_channel_group(None, |_| {})?;
        let mut last_cn: Option<String> = None;
        for ch in &group.meta.channels {
            let id = writer.add_channel(&cg_id, last_cn.as_deref(), |cn| {
                cn.data_type = ch.data_type.clone();
                if let Some(n) = &ch.name {
                    cn.name = Some(n.clone());
                }
                if ch.is_vlsd {
                    cn.channel_type = 1;
                    // Non-zero placeholder so `start_data_block` recognises this
                    // channel as VLSD; `finish_data_block` will overwrite the
                    // link with the real ##SD address.
                    cn.data = 1;
                    cn.bit_offset = 0;
                    cn.byte_offset = ch.byte_offset;
                    cn.bit_count = 64;
                } else {
                    cn.channel_type = ch.channel_type;
                    cn.bit_offset = ch.bit_offset;
                    cn.byte_offset = ch.byte_offset;
                    cn.bit_count = ch.bit_count;
                }
            })?;
            last_cn = Some(id);
        }
        writer.start_data_block_for_cg(&cg_id, group.meta.record_id_len)?;
        let record_count = group.data.get(0).map(|v| v.len()).unwrap_or(0);
        for i in 0..record_count {
            let mut vals = Vec::new();
            for ch_data in &group.data {
                vals.push(ch_data[i].clone());
            }
            writer.write_record(&cg_id, &vals)?;
        }
        writer.finish_data_block(&cg_id)?;
    }

    writer.finalize()
}
