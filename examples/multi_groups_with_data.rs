use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::channel_block::ChannelBlock;
use mf4_rs::blocks::channel_group_block::ChannelGroupBlock;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::api::mdf::MDF;
use mf4_rs::error::MdfError;

fn main() -> Result<(), MdfError> {
    // Create writer and basic structure
    let mut writer = MdfWriter::new("multi_group_data.mf4")?;
    let (_id, _hd) = writer.init_mdf_file()?;
    // Create a data group for the first channel group
    let dg1_id = writer.add_data_group(None)?;
    let cg_block = ChannelGroupBlock::default();

    // -------- Channel Group 1 with 2 channels --------
    let cg1_id = writer.add_channel_group(&dg1_id, None, &cg_block)?;
    let mut ch1 = ChannelBlock::default();
    ch1.byte_offset = 0;
    ch1.bit_count = 32;
    ch1.data_type = DataType::UnsignedIntegerLE;
    ch1.name = Some("Speed".into());
    let mut ch2 = ch1.clone();
    ch2.byte_offset = 4;
    ch2.name = Some("RPM".into());
    let cn1_id = writer.add_channel(&cg1_id, None, &ch1)?;
    writer.add_channel(&cg1_id, Some(&cn1_id), &ch2)?;

    // Create a second data group for the next channel group
    let dg2_id = writer.add_data_group(Some(&dg1_id))?;

    // -------- Channel Group 2 with 3 channels --------
    let cg2_id = writer.add_channel_group(&dg2_id, None, &cg_block)?;

    let mut ch3 = ChannelBlock::default();
    ch3.byte_offset = 0;
    ch3.bit_count = 16;
    ch3.data_type = DataType::SignedIntegerLE;
    ch3.name = Some("Temperature".into());

    let mut ch4 = ChannelBlock::default();
    ch4.byte_offset = 2;
    ch4.bit_count = 32;
    ch4.data_type = DataType::FloatLE;
    ch4.name = Some("Pressure".into());

    // Value to text conversion for a status channel
    let status_map = [(0i64, "OK"), (1i64, "WARN")];
    let (_cc_id, cc_pos) = writer.add_value_to_text_conversion(&status_map, "UNKNOWN")?;
    let mut ch5 = ChannelBlock::default();
    ch5.byte_offset = 6;
    ch5.bit_count = 8;
    ch5.data_type = DataType::UnsignedIntegerLE;
    ch5.name = Some("Status".into());
    ch5.conversion_addr = cc_pos;

    let cn3_id = writer.add_channel(&cg2_id, None, &ch3)?;
    let cn4_id = writer.add_channel(&cg2_id, Some(&cn3_id), &ch4)?;
    writer.add_channel(&cg2_id, Some(&cn4_id), &ch5)?;

    // -------- Write sample data for both groups --------
    // Write 100 records for the first group
    writer.start_data_block(&dg1_id, &cg1_id, 0, &[ch1.clone(), ch2.clone()])?;
    for i in 0u32..100 {
        writer.write_record(
            &cg1_id,
            &[
                DecodedValue::UnsignedInteger(i.into()),
                DecodedValue::UnsignedInteger((i * 2).into()),
            ],
        )?;
    }
    writer.finish_data_block(&cg1_id)?;

    // Write 100 records for the second group
    writer.start_data_block(&dg2_id, &cg2_id, 0, &[ch3.clone(), ch4.clone(), ch5.clone()])?;
    for i in 0u32..100 {
        writer.write_record(
            &cg2_id,
            &[
                DecodedValue::SignedInteger(i as i64 - 50),
                DecodedValue::Float(i as f64 * 0.1),
                DecodedValue::UnsignedInteger((i % 2).into()),
            ],
        )?;
    }
    writer.finish_data_block(&cg2_id)?;

    writer.finalize()?;

    // -------- Verify using the crate parser --------
    let mdf = MDF::from_file("multi_group_data.mf4")?;
    println!("Our parser found {} channel groups", mdf.channel_groups().len());
    for (idx, group) in mdf.channel_groups().iter().enumerate() {
        let chans = group.channels();
        print!("  Group {} has {} channels", idx + 1, chans.len());
        if let Some(ch) = chans.first() {
            let values = ch.values()?;
            println!(" and {} records", values.len());
        } else {
            println!();
        }
    }

    // Optionally verify with asammdf (Python)
    std::process::Command::new("python3")
        .arg("-c")
        .arg("import asammdf,sys;m=asammdf.MDF('multi_group_data.mf4');\nprint('asammdf channels',len(m.channels_db))")
        .status()
        .ok();

    Ok(())
}

