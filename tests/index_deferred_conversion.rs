//! Verifies that indexes built over a [`ByteRangeReader`] (the remote / URL
//! path) defer conversion resolution: only the conversion block's location is
//! recorded at build time, and the conversion is fetched + applied lazily on
//! the first value read. Uses an in-memory [`SliceRangeReader`] so it runs
//! without the `http` feature.

use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::index::{MdfIndex, SliceRangeReader};
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

/// Build a small file with a value-to-text conversion on a "State" channel and
/// return its raw bytes.
fn build_fixture() -> Result<Vec<u8>, MdfError> {
    let tmp = std::env::temp_dir().join(format!(
        "deferred_conv_{}.mf4",
        std::process::id()
    ));
    let path = tmp.to_str().unwrap().to_string();

    let mut writer = MdfWriter::new(&path)?;
    writer.init_mdf_file()?;

    let cg_id = writer.add_channel_group(None, |_| {})?;
    let time_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".to_string());
        ch.bit_count = 64;
    })?;
    writer.set_time_channel(&time_id)?;

    let state_id = writer.add_channel(&cg_id, Some(&time_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("State".to_string());
        ch.bit_count = 32;
    })?;
    writer.add_value_to_text_conversion(&[(0, "OFF"), (1, "ON")], "UNKNOWN", Some(&state_id))?;

    writer.start_data_block_for_cg(&cg_id, 0)?;
    for r in 0..6u64 {
        writer.write_record(
            &cg_id,
            &[
                DecodedValue::Float(r as f64 * 0.1),
                DecodedValue::UnsignedInteger(r % 3),
            ],
        )?;
    }
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    let bytes = std::fs::read(&path)?;
    let _ = std::fs::remove_file(&path);
    Ok(bytes)
}

#[test]
fn range_reader_build_defers_conversion_and_reads_lazily() -> Result<(), MdfError> {
    let bytes = build_fixture()?;
    let file_size = bytes.len() as u64;

    // Build the index over a byte-range reader (the remote path).
    let mut reader = SliceRangeReader::new(bytes.clone());
    let index = MdfIndex::from_range_reader(&mut reader, file_size)?;

    // The conversion must NOT be resolved at build time — only its location
    // recorded.
    let state = index.channel("State").expect("State channel present");
    assert!(
        state.conversion.is_none(),
        "conversion should be resolved lazily, not at build time"
    );
    assert!(
        state.conversion_addr != 0,
        "the conversion block location should be recorded for lazy resolution"
    );

    // Reading applies the (now lazily fetched) conversion: raw 0/1/2 map to
    // OFF / ON / UNKNOWN (the default).
    let mut bound = index.open(SliceRangeReader::new(bytes));
    let values = bound.values("State")?;
    let strings: Vec<String> = values
        .into_iter()
        .map(|v| match v {
            Some(DecodedValue::String(s)) => s,
            other => panic!("expected converted String value, got {:?}", other),
        })
        .collect();
    assert_eq!(strings, vec!["OFF", "ON", "UNKNOWN", "OFF", "ON", "UNKNOWN"]);

    Ok(())
}
