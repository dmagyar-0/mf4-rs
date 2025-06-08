use crate::error::MdfError;
use crate::writer::MdfWriter;
use crate::parsing::mdf_file::MdfFile;
use crate::parsing::decoder::{decode_channel_value, DecodedValue};
use crate::blocks::common::{DataType, read_string_block};

#[derive(Clone)]
struct ChannelMeta {
    name: Option<String>,
    data_type: DataType,
    bit_offset: u8,
    byte_offset: u32,
    bit_count: u32,
    channel_type: u8,
}

#[derive(Clone)]
struct GroupMeta {
    record_id_len: u8,
    channels: Vec<ChannelMeta>,
}

struct MergedGroup {
    meta: GroupMeta,
    data: Vec<Vec<DecodedValue>>, // per channel
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
                });
            }
            let mut data: Vec<Vec<DecodedValue>> = metas.iter().map(|_| Vec::new()).collect();
            for (idx, ch) in cg.raw_channels.iter().enumerate() {
                let mut iter = ch.records(dg, cg, mmap)?;
                while let Some(rec) = iter.next() {
                    let bytes = rec?;
                    let val = decode_channel_value(bytes, record_id_len as usize, &ch.block)
                        .unwrap_or(DecodedValue::Unknown);
                    data[idx].push(val);
                }
            }
            groups.push(MergedGroup { meta: GroupMeta { record_id_len, channels: metas }, data });
        }
    }
    Ok(groups)
}

fn metas_equal(a: &GroupMeta, b: &GroupMeta) -> bool {
    if a.channels.len() != b.channels.len() {
        return false;
    }
    for (ca, cb) in a.channels.iter().zip(&b.channels) {
        if ca.name != cb.name ||
           ca.data_type != cb.data_type ||
           ca.bit_offset != cb.bit_offset ||
           ca.byte_offset != cb.byte_offset ||
           ca.bit_count != cb.bit_count ||
           ca.channel_type != cb.channel_type {
            return false;
        }
    }
    true
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
        if let Some(g1) = groups.iter_mut().find(|g| metas_equal(&g.meta, &og.meta)) {
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
                cn.channel_type = ch.channel_type;
                cn.data_type = ch.data_type.clone();
                cn.bit_offset = ch.bit_offset;
                cn.byte_offset = ch.byte_offset;
                cn.bit_count = ch.bit_count;
                if let Some(n) = &ch.name {
                    cn.name = Some(n.clone());
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
