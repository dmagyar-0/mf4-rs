/// Cross-compatibility tests verifying mf4-rs produces spec-compliant MDF files.
///
/// These tests validate behaviors discovered during comparison with asammdf 8.7.2.
/// They ensure mf4-rs files remain readable by other MDF tools and that mf4-rs
/// can read files with various data type widths and multi-group structures.
///
/// Run the Python cross-compatibility tests for full asammdf interop validation:
///   python tests/test_asammdf_interop.py
use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

/// Helper: create a temp file path with a unique name.
fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("mf4rs_compat_{}.mf4", name))
}

/// Helper: clean up a temp file if it exists.
fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

// ---------------------------------------------------------------------------
// Data type roundtrip tests
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_float64() -> Result<(), MdfError> {
    let path = temp_path("f64");
    cleanup(&path);

    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("pi".into());
        ch.bit_count = 64;
    })?;

    w.start_data_block_for_cg(&cg, 0)?;
    w.write_record(&cg, &[DecodedValue::Float(std::f64::consts::PI)])?;
    w.write_record(&cg, &[DecodedValue::Float(-1e-15)])?;
    w.write_record(&cg, &[DecodedValue::Float(1e15)])?;
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let vals = mdf.channel_groups()[0].channels()[0].values()?;
    assert_eq!(vals.len(), 3);
    match &vals[0] {
        Some(DecodedValue::Float(v)) => assert!((v - std::f64::consts::PI).abs() < 1e-15),
        other => panic!("expected Float, got {:?}", other),
    }
    match &vals[1] {
        Some(DecodedValue::Float(v)) => assert!((v - (-1e-15)).abs() < 1e-30),
        other => panic!("expected Float, got {:?}", other),
    }
    match &vals[2] {
        Some(DecodedValue::Float(v)) => assert!((v - 1e15).abs() < 1.0),
        other => panic!("expected Float, got {:?}", other),
    }

    cleanup(&path);
    Ok(())
}

#[test]
fn roundtrip_float32() -> Result<(), MdfError> {
    let path = temp_path("f32");
    cleanup(&path);

    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("val".into());
        ch.bit_count = 32;
    })?;

    w.start_data_block_for_cg(&cg, 0)?;
    w.write_record(&cg, &[DecodedValue::Float(3.14)])?;
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let vals = mdf.channel_groups()[0].channels()[0].values()?;
    assert_eq!(vals.len(), 1);
    // 32-bit float has ~7 digits of precision
    match &vals[0] {
        Some(DecodedValue::Float(v)) => assert!((v - 3.14).abs() < 1e-5),
        other => panic!("expected Float, got {:?}", other),
    }

    cleanup(&path);
    Ok(())
}

#[test]
fn roundtrip_signed_integers() -> Result<(), MdfError> {
    let path = temp_path("signed");
    cleanup(&path);

    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let ch1 = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::SignedIntegerLE;
        ch.name = Some("i32".into());
        ch.bit_count = 32;
    })?;
    w.add_channel(&cg, Some(&ch1), |ch| {
        ch.data_type = DataType::SignedIntegerLE;
        ch.name = Some("i64".into());
        ch.bit_count = 64;
    })?;

    w.start_data_block_for_cg(&cg, 0)?;
    w.write_record(&cg, &[
        DecodedValue::SignedInteger(-2_147_483_648),
        DecodedValue::SignedInteger(i64::MIN),
    ])?;
    w.write_record(&cg, &[
        DecodedValue::SignedInteger(2_147_483_647),
        DecodedValue::SignedInteger(i64::MAX),
    ])?;
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let chs = mdf.channel_groups()[0].channels();
    let v32 = chs[0].values()?;
    let v64 = chs[1].values()?;
    assert_eq!(v32.len(), 2);
    match &v32[0] {
        Some(DecodedValue::SignedInteger(v)) => assert_eq!(*v, -2_147_483_648),
        other => panic!("expected SignedInteger, got {:?}", other),
    }
    match &v32[1] {
        Some(DecodedValue::SignedInteger(v)) => assert_eq!(*v, 2_147_483_647),
        other => panic!("expected SignedInteger, got {:?}", other),
    }
    match &v64[0] {
        Some(DecodedValue::SignedInteger(v)) => assert_eq!(*v, i64::MIN),
        other => panic!("expected SignedInteger, got {:?}", other),
    }
    match &v64[1] {
        Some(DecodedValue::SignedInteger(v)) => assert_eq!(*v, i64::MAX),
        other => panic!("expected SignedInteger, got {:?}", other),
    }

    cleanup(&path);
    Ok(())
}

