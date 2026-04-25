use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

/// Cut should preserve fixed-length ByteArray channels byte-for-byte.
#[test]
fn cut_preserves_byte_array_channel() -> Result<(), MdfError> {
    let input = std::env::temp_dir().join("cut_bytes_input.mf4");
    let output = std::env::temp_dir().join("cut_bytes_output.mf4");
    if input.exists() {
        std::fs::remove_file(&input)?;
    }
    if output.exists() {
        std::fs::remove_file(&output)?;
    }

    // Source file: time + 4-byte ByteArray "tag" channel + uint32 value channel.
    let mut writer = MdfWriter::new(input.to_str().unwrap())?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    let time_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("Time".into());
    })?;
    writer.set_time_channel(&time_id)?;
    let tag_id = writer.add_channel(&cg_id, Some(&time_id), |ch| {
        ch.data_type = DataType::ByteArray;
        ch.bit_count = 32; // 4 bytes
        ch.name = Some("Tag".into());
    })?;
    writer.add_channel(&cg_id, Some(&tag_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.bit_count = 32;
        ch.name = Some("Val".into());
    })?;
    writer.start_data_block_for_cg(&cg_id, 0)?;

    let payloads: [[u8; 4]; 10] = [
        *b"AAAA", *b"BBBB", *b"CCCC", *b"DDDD", *b"EEEE",
        *b"FFFF", *b"GGGG", *b"HHHH", *b"IIII", *b"JJJJ",
    ];
    for i in 0..10u64 {
        writer.write_record(
            &cg_id,
            &[
                DecodedValue::Float(i as f64 * 0.1),
                DecodedValue::ByteArray(payloads[i as usize].to_vec()),
                DecodedValue::UnsignedInteger(i),
            ],
        )?;
    }
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Cut to [0.3, 0.6] — expect records 3, 4, 5, 6.
    mf4_rs::cut::cut_mdf_by_time(
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        0.3,
        0.6,
    )?;

    let mdf = MDF::from_file(output.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1);
    let chs = groups[0].channels();
    assert_eq!(chs.len(), 3);

    let times = chs[0].values()?;
    let tags = chs[1].values()?;
    let vals = chs[2].values()?;
    assert_eq!(times.len(), 4);
    assert_eq!(tags.len(), 4);
    assert_eq!(vals.len(), 4);

    let expected_tags = [b"DDDD", b"EEEE", b"FFFF", b"GGGG"];
    for (i, tag_opt) in tags.iter().enumerate() {
        match tag_opt {
            Some(DecodedValue::ByteArray(b)) => assert_eq!(b.as_slice(), expected_tags[i]),
            other => panic!("expected ByteArray, got {:?}", other),
        }
    }
    if let Some(DecodedValue::UnsignedInteger(v0)) = vals[0] {
        assert_eq!(v0, 3);
    } else {
        panic!("unexpected first value: {:?}", vals[0]);
    }
    if let Some(DecodedValue::UnsignedInteger(v3)) = vals[3] {
        assert_eq!(v3, 6);
    } else {
        panic!("unexpected last value: {:?}", vals[3]);
    }

    std::fs::remove_file(input)?;
    std::fs::remove_file(output)?;
    Ok(())
}
