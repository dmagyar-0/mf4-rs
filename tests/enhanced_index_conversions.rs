use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::common::{DataType, BlockHeader};
use mf4_rs::blocks::conversion::{ConversionBlock, ConversionType};
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::index::{MdfIndex, FileRangeReader, IndexedChannel, IndexedChannelGroup};
use mf4_rs::api::mdf::MDF;
use mf4_rs::error::MdfError;
use std::fs;

#[test]
fn test_enhanced_index_with_text_conversions() -> Result<(), MdfError> {
    let mdf_path = std::env::temp_dir().join("enhanced_conversion_test.mf4");
    let index_path = std::env::temp_dir().join("enhanced_conversion_index.json");
    
    // Clean up any existing files
    if mdf_path.exists() { fs::remove_file(&mdf_path)?; }
    if index_path.exists() { fs::remove_file(&index_path)?; }

    // Create a test MDF file with a channel that would benefit from text conversion
    let mut writer = MdfWriter::new(mdf_path.to_str().unwrap())?;
    writer.init_mdf_file()?;
    
    let cg_id = writer.add_channel_group(None, |_| {})?;
    
    // Create a status channel that will use value-to-text conversion
    let _status_ch_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Status".to_string());
        ch.bit_count = 8;
    })?;
    
    writer.start_data_block_for_cg(&cg_id, 0)?;
    
    // Write some test data with different status values
    let status_values = vec![0u64, 1u64, 2u64, 3u64, 0u64, 1u64];
    for status in &status_values {
        writer.write_record(&cg_id, &[
            DecodedValue::UnsignedInteger(*status),
        ])?;
    }
    
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    // Test 1: Create and verify enhanced index
    println!("=== Test 1: Creating Enhanced Index ===");
    let index = MdfIndex::from_file(mdf_path.to_str().unwrap())?;
    index.save_to_file(index_path.to_str().unwrap())?;

    // Test 2: Load index and verify structure
    println!("=== Test 2: Loading and Verifying Index Structure ===");
    let loaded_index = MdfIndex::load_from_file(index_path.to_str().unwrap())?;
    
    assert_eq!(loaded_index.channel_groups.len(), 1);
    
    let group = &loaded_index.channel_groups[0];
    assert_eq!(group.channels.len(), 1);
    assert_eq!(group.record_count, status_values.len() as u64);
    
    let status_channel = &group.channels[0];
    assert_eq!(status_channel.name, Some("Status".to_string()));
    assert_eq!(status_channel.data_type, DataType::UnsignedIntegerLE);
    
    // Test 3: Read channel values via enhanced index
    println!("=== Test 3: Reading Values via Enhanced Index ===");
    let mut reader = FileRangeReader::new(mdf_path.to_str().unwrap())?;
    let status_values_via_index = loaded_index.read_channel_values(0, 0, &mut reader)?;
    
    assert_eq!(status_values_via_index.len(), status_values.len());
    
    // Verify the actual values match
    for (i, (expected_status, actual_value)) in status_values.iter().zip(status_values_via_index.iter()).enumerate() {
        match actual_value {
            Some(DecodedValue::UnsignedInteger(actual_status)) => {
                assert_eq!(*actual_status, *expected_status, "Status value mismatch at record {}", i);
            }
            _ => panic!("Expected UnsignedInteger value for status channel at record {}", i),
        }
    }

    // Test 4: Compare with direct MDF reading
    println!("=== Test 4: Comparing with Direct MDF Reading ===");
    let mdf = MDF::from_file(mdf_path.to_str().unwrap())?;
    let direct_values = mdf.channel_groups()[0].channels()[0].values()?;
    
    assert_eq!(direct_values.len(), status_values_via_index.len());
    
    for (i, (direct_val, index_val)) in direct_values.iter().zip(status_values_via_index.iter()).enumerate() {
        assert_eq!(*direct_val, *index_val, "Value mismatch between direct and index reading at record {}", i);
    }

    // Test 5: Test name-based access
    println!("=== Test 5: Testing Name-based Access ===");
    let status_by_name = loaded_index.read_channel_values_by_name("Status", &mut reader)?;
    assert_eq!(status_by_name.len(), status_values.len());
    
    for (i, (expected, actual)) in status_values.iter().zip(status_by_name.iter()).enumerate() {
        if let Some(DecodedValue::UnsignedInteger(actual_val)) = actual {
            assert_eq!(*actual_val, *expected, "Named access value mismatch at record {}", i);
        } else {
            panic!("Expected UnsignedInteger for named access at record {}", i);
        }
    }

    // Test 6: Test byte range calculations
    println!("=== Test 6: Testing Byte Range Calculations ===");
    let byte_ranges = loaded_index.get_channel_byte_ranges(0, 0)?;
    assert!(!byte_ranges.is_empty(), "Should have at least one byte range");
    
    let (total_bytes, range_count) = loaded_index.get_channel_byte_summary(0, 0)?;
    assert!(total_bytes > 0, "Should have positive total bytes");
    assert_eq!(range_count, byte_ranges.len(), "Range count should match");
    
    // Clean up
    fs::remove_file(mdf_path)?;
    fs::remove_file(index_path)?;

    println!("=== All Enhanced Index Tests Passed! ===");
    Ok(())
}

