//! Regression tests for the index-system fixes:
//!
//! - A3: VLSD channels now read correctly through the index (values,
//!   values_f64, and the lazy Signal path) by resolving each record's inline
//!   8-byte SD offset into the captured ##SD fragment chain; only the static
//!   byte-range calculation still refuses VLSD channels.
//! - B7: cn_flags bit 0 ("all values invalid") must apply on the f64 fast
//!   path even when the group has no invalidation bytes.
//! - C6: hostile/stale index JSON (block size < 24, record_size == 0,
//!   forged huge sizes) must return Err instead of panicking or aborting.
//! - D7b: CachingRangeReader must serve reads near EOF when the resource
//!   size is not a multiple of the chunk size, over strict inner readers.

use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::index::{ByteRangeReader, CachingRangeReader, FileRangeReader, MdfIndex, SliceRangeReader};
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

fn tmp_path(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(name);
    if p.exists() {
        let _ = std::fs::remove_file(&p);
    }
    p
}

/// Write a small file with a VLSD string channel (channel_type = 1) next to
/// a float time channel, using the SD-block writer API.
fn write_vlsd_file(path: &str) -> Result<usize, MdfError> {
    const RECORD_LEN: usize = 16; // 8 bytes time + 8 bytes VLSD offset slot

    let mut w = MdfWriter::new(path)?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let t = w.add_channel(&cg, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&t)?;
    let vlsd = w.add_channel(&cg, Some(&t), |c| {
        c.data_type = DataType::StringUtf8;
        c.bit_count = 64;
        c.channel_type = 1; // VLSD
        c.data = 1; // patched to the real ##SD address by the writer
        c.name = Some("Msg".into());
    })?;
    w.start_data_block_for_cg_raw(&cg, 0, RECORD_LEN as u32, 0)?;
    w.start_signal_data_block(&vlsd)?;

    let payloads: [&[u8]; 3] = [b"alpha", b"bravo!", b"charlie"];
    let mut running: u64 = 0;
    for (i, p) in payloads.iter().enumerate() {
        let mut record = Vec::with_capacity(RECORD_LEN);
        record.extend_from_slice(&(i as f64 * 0.1).to_le_bytes());
        record.extend_from_slice(&running.to_le_bytes());
        w.write_raw_record(&cg, &record)?;
        w.write_signal_data(&vlsd, p)?;
        running += 4 + p.len() as u64;
    }
    w.finish_signal_data_block(&vlsd)?;
    w.finish_data_block(&cg)?;
    w.finalize()?;
    Ok(payloads.len())
}

/// A3: reading a VLSD channel through the index must SUCCEED with the correct
/// values on every value-read path (resolving the inline SD offsets), while
/// the static byte-range calculation still refuses VLSD channels.
#[test]
fn vlsd_channel_reads_resolve_correctly() -> Result<(), MdfError> {
    let path = tmp_path("fix_index_vlsd.mf4");
    let path_str = path.to_str().unwrap();
    let n = write_vlsd_file(path_str)?;
    let expected = ["alpha", "bravo!", "charlie"];

    let index = MdfIndex::from_file(path_str)?;

    // MdfReader::values (DecodedValue path) resolves the strings.
    let mut reader = index.open_file(path_str)?;
    let msgs = reader.values("Msg")?;
    assert_eq!(msgs.len(), n);
    for (got, want) in msgs.iter().zip(expected.iter()) {
        match got {
            Some(DecodedValue::String(s)) => assert_eq!(s, want),
            other => panic!("expected string {:?}, got {:?}", want, other),
        }
    }

    // MdfReader::values_f64 (fast f64 path): strings map to NaN, one per record.
    let f64s = reader.values_f64("Msg")?;
    assert_eq!(f64s.len(), n);
    assert!(f64s.iter().all(|v| v.is_nan()), "string VLSD values must be NaN");

    // Byte-range calculation still refuses VLSD channels.
    assert!(
        index.byte_ranges("Msg").is_err(),
        "byte_ranges() on a VLSD channel must error"
    );

    // Lazy source-based read (slice path via mmap) yields a Signal whose
    // timestamps align with the values (one per record).
    let sig = index.read("Msg")?;
    assert_eq!(sig.values.len(), n);
    assert_eq!(sig.timestamps.len(), n);
    for (got, want) in sig.values.iter().zip(expected.iter()) {
        match got {
            Some(DecodedValue::String(s)) => assert_eq!(s, want),
            other => panic!("expected string {:?}, got {:?}", want, other),
        }
    }

    // The fixed-size channel in the same group still reads fine.
    let times = reader.values_f64("Time")?;
    assert_eq!(times.len(), n);
    assert!(times.iter().all(|v| !v.is_nan()));

    std::fs::remove_file(&path)?;
    Ok(())
}

