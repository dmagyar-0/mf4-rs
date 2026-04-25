use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

/// Cut should preserve `invalidation_bytes_nr` on the new channel group and
/// copy each kept record's invalidation byte verbatim, so invalid samples
/// remain invalid after cutting.
#[test]
fn cut_preserves_invalidation_bits() -> Result<(), MdfError> {
    let input = std::env::temp_dir().join("cut_inval_input.mf4");
    let output = std::env::temp_dir().join("cut_inval_output.mf4");
    if input.exists() {
        std::fs::remove_file(&input)?;
    }
    if output.exists() {
        std::fs::remove_file(&output)?;
    }

    // Source file: time master + uint32 value channel that uses invalidation
    // bit 0 in the single invalidation byte. cn_flags bit 1 is set so the
    // reader actually consults the invalidation bit per record.
    let mut writer = MdfWriter::new(input.to_str().unwrap())?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    let time_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("Time".into());
    })?;
    writer.set_time_channel(&time_id)?;
    writer.add_channel(&cg_id, Some(&time_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.bit_count = 32;
        ch.flags = 0x02; // invalidation bit valid
        ch.pos_invalidation_bit = 0;
        ch.name = Some("Val".into());
    })?;

    // Write 10 records manually: record = 8 bytes time (f64) + 4 bytes value
    // (u32) + 1 byte invalidation. Mark records 2 and 4 as invalid.
    writer.start_data_block_for_cg_raw(
        &cg_id,
        /* record_id_len */ 0,
        /* data_bytes */ 12,
        /* invalidation_bytes */ 1,
    )?;
    for i in 0..10u64 {
        let mut record = Vec::with_capacity(13);
        record.extend_from_slice(&(i as f64 * 0.1).to_le_bytes());
        record.extend_from_slice(&(i as u32).to_le_bytes());
        let inval_byte: u8 = if i == 2 || i == 4 { 0x01 } else { 0x00 };
        record.push(inval_byte);
        writer.write_raw_record(&cg_id, &record)?;
    }
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Cut [0.1, 0.5] — kept records: 1,2,3,4,5 (where 2 and 4 are invalid).
    mf4_rs::cut::cut_mdf_by_time(
        input.to_str().unwrap(),
        output.to_str().unwrap(),
        0.1,
        0.5,
    )?;

    let mdf = MDF::from_file(output.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 1);
    let chs = groups[0].channels();
    let times = chs[0].values()?;
    let vals = chs[1].values()?;

    assert_eq!(times.len(), 5, "expected 5 records in cut window");
    assert_eq!(vals.len(), 5);

    // Records at relative i = 1,2,3,4,5. Records 2 and 4 (positions 1 and 3
    // in the cut output) should be reported invalid (None).
    let valid_pattern: Vec<bool> = vals.iter().map(|v| v.is_some()).collect();
    assert_eq!(
        valid_pattern,
        vec![true, false, true, false, true],
        "invalidation pattern not preserved: {:?}",
        valid_pattern
    );

    // The valid samples should carry the original raw values (no conversion
    // attached, so raw == phys).
    if let Some(DecodedValue::UnsignedInteger(v)) = vals[0] {
        assert_eq!(v, 1);
    } else {
        panic!("unexpected value at position 0: {:?}", vals[0]);
    }
    if let Some(DecodedValue::UnsignedInteger(v)) = vals[4] {
        assert_eq!(v, 5);
    } else {
        panic!("unexpected value at position 4: {:?}", vals[4]);
    }
    if let Some(DecodedValue::Float(t)) = times[2] {
        assert!((t - 0.3).abs() < 1e-9);
    } else {
        panic!("unexpected time at position 2: {:?}", times[2]);
    }

    std::fs::remove_file(input)?;
    std::fs::remove_file(output)?;
    Ok(())
}
