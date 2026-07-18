//! VLSD channel reads through the core index reader (`src/index.rs`).
//!
//! Covers the single-fragment round trip across the three read entry points
//! (bound reader, lazy source, explicit `FileRangeReader`), the multi-fragment
//! offset→fragment resolution with uneven `##SD` fragments, JSON persistence of
//! the captured fragment list, old-index compatibility (missing
//! `vlsd_data_blocks`), and Signal timestamp alignment.

use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::index::{FileRangeReader, MdfIndex};
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

fn tmp_path(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(name);
    if p.exists() {
        let _ = std::fs::remove_file(&p);
    }
    p
}

const RECORD_LEN: usize = 16; // 8 bytes time + 8 bytes VLSD offset slot

/// Write a file with a UTF-8 VLSD "Msg" channel next to a float "Time" channel.
fn write_vlsd_file(path: &str, payloads: &[&[u8]]) -> Result<(), MdfError> {
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
        c.channel_type = 1;
        c.data = 1;
        c.name = Some("Msg".into());
    })?;
    w.start_data_block_for_cg_raw(&cg, 0, RECORD_LEN as u32, 0)?;
    w.start_signal_data_block(&vlsd)?;

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
    Ok(())
}

/// Write a file whose VLSD "Image" channel spans multiple uneven ##SD
/// fragments (forcing a ##DL). Mirrors tests/vlsd_multi_fragment_offset_read.rs.
fn write_vlsd_uneven(path: &str) -> Result<Vec<Vec<u8>>, MdfError> {
    let payloads: Vec<Vec<u8>> = vec![
        vec![0xAA; 1 * 1024 * 1024], // 1 MB
        vec![0xBB; 5 * 1024 * 1024], // 5 MB (alone in its fragment)
        vec![0xCC; 256 * 1024],      // 256 KB
    ];

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
        c.data_type = DataType::ByteArray;
        c.bit_count = 64;
        c.channel_type = 1;
        c.name = Some("Image".into());
    })?;
    w.start_data_block_for_cg_raw(&cg, 0, RECORD_LEN as u32, 0)?;
    w.start_signal_data_block(&vlsd)?;

    let mut running: u64 = 0;
    for (i, p) in payloads.iter().enumerate() {
        let mut record = Vec::with_capacity(RECORD_LEN);
        record.extend_from_slice(&(i as f64 * 0.1).to_le_bytes());
        record.extend_from_slice(&running.to_le_bytes());
        w.write_raw_record(&cg, &record)?;
        w.write_signal_data(&vlsd, p)?;
        running = running.checked_add(4 + p.len() as u64).unwrap();
    }
    w.finish_signal_data_block(&vlsd)?;
    w.finish_data_block(&cg)?;
    w.finalize()?;
    Ok(payloads)
}

fn assert_string(v: &Option<DecodedValue>, want: &str) {
    match v {
        Some(DecodedValue::String(s)) => assert_eq!(s, want),
        other => panic!("expected string {:?}, got {:?}", want, other),
    }
}

/// (a) Single-fragment round trip across all three read entry points.
#[test]
fn vlsd_roundtrip_all_entry_points() -> Result<(), MdfError> {
    let path = tmp_path("index_vlsd_roundtrip.mf4");
    let path_str = path.to_str().unwrap();
    let payloads: [&[u8]; 3] = [b"alpha", b"bravo!", b"charlie"];
    let expected = ["alpha", "bravo!", "charlie"];
    write_vlsd_file(path_str, &payloads)?;

    let mut index = MdfIndex::from_file(path_str)?;

    // (1) bound reader via open_file (mmap)
    {
        let mut reader = index.open_file(path_str)?;
        let vals = reader.values("Msg")?;
        assert_eq!(vals.len(), 3);
        for (v, w) in vals.iter().zip(expected.iter()) {
            assert_string(v, w);
        }
    }

    // (2) lazy index.read via attached source
    {
        let sig = index.read("Msg")?;
        assert_eq!(sig.values.len(), 3);
        for (v, w) in sig.values.iter().zip(expected.iter()) {
            assert_string(v, w);
        }
    }

    // (3) explicit FileRangeReader through open(...) -> read_channel_values
    {
        let mut reader = index.open(FileRangeReader::new(path_str)?);
        let vals = reader.values("Msg")?;
        assert_eq!(vals.len(), 3);
        for (v, w) in vals.iter().zip(expected.iter()) {
            assert_string(v, w);
        }
    }

    // Ensure set_file / source path also works after a manual re-attach.
    index.set_file(path_str);
    assert_eq!(index.read("Msg")?.values.len(), 3);

    std::fs::remove_file(&path)?;
    Ok(())
}

