use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;
use mf4_rs::index::{MdfIndex, FileRangeReader};
use mf4_rs::error::MdfError;

fn main() -> Result<(), MdfError> {
    // Step 1: Create a sample MDF file
    let mdf_path = "sample_for_indexing.mf4";
    let index_path = "sample_index.json";

    println!("Creating sample MDF file: {}", mdf_path);
    create_sample_mdf_file(mdf_path)?;

    // Step 2: Create an index from the MDF file
    println!("Creating index from MDF file...");
    let index = MdfIndex::from_file(mdf_path)?;
    
    // Step 3: Save the index to a JSON file
    println!("Saving index to: {}", index_path);
    index.save_to_file(index_path)?;

    // Step 4: Load the index from the JSON file (simulating a fresh start)
    println!("Loading index from JSON file...");
    let loaded_index = MdfIndex::load_from_file(index_path)?;

    // Step 5: Explore the index structure
    println!("\n=== Index Structure ===");
    println!("File size: {} bytes", loaded_index.file_size);
    
    // List channel groups
    println!("\nChannel Groups:");
    for (group_idx, group_name, channel_count) in loaded_index.list_channel_groups() {
        println!("  Group {}: {} ({} channels)", group_idx, group_name, channel_count);
        
        // List channels in each group
        if let Some(channels) = loaded_index.list_channels(group_idx) {
            for (ch_idx, ch_name, data_type) in channels {
                println!("    Channel {}: {} ({:?})", ch_idx, ch_name, data_type);
            }
        }
    }

    // Step 6: Read channel data using the index
    println!("\n=== Reading Channel Data ===");
    if !loaded_index.channel_groups.is_empty() && !loaded_index.channel_groups[0].channels.is_empty() {
        let mut reader = FileRangeReader::new(mdf_path)?;
        match loaded_index.read_channel_values(0, 0, &mut reader) {
            Ok(values) => {
                println!("Successfully read {} values from first channel", values.len());
                
                // Print first few values
                for (i, value) in values.iter().take(5).enumerate() {
                    println!("  Value {}: {:?}", i, value);
                }
                if values.len() > 5 {
                    println!("  ... ({} more values)", values.len() - 5);
                }
            }
            Err(e) => {
                println!("Failed to read channel values: {}", e);
                println!("Note: This might be expected if data blocks aren't properly indexed yet");
            }
        }
    }

    // Step 7: Display index metadata
    println!("\n=== Index Metadata ===");
    if let Some(channel_info) = loaded_index.get_channel_info(0, 0) {
        println!("First channel details:");
        println!("  Name: {:?}", channel_info.name);
        println!("  Unit: {:?}", channel_info.unit);
        println!("  Data type: {:?}", channel_info.data_type);
        println!("  Byte offset: {}", channel_info.byte_offset);
        println!("  Bit count: {}", channel_info.bit_count);
        println!("  Channel type: {}", channel_info.channel_type);
    }

    println!("\nExample completed successfully!");
    println!("Check the generated files:");
    println!("  - {}", mdf_path);
    println!("  - {}", index_path);

    Ok(())
}

fn create_sample_mdf_file(path: &str) -> Result<(), MdfError> {
    let mut writer = MdfWriter::new(path)?;
    writer.init_mdf_file()?;
    
    // Create a channel group with multiple channels
    let cg_id = writer.add_channel_group(None, |_cg| {
        // Can set channel group properties here if needed
    })?;
    
    // Add a time channel
    let time_ch_id = writer.add_channel(&cg_id, None, |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Time".to_string());
        ch.bit_count = 64;
    })?;
    writer.set_time_channel(&time_ch_id)?;
    
    // Add a temperature channel
    writer.add_channel(&cg_id, Some(&time_ch_id), |ch| {
        ch.data_type = DataType::FloatLE;
        ch.name = Some("Temperature".to_string());
        ch.bit_count = 32;
    })?;
    
    // Add a speed channel
    writer.add_channel(&cg_id, Some(&time_ch_id), |ch| {
        ch.data_type = DataType::UnsignedIntegerLE;
        ch.name = Some("Speed".to_string());
        ch.bit_count = 16;
    })?;
    
    // Write some sample data
    writer.start_data_block_for_cg(&cg_id, 0)?;
    
    for i in 0..100 {
        let time = i as f64 * 0.01; // 10ms intervals
        let temperature = 20.0 + 5.0 * (time * 2.0).sin(); // Varying temperature
        let speed = ((time * 10.0).sin() * 50.0 + 60.0) as u64; // Varying speed
        
        writer.write_record(&cg_id, &[
            DecodedValue::Float(time),
            DecodedValue::Float(temperature),
            DecodedValue::UnsignedInteger(speed),
        ])?;
    }
    
    writer.finish_data_block(&cg_id)?;
    writer.finalize()?;
    
    Ok(())
}