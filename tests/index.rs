use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::index::MdfIndex;
use mf4_rs::api::mdf::Mdf;
use mf4_rs::error::MdfError;
use std::fs;

#[test]
fn test_index_roundtrip() -> Result<(), MdfError> {
    let mdf_path = std::env::temp_dir().join("index_test.mf4");
    let index_path = std::env::temp_dir().join("index_test.json");

    // Clean up any existing files
    if mdf_path.exists() { fs::remove_file(&mdf_path)?; }
    if index_path.exists() { fs::remove_file(&index_path)?; }

    // Create a test MDF file
    let mut writer = MdfWriter::new(mdf_path.to_str().unwrap())?;
    writer.init_mdf_file()?;

    let cg_id = writer.add_channel_group_with(None, |_| {})?;

    let time_ch_id = writer.add_channel_with(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".to_string());
        ch.bit_count = 64;
    })?;
    writer.set_time_channel(&time_ch_id)?;

    writer.add_channel_with(&cg_id, Some(&time_ch_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Value".to_string());
        ch.bit_count = 32;
    })?;

    writer.start_data_block_for_cg(&cg_id, 0)?;

    // Write some test data
    let test_values = vec![
        (0.0, 100u64),
        (0.1, 200u64),
        (0.2, 300u64),
    ];

    for (time, value) in &test_values {
        writer.write_record(&cg_id, &[
            DecodedValue::Float(*time),
            DecodedValue::UnsignedInteger(*value),
        ])?;
    }

    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Create and save index
    let index = MdfIndex::from_file(mdf_path.to_str().unwrap())?;
    index.save(index_path.to_str().unwrap())?;

    // Load index and verify structure
    let loaded_index = MdfIndex::load(index_path.to_str().unwrap())?;

    assert_eq!(loaded_index.channel_groups.len(), 1);

    let group = &loaded_index.channel_groups[0];
    assert_eq!(group.channels.len(), 2);
    assert_eq!(group.record_count, test_values.len() as u64);

    // Check channel metadata via name-based navigation
    let time_channel = group.channel("Time").unwrap();
    assert_eq!(time_channel.name, Some("Time".to_string()));
    assert_eq!(time_channel.data_type, DataType::FloatLE);
    assert!(time_channel.is_master());

    let value_channel = group.channel("Value").unwrap();
    assert_eq!(value_channel.name, Some("Value".to_string()));
    assert_eq!(value_channel.data_type, DataType::UnsignedIntegerLE);
    assert!(!value_channel.is_master());

    // Verify data blocks are indexed
    assert!(!group.data_blocks.is_empty());
    let data_block = &group.data_blocks[0];
    assert!(!data_block.is_compressed);
    assert!(data_block.size > 0);

    // Test reading channel values by name via a bound reader
    let mut data = loaded_index.open_file(mdf_path.to_str().unwrap())?;
    let time_values = data.decoded_values("Time", None)?;
    let value_values = data.decoded_values("Value", None)?;

    assert_eq!(time_values.len(), test_values.len());
    assert_eq!(value_values.len(), test_values.len());

    // Verify the actual values
    for (i, (expected_time, expected_value)) in test_values.iter().enumerate() {
        if let Some(DecodedValue::Float(actual_time)) = time_values[i] {
            assert!((actual_time - expected_time).abs() < 1e-10);
        } else {
            panic!("Expected Float value for time channel");
        }

        if let Some(DecodedValue::UnsignedInteger(actual_value)) = value_values[i] {
            assert_eq!(actual_value, *expected_value);
        } else {
            panic!("Expected UnsignedInteger value for value channel");
        }
    }

    // Clean up
    fs::remove_file(mdf_path)?;
    fs::remove_file(index_path)?;

    Ok(())
}

