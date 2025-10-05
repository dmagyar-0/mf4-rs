use mf4_rs::blocks::channel_block::ChannelBlock;
use mf4_rs::blocks::common::{BlockHeader, DataType};
use mf4_rs::parsing::decoder::{check_value_validity, decode_channel_value_with_validity, DecodedValue};

/// Helper function to create a minimal ChannelBlock for testing
fn create_test_channel(flags: u32, pos_invalidation_bit: u32) -> ChannelBlock {
    ChannelBlock {
        header: BlockHeader {
            id: "##CN".to_string(),
            reserved0: 0,
            block_len: 160,
            links_nr: 8,
        },
        next_ch_addr: 0,
        component_addr: 0,
        name_addr: 0,
        source_addr: 0,
        conversion_addr: 0,
        data: 0,
        unit_addr: 0,
        comment_addr: 0,
        channel_type: 0,
        sync_type: 0,
        data_type: DataType::UnsignedIntegerLE,
        bit_offset: 0,
        byte_offset: 0,
        bit_count: 16,
        flags,
        pos_invalidation_bit,
        precision: 0,
        reserved1: 0,
        attachment_nr: 0,
        min_raw_value: 0.0,
        max_raw_value: 0.0,
        lower_limit: 0.0,
        upper_limit: 0.0,
        lower_ext_limit: 0.0,
        upper_ext_limit: 0.0,
        name: None,
        conversion: None,
    }
}

#[test]
fn test_all_values_invalid_flag() {
    // Test cn_flags bit 0 set = all values invalid
    let channel = create_test_channel(0x01, 0);
    
    // Create a simple record: record_id(1 byte) + data(4 bytes) + invalidation(1 byte)
    let record = vec![0xFF, 0x12, 0x34, 0x00, 0x00, 0x00];
    let record_id_size = 1;
    let cg_data_bytes = 4;
    
    let is_valid = check_value_validity(&record, record_id_size, cg_data_bytes, &channel);
    
    assert_eq!(is_valid, false, "When cn_flags bit 0 is set, all values should be invalid");
}

#[test]
fn test_all_values_valid_flag() {
    // Test cn_flags bits 0 and 1 both clear = all values valid
    let channel = create_test_channel(0x00, 0);
    
    // Create a simple record
    let record = vec![0xFF, 0x12, 0x34, 0x00, 0x00, 0xFF];
    let record_id_size = 1;
    let cg_data_bytes = 4;
    
    let is_valid = check_value_validity(&record, record_id_size, cg_data_bytes, &channel);
    
    assert_eq!(is_valid, true, "When cn_flags bits 0 and 1 are clear, all values should be valid");
}

#[test]
fn test_invalidation_bit_position_0_set() {
    // Test checking invalidation bit at position 0 (LSB of first invalidation byte)
    // cn_flags = 0x02 (bit 1 set = must check invalidation bit)
    let channel = create_test_channel(0x02, 0);
    
    // Record structure: record_id(1) + data(4) + inval_byte(1)
    // Invalidation byte at offset 5 with bit 0 set (0x01)
    let record = vec![0xFF, 0x12, 0x34, 0x00, 0x00, 0x01];
    let record_id_size = 1;
    let cg_data_bytes = 4;
    
    let is_valid = check_value_validity(&record, record_id_size, cg_data_bytes, &channel);
    
    assert_eq!(is_valid, false, "When invalidation bit is set, value should be invalid");
}

#[test]
fn test_invalidation_bit_position_0_clear() {
    // Test checking invalidation bit at position 0 when it's clear
    let channel = create_test_channel(0x02, 0);
    
    // Invalidation byte at offset 5 with bit 0 clear (0x00)
    let record = vec![0xFF, 0x12, 0x34, 0x00, 0x00, 0x00];
    let record_id_size = 1;
    let cg_data_bytes = 4;
    
    let is_valid = check_value_validity(&record, record_id_size, cg_data_bytes, &channel);
    
    assert_eq!(is_valid, true, "When invalidation bit is clear, value should be valid");
}

#[test]
fn test_invalidation_bit_position_5() {
    // Test checking invalidation bit at position 5 (bit 5 of first invalidation byte)
    let channel = create_test_channel(0x02, 5);
    
    // Invalidation byte with bit 5 set (0x20 = 0b00100000)
    let record = vec![0xFF, 0x12, 0x34, 0x00, 0x00, 0x20];
    let record_id_size = 1;
    let cg_data_bytes = 4;
    
    let is_valid = check_value_validity(&record, record_id_size, cg_data_bytes, &channel);
    
    assert_eq!(is_valid, false, "When invalidation bit 5 is set, value should be invalid");
}

