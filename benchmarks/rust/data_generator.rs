use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::error::MdfError;
use std::time::Instant;

fn main() -> Result<(), MdfError> {
    println!("🏭 Large MDF File Generator for Performance Testing");
    println!("==================================================");
    
    // Generate files of different sizes for benchmarking
    let test_configs = vec![
        ("small_1mb.mf4", 10_000, "1MB"),      // ~1MB
        ("medium_10mb.mf4", 100_000, "10MB"),   // ~10MB  
        ("large_100mb.mf4", 1_000_000, "100MB"), // ~100MB
        ("huge_500mb.mf4", 5_000_000, "500MB"),  // ~500MB
    ];
    
    for (filename, record_count, description) in test_configs {
        println!("\n📁 Generating {} file: {}", description, filename);
        let start = Instant::now();
        
        create_large_mdf_file(filename, record_count)?;
        
        let duration = start.elapsed();
        let file_size = std::fs::metadata(filename)?.len();
        let mb_size = file_size as f64 / 1_048_576.0;
        let records_per_sec = record_count as f64 / duration.as_secs_f64();
        
        println!("✅ Created {} ({:.1} MB) in {:.2}s", 
                filename, mb_size, duration.as_secs_f64());
        println!("   📊 {} records at {:.0} records/sec", 
                record_count, records_per_sec);
        println!("   💾 {:.2} MB/sec write speed", 
                mb_size / duration.as_secs_f64());
    }
    
    println!("\n🎯 Large MDF files generated successfully!");
    println!("Use these files for Python vs Rust performance comparisons.");
    
    Ok(())
}

fn create_large_mdf_file(filename: &str, record_count: usize) -> Result<(), MdfError> {
    let mut writer = MdfWriter::new(filename)?;
    let (_id, _hd) = writer.init_mdf_file()?;
    
    // Create a single channel group with multiple channels
    let cg_id = writer.add_channel_group(None, |_cg| {
        // Could set channel group name here
    })?;
    
    // Add multiple channels with different data types for realistic testing
    let time_ch_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".to_string());
        ch.bit_count = 64; // Use 64-bit for time precision
    })?;
    
    writer.set_time_channel(&time_ch_id)?;
    
    // Add data channels sequentially (proper linking)
    let temp_ch_id = writer.add_channel(&cg_id, Some(&time_ch_id), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Temperature".to_string());
        ch.bit_count = 32;
    })?;
    
    let pressure_ch_id = writer.add_channel(&cg_id, Some(&temp_ch_id), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Pressure".to_string());
        ch.bit_count = 32;
    })?;
    
    let speed_ch_id = writer.add_channel(&cg_id, Some(&pressure_ch_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Speed".to_string());
        ch.bit_count = 32;
    })?;
    
    let voltage_ch_id = writer.add_channel(&cg_id, Some(&speed_ch_id), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Voltage".to_string());
        ch.bit_count = 32;
    })?;
    
    let current_ch_id = writer.add_channel(&cg_id, Some(&voltage_ch_id), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Current".to_string());
        ch.bit_count = 32;
    })?;
    
    writer.add_channel(&cg_id, Some(&current_ch_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Status".to_string());
        ch.bit_count = 32;
    })?;
    
    // Start writing data
    writer.start_data_block_for_cg(&cg_id, 0)?;
    
    // Write records with realistic data patterns
    let mut progress_interval = record_count / 10; // Show progress every 10%
    if progress_interval == 0 { progress_interval = 1; }
    
    for i in 0..record_count {
        if i % progress_interval == 0 {
            let progress = (i as f64 / record_count as f64) * 100.0;
            print!("\r   Progress: {:.0}% ({}/{})", progress, i, record_count);
            std::io::Write::flush(&mut std::io::stdout()).unwrap();
        }
        
        let time = i as f64 * 0.001; // 1ms intervals
        let temperature = 20.0 + 10.0 * (time * 0.1).sin() + (i as f64 * 0.001).cos();
        let pressure = 1013.25 + 50.0 * (time * 0.05).sin() + (i as f64 * 0.0001);
        let speed = ((60.0 + 40.0 * (time * 0.2).sin()) as u64).max(0);
        let voltage = 12.0 + 2.0 * (time * 0.3).cos() + 0.1 * (i as f64 * 0.01).sin();
        let current = 2.0 + 1.0 * (time * 0.15).sin() + 0.05 * (i as f64 * 0.02).cos();
        let status = (i % 4) as u64; // Cycling status values
        
        writer.write_record(&cg_id, &[
            DecodedValue::Float(time),
            DecodedValue::Float(temperature),
            DecodedValue::Float(pressure),
            DecodedValue::UnsignedInteger(speed),
            DecodedValue::Float(voltage),
            DecodedValue::Float(current),
            DecodedValue::UnsignedInteger(status),
        ])?;
    }
    
    println!("\r   Progress: 100% ({}/{})", record_count, record_count);
    
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_small_file_generation() {
        // Test with a very small file to ensure the function works
        let result = create_large_mdf_file("test_small.mf4", 100);
        assert!(result.is_ok());
        
        // Verify the file was created and has reasonable size
        let metadata = std::fs::metadata("test_small.mf4").unwrap();
        assert!(metadata.len() > 1000); // Should be at least 1KB
        
        // Cleanup
        std::fs::remove_file("test_small.mf4").unwrap();
    }
}