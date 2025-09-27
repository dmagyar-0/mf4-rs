use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::index::{MdfIndex, FileRangeReader, ByteRangeReader};
use mf4_rs::error::MdfError;

fn main() -> Result<(), MdfError> {
    let mdf_path = "byte_ranges_test.mf4";
    let index_path = "byte_ranges_index.json";

    println!("=== Creating MDF File with Sample Data ===");
    create_test_mdf_file(mdf_path)?;

    println!("\n=== Creating Index ===");
    let index = MdfIndex::from_file(mdf_path)?;
    index.save_to_file(index_path)?;

    println!("\n=== Analyzing Byte Ranges ===");
    demonstrate_byte_ranges(&index)?;

    println!("\n=== Reading Data Using Byte Ranges ===");
    read_data_using_byte_ranges(&index)?;

    println!("\n=== Partial Record Reading ===");
    demonstrate_partial_record_reading(&index)?;

    println!("\nExample completed successfully!");
    println!("Check the generated files:");
    println!("  - {}", mdf_path);
    println!("  - {}", index_path);

    Ok(())
}

fn create_test_mdf_file(path: &str) -> Result<(), MdfError> {
    let mut writer = MdfWriter::new(path)?;
    writer.init_mdf_file()?;
    
    // Create a channel group with multiple channels of different types
    let cg_id = writer.add_channel_group(None, |_| {})?;
    
    // Time channel (64-bit float)
    let time_ch_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".to_string());
        ch.bit_count = 64;
    })?;
    writer.set_time_channel(&time_ch_id)?;
    
    // Temperature channel (32-bit float)
    writer.add_channel(&cg_id, Some(&time_ch_id), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Temperature".to_string());
        ch.bit_count = 32;
    })?;
    
    // Speed channel (16-bit unsigned integer)
    writer.add_channel(&cg_id, Some(&time_ch_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Speed".to_string());
        ch.bit_count = 16;
    })?;
    
    // Status channel (8-bit unsigned integer)
    writer.add_channel(&cg_id, Some(&time_ch_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Status".to_string());
        ch.bit_count = 8;
    })?;
    
    // Write sample data
    writer.start_data_block_for_cg(&cg_id, 0)?;
    
    for i in 0..50 {
        let time = i as f64 * 0.1;
        let temperature = 20.0 + 10.0 * (time * 0.5).sin();
        let speed = ((time * 2.0).cos() * 30.0 + 60.0) as u64;
        let status = if i % 10 == 0 { 1u64 } else { 0u64 };
        
        writer.write_record(&cg_id, &[
            DecodedValue::Float(time),
            DecodedValue::Float(temperature),
            DecodedValue::UnsignedInteger(speed),
            DecodedValue::UnsignedInteger(status),
        ])?;
    }
    
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;
    
    Ok(())
}

fn demonstrate_byte_ranges(index: &MdfIndex) -> Result<(), MdfError> {
    println!("Channel Groups: {}", index.channel_groups.len());
    
    if let Some(channels) = index.list_channels(0) {
        for (ch_idx, ch_name, data_type) in channels {
            println!("\nChannel {}: {} ({:?})", ch_idx, ch_name, data_type);
            
            // Get byte ranges for all data
            match index.get_channel_byte_ranges(0, ch_idx) {
                Ok(ranges) => {
                    println!("  Byte ranges for all data:");
                    for (i, (offset, length)) in ranges.iter().enumerate() {
                        println!("    Range {}: offset={}, length={} bytes", i, offset, length);
                    }
                    
                    // Get summary
                    let (total_bytes, range_count) = index.get_channel_byte_summary(0, ch_idx)?;
                    println!("  Summary: {} total bytes in {} ranges", total_bytes, range_count);
                }
                Err(e) => {
                    println!("  Error calculating byte ranges: {}", e);
                }
            }
            
            // Get byte ranges for a subset of records (first 10 records)
            match index.get_channel_byte_ranges_for_records(0, ch_idx, 0, 10) {
                Ok(ranges) => {
                    println!("  Byte ranges for first 10 records:");
                    for (i, (offset, length)) in ranges.iter().enumerate() {
                        println!("    Range {}: offset={}, length={} bytes", i, offset, length);
                    }
                }
                Err(e) => {
                    println!("  Error calculating partial byte ranges: {}", e);
                }
            }
        }
    }
    
    Ok(())
}

