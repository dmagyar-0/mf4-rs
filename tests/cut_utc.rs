use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

/// `cut_mdf_by_utc_ns` should produce the same output as `cut_mdf_by_time`
/// when given equivalent absolute timestamps anchored at the file's
/// `HD.abs_time`.
#[test]
fn cut_by_utc_ns_matches_relative_cut() -> Result<(), MdfError> {
    let input = std::env::temp_dir().join("cut_utc_input.mf4");
    let out_rel = std::env::temp_dir().join("cut_utc_rel.mf4");
    let out_utc = std::env::temp_dir().join("cut_utc_abs.mf4");
    for p in [&input, &out_rel, &out_utc] {
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }

    // Source: simple time + uint32 channel, 10 records at t = i * 0.1s.
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

    // Read the file's absolute start time so we can build matching UTC ns
    // bounds. Default writer header sets a non-zero abs_time, so this is
    // always Some(_) for files we just wrote.
    let anchor_ns = MDF::from_file(input.to_str().unwrap())?
        .start_time_ns()
        .expect("file should have non-zero abs_time");

    // Relative cut window: [0.2, 0.5] seconds.
    let start_rel = 0.2_f64;
    let end_rel = 0.5_f64;
    let start_utc_ns = anchor_ns as i64 + (start_rel * 1.0e9).round() as i64;
    let end_utc_ns = anchor_ns as i64 + (end_rel * 1.0e9).round() as i64;

    mf4_rs::cut::cut_mdf_by_time(
        input.to_str().unwrap(),
        out_rel.to_str().unwrap(),
        start_rel,
        end_rel,
    )?;
    mf4_rs::cut::cut_mdf_by_utc_ns(
        input.to_str().unwrap(),
        out_utc.to_str().unwrap(),
        start_utc_ns,
        end_utc_ns,
    )?;

    let read_vals = |path: &std::path::Path| -> Result<Vec<u64>, MdfError> {
        let mdf = MDF::from_file(path.to_str().unwrap())?;
        let chs = mdf.channel_groups()[0].channels();
        Ok(chs[1]
            .values()?
            .iter()
            .map(|v| match v {
                Some(DecodedValue::UnsignedInteger(u)) => *u,
                other => panic!("unexpected value: {:?}", other),
            })
            .collect())
    };

    let rel_vals = read_vals(&out_rel)?;
    let utc_vals = read_vals(&out_utc)?;
    assert_eq!(rel_vals, vec![2, 3, 4, 5]);
    assert_eq!(rel_vals, utc_vals, "UTC and relative cuts must match");

    for p in [&input, &out_rel, &out_utc] {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

/// A source file with `HD.abs_time == 0` cannot be cut by UTC because there
/// is no anchor; the helper must surface a clear error.
#[test]
fn cut_by_utc_ns_errors_without_abs_time() -> Result<(), MdfError> {
    use mf4_rs::blocks::common::BlockParse;
    use mf4_rs::blocks::header_block::HeaderBlock;
    use std::io::{Read, Seek, SeekFrom, Write};

    let path = std::env::temp_dir().join("cut_utc_zero_input.mf4");
    let out = std::env::temp_dir().join("cut_utc_zero_out.mf4");
    for p in [&path, &out] {
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }

    // Build a minimal valid file via the normal writer path.
    let mut writer = MdfWriter::new(path.to_str().unwrap())?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group(None, |_| {})?;
    let time_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.bit_count = 64;
        ch.name = Some("Time".into());
    })?;
    writer.set_time_channel(&time_id)?;
    writer.start_data_block_for_cg(&cg_id, 0)?;
    writer.write_record(&cg_id, &[DecodedValue::Float(0.0)])?;
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Patch HD.abs_time to 0 in place. HD lives at offset 64, abs_time is at
    // offset 64+24+6*8 = 136 (header 24 bytes + 6 link u64s = 72 bytes).
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)?;
    // Sanity: confirm we are about to overwrite an HD.abs_time field by
    // reading the surrounding HD block first.
    let mut buf = [0u8; 104];
    f.seek(SeekFrom::Start(64))?;
    f.read_exact(&mut buf)?;
    let hd = HeaderBlock::from_bytes(&buf)?;
    assert_eq!(hd.header.id, "##HD");
    assert!(hd.abs_time != 0, "fixture relies on writer setting non-zero abs_time");
    f.seek(SeekFrom::Start(64 + 24 + 6 * 8))?;
    f.write_all(&0u64.to_le_bytes())?;
    drop(f);

    // Now expect an error from cut_mdf_by_utc_ns.
    let err = mf4_rs::cut::cut_mdf_by_utc_ns(
        path.to_str().unwrap(),
        out.to_str().unwrap(),
        0,
        1_000_000,
    )
    .expect_err("expected an error for zero abs_time");
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("absolute start time") || msg.contains("abs_time"),
        "unexpected error message: {}",
        msg
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&out);
    Ok(())
}