#[test]
fn roundtrip_unsigned_integers() -> Result<(), MdfError> {
    let path = temp_path("unsigned");
    cleanup(&path);

    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let ch1 = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("u8".into());
        ch.bit_count = 8;
    })?;
    let ch2 = w.add_channel(&cg, Some(&ch1), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("u16".into());
        ch.bit_count = 16;
    })?;
    let ch3 = w.add_channel(&cg, Some(&ch2), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("u32".into());
        ch.bit_count = 32;
    })?;
    w.add_channel(&cg, Some(&ch3), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("u64".into());
        ch.bit_count = 64;
    })?;

    w.start_data_block_for_cg(&cg, 0)?;
    w.write_record(&cg, &[
        DecodedValue::UnsignedInteger(0),
        DecodedValue::UnsignedInteger(0),
        DecodedValue::UnsignedInteger(0),
        DecodedValue::UnsignedInteger(0),
    ])?;
    w.write_record(&cg, &[
        DecodedValue::UnsignedInteger(255),
        DecodedValue::UnsignedInteger(65535),
        DecodedValue::UnsignedInteger(u32::MAX as u64),
        DecodedValue::UnsignedInteger(u64::MAX),
    ])?;
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let chs = mdf.channel_groups()[0].channels();
    for (i, expected_max) in [(0, 255u64), (1, 65535), (2, u32::MAX as u64), (3, u64::MAX)] {
        let vals = chs[i].values()?;
        assert_eq!(vals.len(), 2, "channel {} should have 2 values", i);
        match &vals[0] {
            Some(DecodedValue::UnsignedInteger(v)) => assert_eq!(*v, 0),
            other => panic!("ch{} rec0: expected UnsignedInteger(0), got {:?}", i, other),
        }
        match &vals[1] {
            Some(DecodedValue::UnsignedInteger(v)) => assert_eq!(*v, expected_max),
            other => panic!("ch{} rec1: expected UnsignedInteger({}), got {:?}", i, expected_max, other),
        }
    }

    cleanup(&path);
    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-group structure
// ---------------------------------------------------------------------------

#[test]
fn multi_group_with_master_channels() -> Result<(), MdfError> {
    let path = temp_path("multi_group");
    cleanup(&path);

    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;

    // Group 1: time + float
    let cg1 = w.add_channel_group(None, |_| {})?;
    let t1 = w.add_channel(&cg1, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".into());
        ch.bit_count = 64;
    })?;
    w.set_time_channel(&t1)?;
    w.add_channel(&cg1, Some(&t1), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Temperature".into());
        ch.bit_count = 64;
    })?;

    // Group 2: time + unsigned int
    let cg2 = w.add_channel_group(None, |_| {})?;
    let t2 = w.add_channel(&cg2, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".into());
        ch.bit_count = 64;
    })?;
    w.set_time_channel(&t2)?;
    w.add_channel(&cg2, Some(&t2), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Counter".into());
        ch.bit_count = 32;
    })?;

    // Write data to group 1
    w.start_data_block_for_cg(&cg1, 0)?;
    for i in 0..10 {
        w.write_record(&cg1, &[
            DecodedValue::Float(i as f64 * 0.1),
            DecodedValue::Float(20.0 + i as f64),
        ])?;
    }
    w.finish_data_block(&cg1)?;

    // Write data to group 2
    w.start_data_block_for_cg(&cg2, 0)?;
    for i in 0..5 {
        w.write_record(&cg2, &[
            DecodedValue::Float(i as f64 * 0.2),
            DecodedValue::UnsignedInteger(i as u64 * 100),
        ])?;
    }
    w.finish_data_block(&cg2)?;

    w.finalize()?;

    // Read back and verify
    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let groups = mdf.channel_groups();
    assert_eq!(groups.len(), 2);

    // Group 1: 2 channels, 10 records
    assert_eq!(groups[0].channels().len(), 2);
    let g1_time = groups[0].channels()[0].values()?;
    let g1_temp = groups[0].channels()[1].values()?;
    assert_eq!(g1_time.len(), 10);
    assert_eq!(g1_temp.len(), 10);
    match &g1_time[0] {
        Some(DecodedValue::Float(v)) => assert!(*v < 0.001),
        other => panic!("expected Float(0.0), got {:?}", other),
    }
    match &g1_temp[9] {
        Some(DecodedValue::Float(v)) => assert!((v - 29.0).abs() < 0.001),
        other => panic!("expected Float(29.0), got {:?}", other),
    }

    // Group 2: 2 channels, 5 records
    assert_eq!(groups[1].channels().len(), 2);
    let g2_counter = groups[1].channels()[1].values()?;
    assert_eq!(g2_counter.len(), 5);
    match &g2_counter[4] {
        Some(DecodedValue::UnsignedInteger(v)) => assert_eq!(*v, 400),
        other => panic!("expected UnsignedInteger(400), got {:?}", other),
    }

    cleanup(&path);
    Ok(())
}

