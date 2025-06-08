use mf4_rs::error::MdfError;
use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::channel_block::ChannelBlock;
use mf4_rs::blocks::channel_group_block::ChannelGroupBlock;
use mf4_rs::blocks::common::DataType;
use mf4_rs::api::mdf::MDF;

fn write_simple_mdf_file_multi(file_path: &str, groups: &[Vec<ChannelBlock>]) -> Result<(), MdfError> {
    let mut writer = MdfWriter::new(file_path)?;
    let (_id, _hd) = writer.init_mdf_file()?;
    let dg_id = writer.add_data_group(None)?;
    let cg_block = ChannelGroupBlock::default();

    let mut prev_cg: Option<String> = None;
    for channels in groups {
        let cg_id = writer.add_channel_group(&dg_id, prev_cg.as_deref(), &cg_block)?;
        prev_cg = Some(cg_id.clone());

        let mut prev_cn: Option<String> = None;
        for ch in channels {
            let cn_id = writer.add_channel(&cg_id, prev_cn.as_deref(), ch)?;
            prev_cn = Some(cn_id);
        }
    }

    writer.finalize()
}

fn main() -> Result<(), MdfError> {
    // Prepare two channel groups with different channels
    let mut ch1 = ChannelBlock::default();
    ch1.byte_offset = 0;
    ch1.bit_count = 32;
    ch1.data_type = DataType::UnsignedIntegerLE;
    ch1.name = Some("Speed".into());

    let mut ch2 = ch1.clone();
    ch2.byte_offset = 4;
    ch2.name = Some("RPM".into());

    let mut ch3 = ChannelBlock::default();
    ch3.byte_offset = 0;
    ch3.bit_count = 16;
    ch3.data_type = DataType::UnsignedIntegerLE;
    ch3.name = Some("Temperature".into());

    let groups = vec![vec![ch1, ch2], vec![ch3]];

    // Write file with helper
    write_simple_mdf_file_multi("multi_groups.mf4", &groups)?;

    // Parse using this crate
    let mdf = MDF::from_file("multi_groups.mf4")?;
    println!("Groups: {}", mdf.channel_groups().len());

    Ok(())
}
