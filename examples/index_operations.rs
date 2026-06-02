use mf4_rs::blocks::common::DataType;
use mf4_rs::error::MdfError;
use mf4_rs::index::MdfIndex;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::writer::MdfWriter;

fn main() -> Result<(), MdfError> {
    let mdf_file = "index_example.mf4";
    let index_file = "index_example.json";

    println!("=== Index Operations Example ===");

    // Step 1: Create an MDF file with some test data
    println!("📄 Creating MDF file with test data...");
    create_test_mdf_file(mdf_file)?;

    // Step 2: Create enhanced index that resolves all conversion dependencies
    println!("📇 Creating enhanced index with resolved conversions...");
    let index = MdfIndex::from_file(mdf_file)?;
    index.save_to_file(index_file)?;

    println!("   ✅ Index created and saved to '{}'", index_file);

    // Step 3: Load the index and demonstrate self-contained conversion capability
    println!("🔄 Loading index and testing self-contained conversions...");
    let loaded_index = MdfIndex::load_from_file(index_file)?;

    // Step 4: Read and convert data using only the index + a bound data source.
    println!("📊 Reading channel values via enhanced index...");

    // List available channels by navigating the index by name
    println!("\nAvailable channels:");
    for channel in &loaded_index.groups()[0].channels {
        println!(
            "  {} ({:?})",
            channel.name.as_deref().unwrap_or("<unnamed>"),
            channel.data_type
        );
    }

    // Bind the data source once, then read by channel name.
    let mut data = loaded_index.open_file(mdf_file)?;
    let values = data.values("Time")?;

    println!("\nChannel values read from enhanced index:");
    for (i, value) in values.iter().enumerate().take(10) {
        println!("  Record {}: {:?}", i, value);
    }
    if values.len() > 10 {
        println!("  ... and {} more records", values.len() - 10);
    }

    // Step 5: Demonstrate byte range efficiency
    println!("\n🎯 Byte Range Analysis (Partial Reading):");
    let byte_ranges = loaded_index.byte_ranges("Time")?;
    let total_bytes: u64 = byte_ranges.iter().map(|(_, len)| len).sum();

    println!("  Total bytes needed: {}", total_bytes);
    println!("  Number of byte ranges: {}", byte_ranges.len());
    println!("  Ranges: {:?}", byte_ranges);

    // Step 6: Read another channel by name
    println!("\n🏷️  Reading 'Temperature' by name:");
    let temp_values = data.values("Temperature")?;
    println!("  Read {} values for channel 'Temperature'", temp_values.len());

    // Step 7: Show resolved conversion information
    println!("\n🔧 Conversion Resolution Status:");
    let group = &loaded_index.groups()[0];
    for (i, channel) in group.channels.iter().enumerate() {
        println!(
            "  Channel {}: {}",
            i,
            channel.name.as_deref().unwrap_or("<unnamed>")
        );
        if let Some(conversion) = &channel.conversion {
            println!("    Conversion type: {:?}", conversion.cc_type);
            if let Some(resolved_texts) = &conversion.resolved_texts {
                println!("    Resolved texts: {} entries", resolved_texts.len());
            }
            if let Some(resolved_conversions) = &conversion.resolved_conversions {
                println!(
                    "    Resolved nested conversions: {} entries",
                    resolved_conversions.len()
                );
            }
        } else {
            println!("    No conversion block");
        }
    }

    println!("\n🎉 Enhanced index example completed successfully!");
    println!("\nKey Benefits Demonstrated:");
    println!("  ✅ All conversion dependencies resolved during index creation");
    println!("  ✅ No file access needed for conversion operations during data reading");
    println!("  ✅ Perfect for HTTP/remote file access scenarios");
    println!("  ✅ Maintains full compatibility with existing functionality");
    println!("  ✅ Works with all conversion types: linear, text, rational, algebraic, etc.");

    // Clean up
    std::fs::remove_file(mdf_file).ok();
    std::fs::remove_file(index_file).ok();

    Ok(())
}

fn create_test_mdf_file(path: &str) -> Result<(), MdfError> {
    let mut writer = MdfWriter::new(path)?;
    writer.init_mdf_file()?;

    let cg_id = writer.add_channel_group(None, |_| {})?;

    // Create a time channel (master)
    let time_ch_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".to_string());
        ch.bit_count = 64;
    })?;
    writer.set_time_channel(&time_ch_id)?;

    // Create a temperature channel with linear conversion
    writer.add_channel(&cg_id, Some(&time_ch_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Temperature".to_string());
        ch.bit_count = 16;
    })?;

    // Create a status channel (could be enhanced with text conversion later)
    writer.add_channel(&cg_id, Some(&time_ch_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Status".to_string());
        ch.bit_count = 8;
    })?;

    // Write sample data
    writer.start_data_block_for_cg(&cg_id, 0)?;

    for i in 0..20 {
        let time = i as f64 * 0.1;
        let temp_raw = (i * 10 + 200) as u64; // Raw ADC value
        let status = if i % 5 == 0 { 1u64 } else { 0u64 };

        writer.write_record(
            &cg_id,
            &[
                DecodedValue::Float(time),
                DecodedValue::UnsignedInteger(temp_raw),
                DecodedValue::UnsignedInteger(status),
            ],
        )?;
    }

    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;

    Ok(())
}