// ---------------------------------------------------------------------------
// Data block splitting
// ---------------------------------------------------------------------------

#[test]
fn data_block_splitting_roundtrip() -> Result<(), MdfError> {
    let path = temp_path("splitting");
    cleanup(&path);

    // 4 x f32 channels = 16 bytes per record
    // MAX_DT_BLOCK_SIZE = 4MB = 4,194,304 bytes
    // Need > 262,144 records to trigger split
    let n = 300_000usize;

    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let ch1 = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("a".into());
        ch.bit_count = 32;
    })?;
    let ch2 = w.add_channel(&cg, Some(&ch1), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("b".into());
        ch.bit_count = 32;
    })?;
    let ch3 = w.add_channel(&cg, Some(&ch2), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("c".into());
        ch.bit_count = 32;
    })?;
    w.add_channel(&cg, Some(&ch3), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("d".into());
        ch.bit_count = 32;
    })?;

    w.start_data_block_for_cg(&cg, 0)?;
    for i in 0..n {
        w.write_record(&cg, &[
            DecodedValue::Float(i as f64),
            DecodedValue::Float(i as f64 * 2.0),
            DecodedValue::Float(i as f64 * 3.0),
            DecodedValue::Float(i as f64 * 4.0),
        ])?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()?;

    // Verify the file has a DL block (data was split)
    let file_bytes = std::fs::read(&path)?;
    let has_dl = file_bytes.windows(4).any(|w| w == b"##DL");
    assert!(has_dl, "expected ##DL block for data block splitting");

    // Verify all values read back correctly
    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let chs = mdf.channel_groups()[0].channels();
    assert_eq!(chs.len(), 4);
    let vals_a = chs[0].values()?;
    assert_eq!(vals_a.len(), n);

    // Check first and last values
    match &vals_a[0] {
        Some(DecodedValue::Float(v)) => assert!(*v < 0.001),
        other => panic!("expected Float(0), got {:?}", other),
    }
    match &vals_a[n - 1] {
        Some(DecodedValue::Float(v)) => assert!((*v - (n - 1) as f64).abs() < 1.0),
        other => panic!("expected Float({}), got {:?}", n - 1, other),
    }

    // Check channel d (4x multiplier)
    let vals_d = chs[3].values()?;
    match &vals_d[100] {
        Some(DecodedValue::Float(v)) => assert!((*v - 400.0).abs() < 1.0),
        other => panic!("expected Float(400), got {:?}", other),
    }

    cleanup(&path);
    Ok(())
}

// ---------------------------------------------------------------------------
// Value-to-text conversion
// ---------------------------------------------------------------------------

#[test]
fn value_to_text_conversion_roundtrip() -> Result<(), MdfError> {
    let path = temp_path("v2t_conv");
    cleanup(&path);

    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let time_ch = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".into());
        ch.bit_count = 64;
    })?;
    w.set_time_channel(&time_ch)?;
    let status_ch = w.add_channel(&cg, Some(&time_ch), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Status".into());
        ch.bit_count = 32;
    })?;

    w.add_value_to_text_conversion(
        &[(0, "OK"), (1, "WARN"), (2, "ERROR")],
        "UNKNOWN",
        Some(&status_ch),
    )?;

    w.start_data_block_for_cg(&cg, 0)?;
    w.write_record(&cg, &[DecodedValue::Float(0.0), DecodedValue::UnsignedInteger(0)])?;
    w.write_record(&cg, &[DecodedValue::Float(1.0), DecodedValue::UnsignedInteger(1)])?;
    w.write_record(&cg, &[DecodedValue::Float(2.0), DecodedValue::UnsignedInteger(2)])?;
    w.write_record(&cg, &[DecodedValue::Float(3.0), DecodedValue::UnsignedInteger(99)])?;
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let chs = mdf.channel_groups()[0].channels();
    let status_vals = chs[1].values()?;
    assert_eq!(status_vals.len(), 4);

    // Conversion should transform integers to strings
    match &status_vals[0] {
        Some(DecodedValue::String(s)) => assert_eq!(s, "OK"),
        other => panic!("expected String(OK), got {:?}", other),
    }
    match &status_vals[1] {
        Some(DecodedValue::String(s)) => assert_eq!(s, "WARN"),
        other => panic!("expected String(WARN), got {:?}", other),
    }
    match &status_vals[2] {
        Some(DecodedValue::String(s)) => assert_eq!(s, "ERROR"),
        other => panic!("expected String(ERROR), got {:?}", other),
    }
    match &status_vals[3] {
        Some(DecodedValue::String(s)) => assert_eq!(s, "UNKNOWN"),
        other => panic!("expected String(UNKNOWN), got {:?}", other),
    }

    cleanup(&path);
    Ok(())
}