/// Write a plain two-channel float file used by the JSON-tampering tests.
fn write_basic_file(path: &str, records: usize) -> Result<(), MdfError> {
    let mut w = MdfWriter::new(path)?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let t = w.add_channel(&cg, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&t)?;
    w.add_channel(&cg, Some(&t), |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Value".into());
    })?;
    w.start_data_block_for_cg(&cg, 0)?;
    for r in 0..records {
        w.write_record(
            &cg,
            &[
                DecodedValue::Float(r as f64 * 0.1),
                DecodedValue::Float(r as f64),
            ],
        )?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()?;
    Ok(())
}

/// C6: hostile / stale index JSON must produce clean errors, not panics,
/// underflows, or allocation aborts.
#[test]
fn malformed_index_json_errors_instead_of_panicking() -> Result<(), MdfError> {
    let path = tmp_path("fix_index_json.mf4");
    let path_str = path.to_str().unwrap();
    write_basic_file(path_str, 10)?;

    let index = MdfIndex::from_file(path_str)?;
    let json = index.to_json()?;

    // (a) data block size < 24 (would underflow `size - 24`)
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["channel_groups"][0]["data_blocks"][0]["size"] = 10.into();
    assert!(
        MdfIndex::from_json(&v.to_string()).is_err(),
        "data block size < 24 must be rejected"
    );

    // (b) record_size == 0 (would divide by zero)
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["channel_groups"][0]["record_size"] = 0.into();
    v["channel_groups"][0]["record_id_len"] = 0.into();
    v["channel_groups"][0]["invalidation_bytes"] = 0.into();
    match MdfIndex::from_json(&v.to_string()) {
        Err(_) => {}
        Ok(idx) => {
            // If deserialization were permissive, reading must still error.
            let mut r = idx.open_file(path_str)?;
            assert!(r.values("Value").is_err(), "record_size == 0 must error on read");
        }
    }

    // (c) forged huge data block size: must error (FileRangeReader validates
    // against the real file length) instead of aborting on a huge allocation.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    // 24-byte header + 2^36 * 16-byte records: passes structural validation,
    // must fail at read time.
    let huge: u64 = 24 + (1u64 << 40);
    v["channel_groups"][0]["data_blocks"][0]["size"] = huge.into();
    let idx = MdfIndex::from_json(&v.to_string())?;
    let mut r = idx.open(FileRangeReader::new(path_str)?);
    assert!(
        r.values_f64("Value").is_err(),
        "forged huge block size must error, not abort"
    );

    // (d) byte_ranges_for_records edge cases: record_count == 0 must not
    // underflow, and start + count must not wrap.
    assert!(index.byte_ranges_for_records("Time", 0, 0)?.is_empty());
    assert!(index.byte_ranges_for_records("Time", u64::MAX, 2).is_err());
    assert!(index.byte_ranges_for_records("Time", 11, 0).is_err());

    std::fs::remove_file(&path)?;
    Ok(())
}

