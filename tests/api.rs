use mf4_rs::writer::MdfWriter;
use mf4_rs::api::mdf::MDF;
use mf4_rs::parsing::decoder::{decode_channel_value, DecodedValue};
use mf4_rs::blocks::channel_block::ChannelBlock;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;

#[test]
fn writer_and_parser_roundtrip() -> Result<(), MdfError> {
    let path = std::env::temp_dir().join("simple_test.mf4");
    if path.exists() {
        std::fs::remove_file(&path)?;
    }

    MdfWriter::write_simple_mdf_file(path.to_str().unwrap())?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1);
    let cg = &groups[0];
    assert!(cg.name()?.is_none());
    let channels = cg.channels();
    assert_eq!(channels.len(), 2);
    assert_eq!(channels[0].name()?.as_deref(), Some("Channel 1"));
    assert_eq!(channels[1].name()?.as_deref(), Some("Channel 2"));
    assert!(channels[0].values()?.is_empty());
    assert!(channels[1].values()?.is_empty());

    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn writer_data_roundtrip() -> Result<(), MdfError> {
    let path = std::env::temp_dir().join("data_test.mf4");
    if path.exists() {
        std::fs::remove_file(&path)?;
    }

    let mut writer = MdfWriter::new(path.to_str().unwrap())?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    let cn1 = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
    })?;
    writer.add_channel(&cg_id, Some(&cn1), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
    })?;

    writer.start_data_block_for_cg(&cg_id, 0)?;
    writer.write_record(
        &cg_id,
        &[DecodedValue::UnsignedInteger(1), DecodedValue::UnsignedInteger(2)],
    )?;
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1);
    let cg = &groups[0];
    let channels = cg.channels();
    assert_eq!(channels.len(), 2);
    let vals1 = channels[0].values()?;
    let vals2 = channels[1].values()?;
    assert_eq!(vals1.len(), 1);
    assert_eq!(vals2.len(), 1);
    match &vals1[0] {
        DecodedValue::UnsignedInteger(v) => assert_eq!(*v, 1),
        other => panic!("unexpected {:?}", other),
    }
    match &vals2[0] {
        DecodedValue::UnsignedInteger(v) => assert_eq!(*v, 2),
        other => panic!("unexpected {:?}", other),
    }

    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn writer_write_records() -> Result<(), MdfError> {
    let path = std::env::temp_dir().join("bulk_test.mf4");
    if path.exists() { std::fs::remove_file(&path)?; }

    let mut writer = MdfWriter::new(path.to_str().unwrap())?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    writer.add_channel(&cg_id, None, |ch| { ch.data_type = DataType::UnsignedIntegerLE; })?;

    writer.start_data_block_for_cg(&cg_id, 0)?;
    let recs = vec![
        vec![DecodedValue::UnsignedInteger(1)],
        vec![DecodedValue::UnsignedInteger(2)],
    ];
    let slices: Vec<&[DecodedValue]> = recs.iter().map(|r| r.as_slice()).collect();
    writer.write_records(&cg_id, slices)?;
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    let vals = groups[0].channels()[0].values()?;
    assert_eq!(vals.len(), 2);
    if let DecodedValue::UnsignedInteger(v) = vals[0] { assert_eq!(v, 1); } else { panic!("wrong type") }
    if let DecodedValue::UnsignedInteger(v) = vals[1] { assert_eq!(v, 2); } else { panic!("wrong type") }

    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn decode_channel_value_integer() {
    let mut ch = ChannelBlock::default();
    ch.data_type = DataType::UnsignedIntegerLE;
    ch.bit_count = 16;
    let record = [0x34, 0x12];
    match decode_channel_value(&record, 0, &ch).unwrap() {
        DecodedValue::UnsignedInteger(v) => assert_eq!(v, 0x1234),
        other => panic!("unexpected {:?}", other),
    }
}

#[test]
fn writer_block_position() -> Result<(), MdfError> {
    let path = std::env::temp_dir().join("pos_test.mf4");
    if path.exists() {
        std::fs::remove_file(&path)?;
    }

    let mut writer = MdfWriter::new(path.to_str().unwrap())?;
    let bytes = [1u8, 2, 3, 4];
    let pos = writer.write_block_with_id(&bytes, "blk")?;
    assert_eq!(writer.get_block_position("blk"), Some(pos));
    writer.finalize()?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn cut_mdf_file_by_time() -> Result<(), MdfError> {
    let input = std::env::temp_dir().join("cut_input.mf4");
    let output = std::env::temp_dir().join("cut_output.mf4");
    if input.exists() { std::fs::remove_file(&input)?; }
    if output.exists() { std::fs::remove_file(&output)?; }

    // Create source file with a time channel and a value channel
    let mut writer = MdfWriter::new(input.to_str().unwrap())?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    let time_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".into());
        ch.bit_count = 64;
    })?;
    writer.set_time_channel(&time_id)?;
    writer.add_channel(&cg_id, Some(&time_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.bit_count = 32;
        ch.name = Some("Val".into());
    })?;
    writer.start_data_block_for_cg(&cg_id, 0)?;
    for i in 0..10u64 {
        writer.write_record(
            &cg_id,
            &[
                DecodedValue::Float(i as f64 * 0.1),
                DecodedValue::UnsignedInteger(i),
            ],
        )?;
    }
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Cut between 0.2 and 0.5 seconds
    mf4_rs::cut::cut_mdf_by_time(
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        0.2,
        0.5,
    )?;

    let mdf = MDF::from_file(output.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1);
    let cg = &groups[0];
    let chs = cg.channels();
    assert_eq!(chs.len(), 2);
    let times = chs[0].values()?;
    let vals = chs[1].values()?;
    assert_eq!(times.len(), 4);
    assert_eq!(vals.len(), 4);
    if let DecodedValue::Float(t0) = times[0] { assert!((t0 - 0.2).abs() < 1e-6); }
    if let DecodedValue::Float(t_last) = times[3] { assert!((t_last - 0.5).abs() < 1e-6); }

    std::fs::remove_file(input)?;
    std::fs::remove_file(output)?;
    Ok(())
}