#[test]
fn test_index_vs_direct_read() -> Result<(), MdfError> {
    let mdf_path = std::env::temp_dir().join("comparison_test.mf4");
    let index_path = std::env::temp_dir().join("comparison_index.json");

    // Clean up
    let _ = fs::remove_file(&mdf_path);
    let _ = fs::remove_file(&index_path);

    // Create test file
    let mut writer = MdfWriter::new(mdf_path.to_str().unwrap())?;
    writer.init_mdf_file()?;

    let cg_id = writer.add_channel_group_with(None, |_| {})?;
    writer.add_channel_with(&cg_id, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("TestChannel".to_string());
        ch.bit_count = 32;
    })?;

    writer.start_data_block_for_cg(&cg_id, 0)?;

    let test_data = vec![42u64, 123u64, 456u64, 789u64];
    for &value in &test_data {
        writer.write_record(&cg_id, &[DecodedValue::UnsignedInteger(value)])?;
    }

    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Read via direct MDF parsing
    let mdf = Mdf::from_file(mdf_path.to_str().unwrap())?;
    let direct_values = mdf.find_channel("TestChannel", None).unwrap().unwrap().decoded_values()?;

    // Read via index (bound reader, by name)
    let index = MdfIndex::from_file(mdf_path.to_str().unwrap())?;
    let mut data = index.open_file(mdf_path.to_str().unwrap())?;
    let indexed_values = data.decoded_values("TestChannel", None)?;

    // Compare results
    assert_eq!(direct_values.len(), indexed_values.len());
    assert_eq!(direct_values.len(), test_data.len());

    for i in 0..test_data.len() {
        assert_eq!(direct_values[i], indexed_values[i]);

        if let Some(DecodedValue::UnsignedInteger(value)) = indexed_values[i] {
            assert_eq!(value, test_data[i]);
        } else {
            panic!("Expected UnsignedInteger");
        }
    }

    // Clean up
    let _ = fs::remove_file(mdf_path);
    let _ = fs::remove_file(index_path);

    Ok(())
}

#[test]
fn test_index_metadata() -> Result<(), MdfError> {
    let mdf_path = std::env::temp_dir().join("metadata_test.mf4");

    if mdf_path.exists() { fs::remove_file(&mdf_path)?; }

    // Create file with specific metadata
    let mut writer = MdfWriter::new(mdf_path.to_str().unwrap())?;
    writer.init_mdf_file()?;

    let cg_id = writer.add_channel_group_with(None, |_| {})?;
    let float_ch_id = writer.add_channel_with(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("TestFloat".to_string());
        ch.bit_count = 32;
    })?;

    writer.add_channel_with(&cg_id, Some(&float_ch_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("TestInt".to_string());
        ch.bit_count = 16;
    })?;

    writer.start_data_block_for_cg(&cg_id, 0)?;
    writer.write_record(&cg_id, &[
        DecodedValue::Float(3.5),
        DecodedValue::UnsignedInteger(42),
    ])?;
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Create index and verify metadata via the name-based navigation
    let index = MdfIndex::from_file(mdf_path.to_str().unwrap())?;

    let groups = &index.channel_groups;
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, None); // unnamed group
    assert_eq!(groups[0].channels.len(), 2);

    let channel_names = groups[0].channel_names();
    assert_eq!(channel_names, vec!["TestFloat", "TestInt"]);

    // Channel info retrieval by name
    let float_info = index.indexed_channel("TestFloat", None).unwrap();
    assert_eq!(float_info.name, Some("TestFloat".to_string()));
    assert_eq!(float_info.data_type, DataType::FloatLE);
    assert_eq!(float_info.bit_count, 32);

    let int_info = index.indexed_channel("TestInt", None).unwrap();
    assert_eq!(int_info.name, Some("TestInt".to_string()));
    assert_eq!(int_info.data_type, DataType::UnsignedIntegerLE);
    assert_eq!(int_info.bit_count, 16);

    fs::remove_file(mdf_path)?;
    Ok(())
}