/// D7b: CachingRangeReader over a strict in-memory reader whose length is NOT
/// a multiple of the chunk size must serve reads near EOF correctly.
#[test]
fn caching_reader_serves_reads_near_eof() -> Result<(), MdfError> {
    let len = 250usize; // chunk_size = 100 → final chunk is 50 bytes
    let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    let mut cached = CachingRangeReader::with_chunk_size(SliceRangeReader::new(data.clone()), 100);

    // Read spanning from a full chunk into the short final chunk.
    assert_eq!(cached.read_range(190, 50)?, &data[190..240]);
    // Read right up to EOF.
    assert_eq!(cached.read_range(200, 50)?, &data[200..250]);
    // Read deeper into the final chunk than any earlier read.
    assert_eq!(cached.read_range(240, 10)?, &data[240..250]);
    // Whole-resource read.
    assert_eq!(cached.read_range(0, 250)?, data);
    // Reads extending past EOF must still error.
    assert!(cached.read_range(240, 20).is_err());
    assert!(cached.read_range(250, 1).is_err());

    // Same scenario when the *first* touch is a tiny read inside the final
    // chunk (exercises the clamp-and-retry path without prior state).
    let mut cached = CachingRangeReader::with_chunk_size(SliceRangeReader::new(data.clone()), 100);
    assert_eq!(cached.read_range(245, 5)?, &data[245..250]);
    assert_eq!(cached.read_range(230, 20)?, &data[230..250]);
    Ok(())
}

/// B7: a channel with cn_flags bit 0 ("all values invalid") must yield NaN
/// for every sample on the f64 fast path, even when the group has no
/// invalidation bytes.
#[test]
fn all_invalid_flag_yields_nan_on_f64_path() -> Result<(), MdfError> {
    let path = tmp_path("fix_index_all_invalid.mf4");
    let path_str = path.to_str().unwrap();

    let mut w = MdfWriter::new(path_str)?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let t = w.add_channel(&cg, None, |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Time".into());
    })?;
    w.set_time_channel(&t)?;
    w.add_channel(&cg, Some(&t), |c| {
        c.data_type = DataType::FloatLE;
        c.bit_count = 64;
        c.name = Some("Broken".into());
        c.flags = 0x1; // cn_flags bit 0: all values invalid
    })?;
    w.start_data_block_for_cg(&cg, 0)?;
    for r in 0..10 {
        w.write_record(
            &cg,
            &[
                DecodedValue::Float(r as f64 * 0.1),
                DecodedValue::Float(r as f64),
            ],
        )?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let index = MdfIndex::from_file(path_str)?;
    // The group must have NO invalidation bytes for this regression to bite.
    assert_eq!(index.groups()[0].invalidation_bytes, 0);

    let mut reader = index.open_file(path_str)?;

    // f64 fast path: previously returned the raw values.
    let broken = reader.values_f64("Broken")?;
    assert_eq!(broken.len(), 10);
    assert!(
        broken.iter().all(|v| v.is_nan()),
        "cn_flags bit 0 must force NaN for every sample, got {:?}",
        broken
    );

    // Consistency with the DecodedValue path (already handled bit 0).
    let decoded = reader.values("Broken")?;
    assert!(decoded.iter().all(|v| v.is_none()));

    // Sibling channel without the flag is unaffected.
    let times = reader.values_f64("Time")?;
    assert!(times.iter().all(|v| !v.is_nan()));

    std::fs::remove_file(&path)?;
    Ok(())
}

/// Sanity: an untampered index still round-trips through JSON validation.
#[test]
fn valid_index_json_round_trips() -> Result<(), MdfError> {
    let path = tmp_path("fix_index_roundtrip.mf4");
    let path_str = path.to_str().unwrap();
    write_basic_file(path_str, 25)?;

    let index = MdfIndex::from_file(path_str)?;
    let restored = MdfIndex::from_json(&index.to_json()?)?;
    let mut reader = restored.open_file(path_str)?;
    let vals = reader.values_f64("Value")?;
    assert_eq!(vals.len(), 25);
    assert_eq!(vals[7], 7.0);

    std::fs::remove_file(&path)?;
    Ok(())
}