// ---------------------------------------------------------------------------
// Performance regression test
// ---------------------------------------------------------------------------

#[test]
fn write_100k_records_performance() -> Result<(), MdfError> {
    let path = temp_path("perf");
    cleanup(&path);

    let n = 100_000usize;

    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let t = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".into());
        ch.bit_count = 64;
    })?;
    w.set_time_channel(&t)?;
    let a = w.add_channel(&cg, Some(&t), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("A".into());
        ch.bit_count = 64;
    })?;
    let b = w.add_channel(&cg, Some(&a), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("B".into());
        ch.bit_count = 64;
    })?;
    w.add_channel(&cg, Some(&b), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("C".into());
        ch.bit_count = 64;
    })?;

    let start = std::time::Instant::now();
    w.start_data_block_for_cg(&cg, 0)?;
    for i in 0..n {
        let v = i as f64 * 0.001;
        w.write_record(&cg, &[
            DecodedValue::Float(v),
            DecodedValue::Float(v * 2.0),
            DecodedValue::Float(v * 3.0),
            DecodedValue::Float(v * 4.0),
        ])?;
    }
    w.finish_data_block(&cg)?;
    w.finalize()?;
    let write_time = start.elapsed();

    // Read back
    let start = std::time::Instant::now();
    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let mut total = 0;
    for group in mdf.channel_groups() {
        for channel in group.channels() {
            total += channel.values()?.len();
        }
    }
    let read_time = start.elapsed();

    assert_eq!(total, n * 4);

    // Performance assertions (generous limits for CI - debug builds are slower)
    // In release mode: write ~0.008s, read ~0.006s
    // In debug mode: can be 10-20x slower
    assert!(
        write_time.as_secs() < 10,
        "write took {:?} - performance regression", write_time,
    );
    assert!(
        read_time.as_secs() < 10,
        "read took {:?} - performance regression", read_time,
    );

    eprintln!(
        "Performance: write={:.3}s, read={:.3}s ({} records, 4 x f64 channels)",
        write_time.as_secs_f64(),
        read_time.as_secs_f64(),
        n,
    );

    cleanup(&path);
    Ok(())
}

// ---------------------------------------------------------------------------
// File structure validation
// ---------------------------------------------------------------------------

#[test]
fn file_has_correct_identification() -> Result<(), MdfError> {
    let path = temp_path("ident");
    cleanup(&path);

    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
    })?;
    w.finalize()?;

    let bytes = std::fs::read(&path)?;
    // ID block: first 8 bytes = "MDF     "
    assert_eq!(&bytes[0..8], b"MDF     ");
    // Format: "4.10    "
    let fmt = std::str::from_utf8(&bytes[8..16]).unwrap().trim_end_matches('\0');
    assert_eq!(fmt.trim(), "4.10");
    // Program: "mf4-rs  "
    let prog = std::str::from_utf8(&bytes[16..24]).unwrap().trim_end_matches('\0');
    assert_eq!(prog.trim(), "mf4-rs");

    cleanup(&path);
    Ok(())
}

#[test]
fn master_channel_has_correct_type_and_sync() -> Result<(), MdfError> {
    let path = temp_path("master");
    cleanup(&path);

    let mut w = MdfWriter::new(path.to_str().unwrap())?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(None, |_| {})?;
    let time_ch = w.add_channel(&cg, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".into());
        ch.bit_count = 64;
    })?;
    w.set_time_channel(&time_ch)?;
    w.add_channel(&cg, Some(&time_ch), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Data".into());
        ch.bit_count = 64;
    })?;
    w.finalize()?;

    let mdf = MDF::from_file(path.to_str().unwrap())?;
    let chs = mdf.channel_groups()[0].channels();
    assert_eq!(chs.len(), 2);
    // Time channel should have channel_type=2 (Master) and sync_type=1 (Time)
    assert_eq!(chs[0].block().channel_type, 2, "Time channel should be master (type=2)");
    assert_eq!(chs[0].block().sync_type, 1, "Time channel should have sync_type=1");
    // Data channel should have channel_type=0 (Value)
    assert_eq!(chs[1].block().channel_type, 0, "Data channel should be value (type=0)");

    cleanup(&path);
    Ok(())
}
