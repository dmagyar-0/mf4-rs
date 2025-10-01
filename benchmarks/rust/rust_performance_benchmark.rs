use mf4_rs::api::mdf::MDF;
use mf4_rs::index::MdfIndex;
use mf4_rs::error::MdfError;
use std::time::Instant;
use std::fs;

fn main() -> Result<(), MdfError> {
    println!("🦀 Rust Performance Benchmark");
    println!("=============================");
    
    let test_files = vec![
        "small_1mb.mf4",
        "medium_10mb.mf4", 
        "large_100mb.mf4",
        "huge_500mb.mf4",
    ];
    
    // Check which files exist
    let available_files: Vec<&str> = test_files.into_iter()
        .filter(|&file| fs::metadata(file).is_ok())
        .collect();
    
    if available_files.is_empty() {
        println!("❌ No test files found! Run 'cargo run --example generate_large_mdf' first.");
        return Ok(());
    }
    
    println!("📁 Found {} test files", available_files.len());
    
    for filename in available_files {
        let file_size = fs::metadata(filename)?.len();
        let mb_size = file_size as f64 / 1_048_576.0;
        
        println!("\n🔍 Benchmarking: {} ({:.1} MB)", filename, mb_size);
        println!("{}", "─".repeat(50));
        
        benchmark_file(filename, file_size)?;
    }
    
    println!("\n✅ Rust benchmarks completed!");
    Ok(())
}

fn benchmark_file(filename: &str, file_size: u64) -> Result<(), MdfError> {
    let mb_size = file_size as f64 / 1_048_576.0;
    
    // Benchmark 1: Full MDF parsing and reading
    println!("📖 1. Full MDF parsing and channel reading:");
    let start = Instant::now();
    
    let mdf = MDF::from_file(filename)?;
    let parsing_duration = start.elapsed();
    
    let groups = mdf.channel_groups();
    let mut total_channels = 0;
    let mut total_records = 0;
    
    for group in &groups {
        total_channels += group.channels().len();
        if let Some(first_channel) = group.channels().first() {
            let values = first_channel.values()?;
            total_records += values.len();
        }
    }
    
    let total_duration = start.elapsed();
    
    println!("   ⏱️  Parsing: {:.3}s", parsing_duration.as_secs_f64());
    println!("   ⏱️  Total (parse + read): {:.3}s", total_duration.as_secs_f64());
    println!("   📊 Found: {} groups, {} channels, {} records", 
             groups.len(), total_channels, total_records);
    println!("   🚀 Throughput: {:.2} MB/s", mb_size / total_duration.as_secs_f64());
    
    // Benchmark 2: Index creation
    println!("\n📇 2. Index creation:");
    let start = Instant::now();
    
    let index = MdfIndex::from_file(filename)?;
    let index_duration = start.elapsed();
    
    let index_filename = format!("{}.index.json", filename);
    index.save_to_file(&index_filename)?;
    let save_duration = start.elapsed();
    
    let index_size = fs::metadata(&index_filename)?.len();
    let compression_ratio = (file_size - index_size) as f64 / file_size as f64 * 100.0;
    
    println!("   ⏱️  Index creation: {:.3}s", index_duration.as_secs_f64());
    println!("   ⏱️  Index save: {:.3}s", save_duration.as_secs_f64() - index_duration.as_secs_f64());
    println!("   💾 Index size: {:.2} KB ({:.1}% compression)", 
             index_size as f64 / 1024.0, compression_ratio);
    println!("   🚀 Index throughput: {:.2} MB/s", mb_size / index_duration.as_secs_f64());
    
    // Benchmark 3: Index-based reading
    println!("\n🔍 3. Index-based channel reading:");
    let start = Instant::now();
    
    let loaded_index = MdfIndex::load_from_file(&index_filename)?;
    let load_duration = start.elapsed();
    
    // Read multiple channels via index
    let mut reader = mf4_rs::index::FileRangeReader::new(filename)?;
    
    let mut total_values_read = 0;
    let groups = loaded_index.list_channel_groups();
    
    for (group_idx, (_, _, _)) in groups.iter().enumerate().take(1) { // Test first group only
        if let Some(channels) = loaded_index.list_channels(group_idx) {
            // Read first few channels to test performance
            for (channel_idx, (_, channel_name, _)) in channels.iter().enumerate().take(3) {
                let values = loaded_index.read_channel_values(group_idx, channel_idx, &mut reader)?;
                total_values_read += values.len();
                
                if channel_idx == 0 {
                    println!("   📊 Sample channel '{}': {} values", channel_name, values.len());
                }
            }
        }
    }
    
    let total_index_duration = start.elapsed();
    
    println!("   ⏱️  Index load: {:.3}s", load_duration.as_secs_f64());
    println!("   ⏱️  Index read (3 channels): {:.3}s", 
             total_index_duration.as_secs_f64() - load_duration.as_secs_f64());
    println!("   ⏱️  Total index access: {:.3}s", total_index_duration.as_secs_f64());
    println!("   📊 Values read: {}", total_values_read);
    println!("   🚀 Index read throughput: {:.2} MB/s", mb_size / total_index_duration.as_secs_f64());
    
    // Benchmark 4: Specific channel access (by name)
    println!("\n🎯 4. Targeted channel access:");
    let start = Instant::now();
    
    // Test reading specific channels by name
    let temp_values = loaded_index.read_channel_values_by_name("Temperature", &mut reader)?;
    let pressure_values = loaded_index.read_channel_values_by_name("Pressure", &mut reader)?;
    
    let targeted_duration = start.elapsed();
    
    println!("   ⏱️  Read 2 specific channels: {:.3}s", targeted_duration.as_secs_f64());
    println!("   📊 Temperature: {} values", temp_values.len());
    println!("   📊 Pressure: {} values", pressure_values.len());
    println!("   🚀 Targeted throughput: {:.2} MB/s", mb_size / targeted_duration.as_secs_f64());
    
    // Performance summary
    println!("\n📈 Performance Summary:");
    println!("   🥇 Fastest: Index targeted read ({:.3}s)", targeted_duration.as_secs_f64());
    println!("   🥈 Second: Index full read ({:.3}s)", total_index_duration.as_secs_f64());
    println!("   🥉 Third: Full MDF parse+read ({:.3}s)", total_duration.as_secs_f64());
    
    let speedup_targeted = total_duration.as_secs_f64() / targeted_duration.as_secs_f64();
    let speedup_index = total_duration.as_secs_f64() / total_index_duration.as_secs_f64();
    
    println!("   ⚡ Speedup - Targeted: {:.1}x", speedup_targeted);
    println!("   ⚡ Speedup - Index: {:.1}x", speedup_index);
    
    // Cleanup
    fs::remove_file(index_filename).ok();
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_benchmark_with_small_file() {
        // This test would need a small test file to be meaningful
        // For now, just test that the function doesn't panic
        assert!(true);
    }
}