#[test]
fn test_invalidation_bit_position_in_second_byte() {
    // Test checking invalidation bit at position 10 (bit 2 of second invalidation byte)
    // pos_invalidation_bit = 10 means:
    //   - byte offset: 10 >> 3 = 1 (second byte)
    //   - bit offset: 10 & 0x07 = 2 (bit 2)
    let channel = create_test_channel(0x02, 10);
    
    // Record: record_id(1) + data(4) + inval_bytes(2)
    // Second invalidation byte (offset 6) with bit 2 set (0x04 = 0b00000100)
    let record = vec![0xFF, 0x12, 0x34, 0x00, 0x00, 0x00, 0x04];
    let record_id_size = 1;
    let cg_data_bytes = 4;
    
    let is_valid = check_value_validity(&record, record_id_size, cg_data_bytes, &channel);
    
    assert_eq!(is_valid, false, "When invalidation bit in second byte is set, value should be invalid");
}

#[test]
fn test_decode_with_validity_valid_sample() {
    // Test full decoding with validity checking - valid sample
    let mut channel = create_test_channel(0x02, 0);
    channel.byte_offset = 0;
    channel.bit_offset = 0;
    channel.bit_count = 16;
    channel.data_type = DataType::UnsignedIntegerLE;
    
    // Record: record_id(1) + data(2 bytes = 0x3412 LE = 4660) + inval(1, bit clear)
    let record = vec![0xFF, 0x12, 0x34, 0x00];
    let record_id_size = 1;
    let cg_data_bytes = 2;
    
    let result = decode_channel_value_with_validity(
        &record, 
        record_id_size, 
        cg_data_bytes,
        &channel
    );
    
    assert!(result.is_some());
    let decoded = result.unwrap();
    assert_eq!(decoded.is_valid, true);
    assert_eq!(decoded.value, DecodedValue::UnsignedInteger(0x3412));
}

#[test]
fn test_decode_with_validity_invalid_sample() {
    // Test full decoding with validity checking - invalid sample
    let mut channel = create_test_channel(0x02, 0);
    channel.byte_offset = 0;
    channel.bit_offset = 0;
    channel.bit_count = 16;
    channel.data_type = DataType::UnsignedIntegerLE;
    
    // Record: record_id(1) + data(2 bytes) + inval(1, bit 0 set)
    let record = vec![0xFF, 0x12, 0x34, 0x01];
    let record_id_size = 1;
    let cg_data_bytes = 2;
    
    let result = decode_channel_value_with_validity(
        &record, 
        record_id_size, 
        cg_data_bytes,
        &channel
    );
    
    assert!(result.is_some());
    let decoded = result.unwrap();
    assert_eq!(decoded.is_valid, false, "Sample should be marked as invalid");
    // Value should still be decoded correctly, just marked as invalid
    assert_eq!(decoded.value, DecodedValue::UnsignedInteger(0x3412));
}

#[test]
fn test_no_invalidation_bytes_available() {
    // Test when record is too short to contain invalidation bytes
    // Should assume valid (graceful degradation)
    let channel = create_test_channel(0x02, 0);
    
    // Short record without invalidation bytes
    let record = vec![0xFF, 0x12, 0x34];
    let record_id_size = 1;
    let cg_data_bytes = 4; // Claiming 4 data bytes but record is shorter
    
    let is_valid = check_value_validity(&record, record_id_size, cg_data_bytes, &channel);
    
    assert_eq!(is_valid, true, "When invalidation bytes are not available, should assume valid");
}

#[test]
fn test_sorted_data_no_record_id() {
    // Test with sorted data (record_id_size = 0)
    let channel = create_test_channel(0x02, 0);
    
    // Record: data(4) + inval(1, bit clear)
    let record = vec![0x12, 0x34, 0x00, 0x00, 0x00];
    let record_id_size = 0; // No record ID in sorted data
    let cg_data_bytes = 4;
    
    let is_valid = check_value_validity(&record, record_id_size, cg_data_bytes, &channel);
    
    assert_eq!(is_valid, true, "Should work correctly with sorted data (no record ID)");
}

#[test]
fn test_multiple_invalidation_bits() {
    // Test that different channels can have different invalidation bits
    let channel1 = create_test_channel(0x02, 0);
    let channel2 = create_test_channel(0x02, 1);
    
    // Invalidation byte with bit 0 set (0x01) but bit 1 clear
    let record = vec![0xFF, 0x12, 0x34, 0x00, 0x00, 0x01];
    let record_id_size = 1;
    let cg_data_bytes = 4;
    
    let is_valid1 = check_value_validity(&record, record_id_size, cg_data_bytes, &channel1);
    let is_valid2 = check_value_validity(&record, record_id_size, cg_data_bytes, &channel2);
    
    assert_eq!(is_valid1, false, "Channel 1 (bit 0) should be invalid");
    assert_eq!(is_valid2, true, "Channel 2 (bit 1) should be valid");
}

#[test]
fn test_flag_priority_over_bits() {
    // Test that cn_flags bit 0 takes priority over invalidation bits
    let channel = create_test_channel(0x01, 0); // All invalid flag set
    
    // Invalidation bit is CLEAR, but flag says all invalid
    let record = vec![0xFF, 0x12, 0x34, 0x00, 0x00, 0x00];
    let record_id_size = 1;
    let cg_data_bytes = 4;
    
    let is_valid = check_value_validity(&record, record_id_size, cg_data_bytes, &channel);
    
    assert_eq!(is_valid, false, "Flag should take priority: all values invalid");
}