#[test]
fn test_byte_ranges() -> Result<(), MdfError> {
    let mdf_path = std::env::temp_dir().join("byte_ranges_test.mf4");

    let _ = fs::remove_file(&mdf_path);

    // Create test file with known structure
    let mut writer = MdfWriter::new(mdf_path.to_str().unwrap())?;
    writer.init_mdf_file()?;

    let cg_id = writer.add_channel_group_with(None, |_| {})?;

    // Create channels with predictable sizes
    let ch1_id = writer.add_channel_with(&cg_id, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Ch1".to_string());
        ch.bit_count = 32; // 4 bytes
    })?;

    writer.add_channel_with(&cg_id, Some(&ch1_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Ch2".to_string());
        ch.bit_count = 16; // 2 bytes
    })?;

    writer.start_data_block_for_cg(&cg_id, 0)?;

    // Write exactly 5 records
    for i in 0..5 {
        writer.write_record(&cg_id, &[
            DecodedValue::UnsignedInteger(i * 100),
            DecodedValue::UnsignedInteger(i * 10),
        ])?;
    }

    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Create index and test byte ranges by name
    let index = MdfIndex::from_file(mdf_path.to_str().unwrap())?;

    // Byte ranges for all data of a channel
    let ranges = index.byte_ranges("Ch1", None)?;
    assert!(!ranges.is_empty(), "Should have at least one byte range");

    let total_bytes: u64 = ranges.iter().map(|(_, len)| len).sum();
    assert!(total_bytes > 0, "Should have positive total bytes");

    // Partial record reading
    let partial_ranges = index.byte_ranges_for_records("Ch1", 1, 3, None)?;
    assert!(!partial_ranges.is_empty(), "Should have byte ranges for partial records");

    let partial_total: u64 = partial_ranges.iter().map(|(_, len)| len).sum();
    assert!(partial_total <= total_bytes, "Partial range should be <= total range");

    // Error conditions
    assert!(index.byte_ranges("DoesNotExist", None).is_err(), "Unknown channel should error");
    assert!(index.byte_ranges_for_records("Ch1", 10, 1, None).is_err(), "Out of range records should error");

    let _ = fs::remove_file(mdf_path);
    Ok(())
}

#[test]
fn test_byte_ranges_accuracy() -> Result<(), MdfError> {
    let mdf_path = std::env::temp_dir().join("byte_accuracy_test.mf4");

    let _ = fs::remove_file(&mdf_path);

    // Create simple test file
    let mut writer = MdfWriter::new(mdf_path.to_str().unwrap())?;
    writer.init_mdf_file()?;

    let cg_id = writer.add_channel_group_with(None, |_| {})?;
    writer.add_channel_with(&cg_id, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Val".to_string());
        ch.bit_count = 32;
    })?;

    writer.start_data_block_for_cg(&cg_id, 0)?;
    writer.write_record(&cg_id, &[DecodedValue::UnsignedInteger(0x12345678)])?;
    writer.write_record(&cg_id, &[DecodedValue::UnsignedInteger(0xABCDEF00)])?;
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Create index and read values via the bound reader
    let index = MdfIndex::from_file(mdf_path.to_str().unwrap())?;
    let mut data = index.open_file(mdf_path.to_str().unwrap())?;
    let values = data.decoded_values("Val", None)?;

    // Verify the values are what we expect
    assert_eq!(values.len(), 2);
    if let Some(DecodedValue::UnsignedInteger(val)) = values[0] {
        assert_eq!(val, 0x12345678);
    } else {
        panic!("Expected UnsignedInteger");
    }

    if let Some(DecodedValue::UnsignedInteger(val)) = values[1] {
        assert_eq!(val, 0xABCDEF00);
    } else {
        panic!("Expected UnsignedInteger");
    }

    // Byte ranges should make sense
    let ranges = index.byte_ranges("Val", None)?;
    let total_bytes: u64 = ranges.iter().map(|(_, len)| len).sum();
    assert!(total_bytes >= 8, "Should have at least 8 bytes for 2x32-bit values");

    let _ = fs::remove_file(mdf_path);
    Ok(())
}