/// (b) Multi-fragment: uneven ##SD fragments; exercise offset→fragment search.
#[test]
fn vlsd_multi_fragment_exact_bytes() -> Result<(), MdfError> {
    let path = tmp_path("index_vlsd_uneven.mf4");
    let path_str = path.to_str().unwrap();
    let payloads = write_vlsd_uneven(path_str)?;

    let index = MdfIndex::from_file(path_str)?;
    let mut reader = index.open_file(path_str)?;
    let vals = reader.values("Image")?;
    assert_eq!(vals.len(), payloads.len());
    for (i, v) in vals.iter().enumerate() {
        match v {
            Some(DecodedValue::ByteArray(b)) => {
                assert_eq!(b.len(), payloads[i].len(), "entry {} wrong length", i);
                assert_eq!(b, &payloads[i], "entry {} wrong bytes", i);
            }
            other => panic!("expected ByteArray at {}, got {:?}", i, other),
        }
    }

    std::fs::remove_file(&path)?;
    Ok(())
}

/// (c) JSON persistence: save -> load -> set_file -> read works, proving
/// vlsd_data_blocks serializes.
#[test]
fn vlsd_json_persistence() -> Result<(), MdfError> {
    let path = tmp_path("index_vlsd_persist.mf4");
    let path_str = path.to_str().unwrap();
    let json_path = tmp_path("index_vlsd_persist.idx.json");
    let json_str = json_path.to_str().unwrap();
    let payloads: [&[u8]; 3] = [b"one", b"two", b"three"];
    write_vlsd_file(path_str, &payloads)?;

    MdfIndex::from_file(path_str)?.save_to_file(json_str)?;

    let mut index = MdfIndex::load_from_file(json_str)?;
    index.set_file(path_str);
    let sig = index.read("Msg")?;
    assert_eq!(sig.values.len(), 3);
    assert_string(&sig.values[0], "one");
    assert_string(&sig.values[2], "three");

    std::fs::remove_file(&path)?;
    std::fs::remove_file(&json_path)?;
    Ok(())
}

/// (d) Old-index compat: a JSON index without `vlsd_data_blocks` must load
/// (serde default) and error with a clear rebuild message on VLSD reads, while
/// fixed-size channels still read fine.
#[test]
fn vlsd_missing_fragments_errors_with_rebuild_message() -> Result<(), MdfError> {
    let path = tmp_path("index_vlsd_oldindex.mf4");
    let path_str = path.to_str().unwrap();
    let payloads: [&[u8]; 3] = [b"aa", b"bb", b"cc"];
    write_vlsd_file(path_str, &payloads)?;

    let index = MdfIndex::from_file(path_str)?;
    let json = index.to_json()?;

    // Strip vlsd_data_blocks from every channel to emulate an older index.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    for group in v["channel_groups"].as_array_mut().unwrap() {
        for ch in group["channels"].as_array_mut().unwrap() {
            ch.as_object_mut().unwrap().remove("vlsd_data_blocks");
        }
    }
    let mut stripped = MdfIndex::from_json(&v.to_string())?;
    stripped.set_file(path_str);

    let err = stripped.read("Msg").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("rebuild the index"),
        "expected a rebuild hint, got: {msg}"
    );

    // The fixed-size Time channel still reads fine.
    let times = stripped.read("Time")?;
    assert_eq!(times.values.len(), 3);

    std::fs::remove_file(&path)?;
    Ok(())
}

/// (e) Signal alignment: read() timestamps length == values length == records.
#[test]
fn vlsd_signal_alignment() -> Result<(), MdfError> {
    let path = tmp_path("index_vlsd_align.mf4");
    let path_str = path.to_str().unwrap();
    let payloads: [&[u8]; 4] = [b"w", b"x", b"y", b"z"];
    write_vlsd_file(path_str, &payloads)?;

    let index = MdfIndex::from_file(path_str)?;
    let sig = index.read("Msg")?;
    assert_eq!(sig.values.len(), payloads.len());
    assert_eq!(sig.timestamps.len(), payloads.len());
    assert_eq!(sig.timestamps.len(), sig.values.len());

    std::fs::remove_file(&path)?;
    Ok(())
}