fn read_data_using_byte_ranges(index: &MdfIndex) -> Result<(), MdfError> {
    println!("Reading Status channel (index 1) using byte ranges...");
    
    // Get byte ranges for the status channel
    let ranges = index.get_channel_byte_ranges(0, 1)?;
    
    // Option 1: Use built-in FileRangeReader
    println!("  Method 1: Using built-in FileRangeReader");
    let mut file_reader = FileRangeReader::new("byte_ranges_test.mf4")?;
    let all_data_via_reader = file_reader.read_range(ranges[0].0, ranges[0].1)?;
    println!("    Read {} bytes via FileRangeReader", all_data_via_reader.len());
    
    // Option 2: Manual byte range reading (as you would do with HTTP)
    println!("  Method 2: Manual byte range reading (for HTTP/custom scenarios)");
    let mut all_channel_data = Vec::new();
    
    for (i, (offset, length)) in ranges.iter().enumerate() {
        println!("    Range {}: {} bytes from offset {} (in HTTP this would be 'bytes={}-{}')", 
                 i, length, offset, offset, offset + length - 1);
        
        // In production, you'd make an HTTP request here:
        // let response = client.get(url)
        //     .header("Range", format!("bytes={}-{}", offset, offset + length - 1))
        //     .send()?;
        // let bytes = response.bytes()?.to_vec();
        
        // For demo, simulate this with direct file access
        let mut file_reader = FileRangeReader::new("byte_ranges_test.mf4")?;
        let buffer = file_reader.read_range(*offset, *length)?;
        all_channel_data.extend_from_slice(&buffer);
    }
    
    println!("    Total raw bytes read: {}", all_channel_data.len());
    
    // Show first few bytes as hex
    print!("    First 20 bytes (hex): ");
    for (i, &byte) in all_channel_data.iter().take(20).enumerate() {
        print!("{:02x}", byte);
        if i % 4 == 3 { print!(" "); }
    }
    println!();
    
    // Compare: verify both methods give same result
    if all_data_via_reader == all_channel_data {
        println!("  ✓ Both methods produced identical results");
    } else {
        println!("  ✗ Methods produced different results");
    }
    
    Ok(())
}

fn demonstrate_partial_record_reading(index: &MdfIndex) -> Result<(), MdfError> {
    // First, let's see what channels we actually have
    if let Some(channels) = index.list_channels(0) {
        println!("Available channels:");
        for (idx, name, data_type) in &channels {
            println!("  Channel {}: {} ({:?})", idx, name, data_type);
        }
        
        // Use the last channel instead of assuming index 2
        let speed_channel_idx = channels.len() - 1;
        let (_, speed_name, _) = &channels[speed_channel_idx];
        
        println!("Reading partial records (records 10-19) for {} channel (index {})...", speed_name, speed_channel_idx);
        
        // Get byte ranges for records 10-19 of the speed channel
        let ranges = index.get_channel_byte_ranges_for_records(0, speed_channel_idx, 10, 10)?;
        
        println!("  Byte ranges for records 10-19:");
        for (i, (offset, length)) in ranges.iter().enumerate() {
            println!("    Range {}: offset={}, length={} bytes", i, offset, length);
        }
        
        // Calculate some statistics
        let total_bytes: u64 = ranges.iter().map(|(_, len)| len).sum();
        println!("  Total bytes needed: {}", total_bytes);
        
        // Compare with reading all records
        let all_ranges = index.get_channel_byte_ranges(0, speed_channel_idx)?;
        let all_total_bytes: u64 = all_ranges.iter().map(|(_, len)| len).sum();
        let savings_percentage = ((all_total_bytes - total_bytes) as f64 / all_total_bytes as f64) * 100.0;
        
        println!("  Savings: {} bytes ({:.1}% less than reading all records)", 
                 all_total_bytes - total_bytes, savings_percentage);
        
        // Show how to read with minimal I/O
        if ranges.len() == 1 {
            println!("  Optimal: Single contiguous read required");
        } else {
            println!("  Note: {} separate reads required due to data layout", ranges.len());
        }
    } else {
        println!("No channels found in group 0");
    }
    
    Ok(())
}