#[test]
fn test_name_based_lookup() -> Result<(), MdfError> {
    let mdf_path = std::env::temp_dir().join("name_lookup_test.mf4");

    let _ = fs::remove_file(&mdf_path);

    // Create test file with named channels
    let mut writer = MdfWriter::new(mdf_path.to_str().unwrap())?;
    writer.init_mdf_file()?;

    let cg_id = writer.add_channel_group_with(None, |_| {})?;

    // Create master channel
    let temp_ch_id = writer.add_channel_with(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Temperature".to_string());
        ch.bit_count = 32;
    })?;
    writer.set_time_channel(&temp_ch_id)?;

    // Create data channel with master as parent
    writer.add_channel_with(&cg_id, Some(&temp_ch_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Speed".to_string());
        ch.bit_count = 16;
    })?;

    writer.start_data_block_for_cg(&cg_id, 0)?;
    writer.write_record(&cg_id, &[
        DecodedValue::Float(25.5),           // Temperature
        DecodedValue::UnsignedInteger(120),  // Speed
    ])?;
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Create index and test name-based lookups
    let index = MdfIndex::from_file(mdf_path.to_str().unwrap())?;

    // Locating channels by name
    assert_eq!(index.indexed_channel("Temperature", None).map(|c| c.byte_offset), Some(0));
    assert!(index.indexed_channel("Speed", None).is_some());
    assert!(index.indexed_channel("NonExistent", None).is_none());

    // Group-scoped lookup
    assert!(index.channel_groups[0].channel("Temperature").is_some());
    assert!(index.channel_groups[0].channel("Speed").is_some());
    assert!(index.channel_groups[0].channel("NonExistent").is_none());

    // Channel info by name
    let channel_info = index.indexed_channel("Temperature", None).unwrap();
    assert_eq!(channel_info.name, Some("Temperature".to_string()));
    assert_eq!(channel_info.data_type, DataType::FloatLE);

    // Reading channels by name through the bound reader
    let mut data = index.open_file(mdf_path.to_str().unwrap())?;
    let temp_values = data.decoded_values("Temperature", None)?;
    assert_eq!(temp_values.len(), 1);
    if let Some(DecodedValue::Float(temp)) = temp_values[0] {
        assert!((temp - 25.5).abs() < 0.001);
    } else {
        panic!("Expected Float value");
    }

    let speed_values = data.decoded_values("Speed", None)?;
    assert_eq!(speed_values.len(), 1);
    if let Some(DecodedValue::UnsignedInteger(speed)) = speed_values[0] {
        assert_eq!(speed, 120);
    } else {
        panic!("Expected UnsignedInteger value");
    }

    // Error for non-existent channel
    assert!(data.decoded_values("NonExistent", None).is_err());

    // Byte ranges by name
    let ranges = index.byte_ranges("Temperature", None)?;
    assert!(!ranges.is_empty());

    let _ = fs::remove_file(mdf_path);
    Ok(())
}

#[test]
fn test_multiple_channels_same_name() -> Result<(), MdfError> {
    let mdf_path = std::env::temp_dir().join("duplicate_names_test.mf4");

    let _ = fs::remove_file(&mdf_path);

    // Create test file with duplicate channel names across groups
    let mut writer = MdfWriter::new(mdf_path.to_str().unwrap())?;
    writer.init_mdf_file()?;

    // First group
    let cg1_id = writer.add_channel_group_with(None, |_| {})?;
    writer.add_channel_with(&cg1_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Temperature".to_string());
        ch.bit_count = 32;
    })?;

    // Second group
    let cg2_id = writer.add_channel_group_with(Some(&cg1_id), |_| {})?;
    writer.add_channel_with(&cg2_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Temperature".to_string());
        ch.bit_count = 32;
    })?;

    writer.start_data_block_for_cg(&cg1_id, 0)?;
    writer.write_record(&cg1_id, &[DecodedValue::Float(25.0)])?;
    writer.finish_data_block(&cg1_id)?;

    writer.start_data_block_for_cg(&cg2_id, 0)?;
    writer.write_record(&cg2_id, &[DecodedValue::Float(30.0)])?;
    writer.finish_data_block(&cg2_id)?;

    writer.finalize()?;

    // Test finding all channels with same name
    let index = MdfIndex::from_file(mdf_path.to_str().unwrap())?;

    let holders: Vec<_> = index
        .groups()?
        .into_iter()
        .filter(|g| g.channel("Temperature").is_some())
        .collect();
    assert_eq!(holders.len(), 2);

    // index.channel() returns the first match
    assert!(index.channel("Temperature", None)?.is_some());

    let _ = fs::remove_file(mdf_path);
    Ok(())
}