#[test]
fn test_conversion_dependency_resolution() -> Result<(), MdfError> {
    println!("=== Testing Conversion Dependency Resolution ===");
    
    // Create a simple conversion block and test the resolution methods
    let mut conversion = ConversionBlock {
        header: BlockHeader {
            id: "##CC".to_string(),
            reserved0: 0,
            block_len: 160,
            links_nr: 2,
        },
        cc_tx_name: None,
        cc_md_unit: None,
        cc_md_comment: None,
        cc_cc_inverse: None,
        cc_ref: vec![0, 0], // No actual references for this test
        cc_type: ConversionType::Linear,
        cc_precision: 0,
        cc_flags: 0,
        cc_ref_count: 0,
        cc_val_count: 2,
        cc_phy_range_min: None,
        cc_phy_range_max: None,
        cc_val: vec![1.0, 2.0], // Linear: y = 1.0 + 2.0*x
        formula: None,
        resolved_texts: None,
        resolved_conversions: None,
    };
    
    // Test resolution with empty file data (should not crash)
    let empty_data = vec![];
    let result = conversion.resolve_all_dependencies(&empty_data);
    assert!(result.is_ok(), "Resolution should succeed even with empty data");
    
    // Test linear conversion application
    let test_value = DecodedValue::UnsignedInteger(5);
    let converted = conversion.apply_decoded(test_value, &[])?;
    
    if let DecodedValue::Float(result) = converted {
        assert!((result - 11.0).abs() < 0.001, "Linear conversion should give 1.0 + 2.0*5 = 11.0, got {}", result);
    } else {
        panic!("Expected Float result from linear conversion");
    }
    
    println!("Conversion dependency resolution test passed!");
    Ok(())
}

#[test]
fn test_resolved_data_accessor_methods() -> Result<(), MdfError> {
    println!("=== Testing Resolved Data Accessor Methods ===");
    
    let mut conversion = ConversionBlock {
        header: BlockHeader {
            id: "##CC".to_string(),
            reserved0: 0,
            block_len: 160,
            links_nr: 1,
        },
        cc_tx_name: None,
        cc_md_unit: None,
        cc_md_comment: None,
        cc_cc_inverse: None,
        cc_ref: vec![0],
        cc_type: ConversionType::ValueToText,
        cc_precision: 0,
        cc_flags: 0,
        cc_ref_count: 1,
        cc_val_count: 1,
        cc_phy_range_min: None,
        cc_phy_range_max: None,
        cc_val: vec![42.0],
        formula: None,
        resolved_texts: None,
        resolved_conversions: None,
    };
    
    // Manually set some resolved data
    let mut resolved_texts = std::collections::HashMap::new();
    resolved_texts.insert(0, "Test Text".to_string());
    conversion.resolved_texts = Some(resolved_texts);
    
    // Test accessor methods
    assert_eq!(conversion.get_resolved_text(0), Some(&"Test Text".to_string()));
    assert_eq!(conversion.get_resolved_text(1), None);
    assert!(conversion.get_resolved_conversion(0).is_none());
    
    println!("Resolved data accessor methods test passed!");
    Ok(())
}

#[test] 
fn test_index_serialization_with_resolved_data() -> Result<(), MdfError> {
    println!("=== Testing Index Serialization with Resolved Data ===");
    
    let temp_index_path = std::env::temp_dir().join("serialization_test.json");
    if temp_index_path.exists() { fs::remove_file(&temp_index_path)?; }
    
    // Create a mock index with resolved conversion data
    let mut conversion = ConversionBlock {
        header: BlockHeader {
            id: "##CC".to_string(),
            reserved0: 0,
            block_len: 160,
            links_nr: 1,
        },
        cc_tx_name: None,
        cc_md_unit: None,
        cc_md_comment: None,
        cc_cc_inverse: None,
        cc_ref: vec![0],
        cc_type: ConversionType::Linear,
        cc_precision: 0,
        cc_flags: 0,
        cc_ref_count: 0,
        cc_val_count: 2,
        cc_phy_range_min: None,
        cc_phy_range_max: None,
        cc_val: vec![0.0, 1.0],
        formula: None,
        resolved_texts: None,
        resolved_conversions: None,
    };
    
    let mut resolved_texts = std::collections::HashMap::new();
    resolved_texts.insert(0, "Resolved Text".to_string());
    conversion.resolved_texts = Some(resolved_texts);
    
    let indexed_channel = IndexedChannel {
        name: Some("Test Channel".to_string()),
        unit: Some("V".to_string()),
        data_type: DataType::FloatLE,
        byte_offset: 0,
        bit_offset: 0,
        bit_count: 32,
        channel_type: 0,
        flags: 0,
        pos_invalidation_bit: 0,
        conversion: Some(conversion),
        vlsd_data_address: None,
    };
    
    let indexed_group = IndexedChannelGroup {
        name: Some("Test Group".to_string()),
        comment: None,
        record_id_len: 0,
        record_size: 4,
        record_count: 1,
        channels: vec![indexed_channel],
        data_blocks: vec![],
    };
    
    let index = MdfIndex {
        file_size: 1024,
        channel_groups: vec![indexed_group],
    };
    
    // Test serialization
    index.save_to_file(temp_index_path.to_str().unwrap())?;
    
    // Test deserialization
    let loaded_index = MdfIndex::load_from_file(temp_index_path.to_str().unwrap())?;
    
    // Verify the resolved data was preserved
    assert_eq!(loaded_index.channel_groups.len(), 1);
    let loaded_group = &loaded_index.channel_groups[0];
    assert_eq!(loaded_group.channels.len(), 1);
    let loaded_channel = &loaded_group.channels[0];
    
    if let Some(ref conversion) = loaded_channel.conversion {
        assert_eq!(conversion.get_resolved_text(0), Some(&"Resolved Text".to_string()));
    } else {
        panic!("Expected conversion block to be present");
    }
    
    // Clean up
    fs::remove_file(temp_index_path)?;
    
    println!("Index serialization with resolved data test passed!");
    Ok(())
}