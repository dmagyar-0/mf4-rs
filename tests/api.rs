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