#[test]
fn test_signal_and_lazy_source() -> Result<(), MdfError> {
    let mdf_path = std::env::temp_dir().join("signal_source_test.mf4");
    let index_path = std::env::temp_dir().join("signal_source_test.json");
    let _ = fs::remove_file(&mdf_path);
    let _ = fs::remove_file(&index_path);

    // Master (Time) + one data channel.
    let mut writer = MdfWriter::new(mdf_path.to_str().unwrap())?;
    writer.init_mdf_file()?;
    let cg_id = writer.add_channel_group_with(None, |_| {})?;
    let t_id = writer.add_channel_with(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".to_string());
        ch.bit_count = 64;
    })?;
    writer.set_time_channel(&t_id)?;
    writer.add_channel_with(&cg_id, Some(&t_id), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Speed".to_string());
        ch.bit_count = 64;
    })?;
    writer.start_data_block_for_cg(&cg_id, 0)?;
    for i in 0..10 {
        writer.write_record(&cg_id, &[
            DecodedValue::Float(i as f64 * 0.1),
            DecodedValue::Float(i as f64 * 2.0),
        ])?;
    }
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // from_file remembers the source; read() is lazy and pairs values with the
    // master/time axis.
    let index = MdfIndex::from_file(mdf_path.to_str().unwrap())?;
    assert!(index.source().is_some(), "from_file should attach a source");

    let speed = index.read("Speed", None)?;
    assert_eq!(speed.name, "Speed");
    assert_eq!(speed.values.len(), 10);
    assert_eq!(speed.timestamps.len(), 10, "Speed should carry the Time axis");
    assert!((speed.timestamps[5] - 0.5).abs() < 1e-9);
    if let Some(DecodedValue::Float(v)) = speed.values[5] {
        assert!((v - 10.0).abs() < 1e-9);
    } else {
        panic!("expected Float value");
    }

    // The master channel indexes itself: no separate timestamps.
    let time = index.read("Time", None)?;
    assert_eq!(time.values.len(), 10);
    assert!(!time.has_timestamps(), "master channel has no separate timestamps");

    // Source is not persisted; re-attach after load, then read lazily.
    index.save(index_path.to_str().unwrap())?;
    let mut loaded = MdfIndex::load(index_path.to_str().unwrap())?;
    assert!(loaded.source().is_none(), "source must not be serialized");
    assert!(loaded.read("Speed", None).is_err(), "read without a source should error");
    loaded.set_source(mdf_path.to_str().unwrap());
    let speed2 = loaded.read("Speed", None)?;
    assert_eq!(speed2.values_f64(), speed.values_f64());

    let _ = fs::remove_file(mdf_path);
    let _ = fs::remove_file(index_path);
    Ok(())
}

#[test]
fn test_channel_group_name_lookup() -> Result<(), MdfError> {
    let mdf_path = std::env::temp_dir().join("group_name_test.mf4");

    let _ = fs::remove_file(&mdf_path);

    let mut writer = MdfWriter::new(mdf_path.to_str().unwrap())?;
    writer.init_mdf_file()?;

    let _cg1_id = writer.add_channel_group_with(None, |_| {})?;
    writer.finalize()?;

    let index = MdfIndex::from_file(mdf_path.to_str().unwrap())?;

    // Unnamed groups are not found by name
    assert!(index.group("SomeGroup").unwrap().is_none());

    let _ = fs::remove_file(mdf_path);
    Ok(())
}

