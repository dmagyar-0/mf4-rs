use mf4_rs::writer::MdfWriter;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::api::mdf::MDF;
use mf4_rs::merge::merge_files;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;

#[test]
fn merge_simple_files() -> Result<(), MdfError> {
    let dir = std::env::temp_dir();
    let f1 = dir.join("mf4_merge_1.mf4");
    let f2 = dir.join("mf4_merge_2.mf4");
    let out = dir.join("mf4_merge_out.mf4");
    for p in [&f1, &f2, &out] { if p.exists() { std::fs::remove_file(p)?; } }

    // first file with one record value 1
    let mut w1 = MdfWriter::new(f1.to_str().unwrap())?;
    w1.init_mdf_file()?;
    let cg1 = w1.add_channel_group(None, |_| {})?;
    w1.add_channel(&cg1, None, |ch| { ch.data_type = DataType::UnsignedIntegerLE; })?;
    w1.start_data_block_for_cg(&cg1, 0)?;
    w1.write_record(&cg1, &[DecodedValue::UnsignedInteger(1)])?;
    w1.finish_data_block(&cg1)?;
    w1.finalize()?;

    // second file with one record value 2
    let mut w2 = MdfWriter::new(f2.to_str().unwrap())?;
    w2.init_mdf_file()?;
    let cg2 = w2.add_channel_group(None, |_| {})?;
    w2.add_channel(&cg2, None, |ch| { ch.data_type = DataType::UnsignedIntegerLE; })?;
    w2.start_data_block_for_cg(&cg2, 0)?;
    w2.write_record(&cg2, &[DecodedValue::UnsignedInteger(2)])?;
    w2.finish_data_block(&cg2)?;
    w2.finalize()?;

    merge_files(out.to_str().unwrap(), f1.to_str().unwrap(), f2.to_str().unwrap())?;

    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1);
    let channels = groups[0].channels();
    let values = channels[0].values()?;
    assert_eq!(values.len(), 2);

    for p in [&f1, &f2, &out] { std::fs::remove_file(p)?; }
    Ok(())
}

#[test]
fn merge_different_files() -> Result<(), MdfError> {
    let dir = std::env::temp_dir();
    let f1 = dir.join("mf4_merge_a.mf4");
    let f2 = dir.join("mf4_merge_b.mf4");
    let out = dir.join("mf4_merge_out_diff.mf4");
    for p in [&f1, &f2, &out] { if p.exists() { std::fs::remove_file(p)?; } }

    // file 1 with channel A
    let mut w1 = MdfWriter::new(f1.to_str().unwrap())?;
    w1.init_mdf_file()?;
    let cg1 = w1.add_channel_group(None, |_| {})?;
    w1.add_channel(&cg1, None, |ch| { ch.data_type = DataType::UnsignedIntegerLE; ch.name = Some("A".into()); })?;
    w1.start_data_block_for_cg(&cg1, 0)?;
    w1.write_record(&cg1, &[DecodedValue::UnsignedInteger(1)])?;
    w1.finish_data_block(&cg1)?;
    w1.finalize()?;

    // file 2 with channel B
    let mut w2 = MdfWriter::new(f2.to_str().unwrap())?;
    w2.init_mdf_file()?;
    let cg2 = w2.add_channel_group(None, |_| {})?;
    w2.add_channel(&cg2, None, |ch| { ch.data_type = DataType::UnsignedIntegerLE; ch.name = Some("B".into()); })?;
    w2.start_data_block_for_cg(&cg2, 0)?;
    w2.write_record(&cg2, &[DecodedValue::UnsignedInteger(2)])?;
    w2.finish_data_block(&cg2)?;
    w2.finalize()?;

    merge_files(out.to_str().unwrap(), f1.to_str().unwrap(), f2.to_str().unwrap())?;

    let mdf = MDF::from_file(out.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 2);

    for p in [&f1, &f2, &out] { std::fs::remove_file(p)?; }
    Ok(())
}