/// A time axis that resets (two recordings back to back) is refused by both
/// readers with the same error, instead of each silently selecting a
/// different half of the data; a non-decreasing axis with repeated
/// timestamps still selects identical records.
#[test]
fn non_monotonic_time_axis_is_refused_by_mdf_and_index() -> Result<(), mf4_rs::error::MdfError> {
    use mf4_rs::error::MdfError;
    use mf4_rs::parsing::decoder::DecodedValue;
    use mf4_rs::{Mdf, MdfIndex, MdfWriter, ReadOptions};

    let dir = tempfile::tempdir()?;
    let write = |name: &str, times: &[f64]| -> Result<String, MdfError> {
        let path = dir.path().join(name).to_str().unwrap().to_string();
        let mut w = MdfWriter::new(&path)?;
        w.init_mdf_file()?;
        let cg = w.add_channel_group(Some("G"))?;
        w.add_time_channel(&cg, "t")?;
        w.add_float_channel(&cg, "X")?;
        w.start_data_block(&cg)?;
        for (k, t) in times.iter().enumerate() {
            w.write_record(&cg, &[DecodedValue::Float(*t), DecodedValue::Float(k as f64)])?;
        }
        w.finish_data_block(&cg)?;
        w.finalize()?;
        Ok(path)
    };

    let reset: Vec<f64> = (0..2000).map(|i| (i % 1000) as f64 * 0.01).collect();
    let path = write("reset.mf4", &reset)?;
    let (mdf, index) = (Mdf::from_file(&path)?, MdfIndex::from_file(&path)?);
    let window = ReadOptions::new().time(Some(2.0), Some(3.0));
    for result in [mdf.read("X", &window).map(|_| ()), index.read("X", &window).map(|_| ()), index.values("X", &window).map(|_| ())] {
        match result {
            Err(MdfError::InvalidArgument(msg)) => assert!(msg.contains("not monotonic"), "{msg}"),
            other => panic!("expected a non-monotonic error, got {other:?}"),
        }
    }
    // Record windows do not depend on the time axis.
    let records = ReadOptions::new().records(1200, 3);
    assert_eq!(mdf.read("X", &records)?.values, index.read("X", &records)?.values);

    let steps: Vec<f64> = (0..3000).map(|i| (i / 7) as f64).collect();
    let path = write("steps.mf4", &steps)?;
    let (mdf, index) = (Mdf::from_file(&path)?, MdfIndex::from_file(&path)?);
    for (a, b) in [(Some(10.0), Some(12.0)), (None, Some(0.0)), (Some(428.0), None), (Some(5.5), Some(5.6))] {
        let w = ReadOptions::new().time(a, b);
        let (m, i) = (mdf.read("X", &w)?, index.read("X", &w)?);
        assert_eq!(m.values, i.values, "{a:?}..{b:?}");
        assert_eq!(m.timestamps, i.timestamps, "{a:?}..{b:?}");
    }
    assert_eq!(mdf.read("X", &ReadOptions::new().time(Some(10.0), Some(12.0)))?.values.len(), 21);
    Ok(())
}

/// An index re-attached to a different file (the source was replaced, or the
/// wrong file was given) is refused with a clear error naming both sizes,
/// rather than decoding bytes at offsets meant for another file; a missing
/// source names its path.
#[test]
fn source_that_does_not_match_the_index_is_refused() -> Result<(), mf4_rs::error::MdfError> {
    use mf4_rs::error::MdfError;
    use mf4_rs::{MdfIndex, MdfWriter};

    let dir = tempfile::tempdir()?;
    let write = |name: &str, records: u64| -> Result<String, MdfError> {
        let path = dir.path().join(name).to_str().unwrap().to_string();
        let mut w = MdfWriter::new(&path)?;
        w.init_mdf_file()?;
        let cg = w.add_channel_group(Some("G"))?;
        w.add_time_channel(&cg, "t")?;
        w.add_float_channel(&cg, "X")?;
        w.start_data_block(&cg)?;
        for k in 0..records {
            w.write_record(&cg, &[DecodedValue::Float(k as f64), DecodedValue::Float(k as f64)])?;
        }
        w.finish_data_block(&cg)?;
        w.finalize()?;
        Ok(path)
    };
    let (a, b) = (write("a.mf4", 100)?, write("b.mf4", 10)?);
    let mut index = MdfIndex::from_file(&a)?;
    assert_eq!(index.values("X", None)?.len(), 100);

    index.set_source(b.as_str());
    for result in [index.values("X", None).map(|_| ()), index.read("X", None).map(|_| ()), index.time_range("X", None).map(|_| ())] {
        match result {
            Err(MdfError::InvalidArgument(msg)) => {
                assert!(msg.contains("does not match the index"), "{msg}");
                assert!(msg.contains(&b), "{msg}");
            }
            other => panic!("expected a source mismatch, got {other:?}"),
        }
    }

    let missing = dir.path().join("missing.mf4").to_str().unwrap().to_string();
    index.set_source(missing.as_str());
    let err = index.values("X", None).unwrap_err().to_string();
    assert!(err.contains(&missing), "{err}");
    Ok(())
}

/// An index's JSON is byte-for-byte reproducible: conversion text/reference
/// maps are written with sorted keys, not in `HashMap` order.
#[test]
fn index_json_is_deterministic() -> Result<(), mf4_rs::error::MdfError> {
    use mf4_rs::{MdfIndex, MdfWriter};
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("texts.mf4").to_str().unwrap().to_string();
    let mut w = MdfWriter::new(&path)?;
    w.init_mdf_file()?;
    let cg = w.add_channel_group(Some("G"))?;
    w.add_time_channel(&cg, "t")?;
    let gear = w.add_int_channel(&cg, "Gear")?;
    let mapping: Vec<(i64, String)> = (0..40).map(|i| (i, format!("state-{i}"))).collect();
    let mapping: Vec<(i64, &str)> = mapping.iter().map(|(k, v)| (*k, v.as_str())).collect();
    w.add_value_to_text_conversion(&gear, &mapping, "unknown")?;
    w.start_data_block(&cg)?;
    w.write_record(&cg, &[DecodedValue::Float(0.0), DecodedValue::UnsignedInteger(3)])?;
    w.finish_data_block(&cg)?;
    w.finalize()?;

    let first = MdfIndex::from_file(&path)?.to_json()?;
    for _ in 0..5 {
        assert_eq!(MdfIndex::from_file(&path)?.to_json()?, first);
    }
    let pos = |k: &str| first.find(&format!("\"{k}\": \"state-")).unwrap();
    assert!(pos("2") < pos("10") && pos("10") < pos("39"), "keys must be in numeric order");
    Ok(())
}

/// `HTTP://` is recognised as a URL in any letter case; other URL schemes
/// (`s3://`, `gs://`) are refused with an explanation instead of an opaque
/// "No such file"; a Windows drive path stays a file path.
#[test]
fn source_strings_are_classified_clearly() {
    use mf4_rs::index::Source;
    use mf4_rs::MdfIndex;
    #[cfg(feature = "http")]
    assert!(matches!(Source::from("HTTPS://example.com/a.mf4"), Source::Url(_)));
    assert!(matches!(Source::from(r"C:\data\a.mf4"), Source::File(_)));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.mf4").to_str().unwrap().to_string();
    let mut w = mf4_rs::MdfWriter::new(&path).unwrap();
    w.init_mdf_file().unwrap();
    let cg = w.add_channel_group(Some("G")).unwrap();
    w.add_float_channel(&cg, "X").unwrap();
    w.start_data_block(&cg).unwrap();
    w.write_record(&cg, &[DecodedValue::Float(1.0)]).unwrap();
    w.finish_data_block(&cg).unwrap();
    w.finalize().unwrap();
    let mut index = MdfIndex::from_file(&path).unwrap();
    index.set_source("s3://bucket/a.mf4");
    let err = index.values("X", None).unwrap_err().to_string();
    assert!(err.contains("s3://") && err.contains("http(s)://"), "{err}");
    let err = index.open_file("https://example.com/a.mf4").err().expect("a URL is not a file").to_string();
    #[cfg(feature = "http")]
    assert!(err.contains("set_source") && !err.contains("without the `http` feature"), "{err}");
    #[cfg(not(feature = "http"))]
    assert!(err.contains("without the `http` feature"), "{err}");
}
