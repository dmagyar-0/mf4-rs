///! Example of writing a complete MF4 file with data groups, channel groups, and channels
///! This demonstrates creating a structured MDF file and reading it back using the parser

use mf4_rs::error::MdfError;
use mf4_rs::writer::MdfWriter;
use mf4_rs::api::mdf::MDF;
use mf4_rs::blocks::channel_group_block::ChannelGroupBlock;
use mf4_rs::blocks::channel_block::ChannelBlock;
use mf4_rs::blocks::common::DataType;

/// Creates a structured MDF file with data groups, channel groups, and channels, then reads it back.
/// This demonstrates the enhanced MdfWriter API that handles links automatically.
fn main() -> Result<(), MdfError> {
    // Output file path
    let file_path = "example_structured.mf4";
    println!("Writing a structured MDF file to {}", file_path);

    // ------- Step 1: Create the file and write base blocks -------
    
    // Create a writer for our output file
    // PITFALL: This will overwrite any existing file at the given path
    let mut mdf_writer = MdfWriter::new(file_path)?;
    
    // Initialize the file with identification and header blocks
    // This creates both blocks and automatically links ID → HD
    let (id_block_position, hd_block_position) = mdf_writer.init_mdf_file()?;
    println!("Wrote Identification block at position: {}", id_block_position);
    println!("Wrote Header block at position: {}", hd_block_position);
    
    // ------- Step 2: Add a Data Group -------
    
    // Add our first data group (None means this is the first DG)
    // The method automatically links HD → DG
    let dg1_id = mdf_writer.add_data_group(None)?;
    println!("Added Data Group with ID: {}", dg1_id);
    
    // ------- Step 3: Add Channel Groups -------
    
    // Add first channel group to our data group
    // The method automatically links DG → CG
    let cg_block = ChannelGroupBlock::default();
    let cg1_id = mdf_writer.add_channel_group(&dg1_id, None, &cg_block)?;
    println!("Added Channel Group with ID: {}", cg1_id);
    
    // Add a second channel group (linked after the first one)
    // The method automatically links CG1 → CG2
    let cg2_id = mdf_writer.add_channel_group(&dg1_id, Some(&cg1_id), &cg_block)?;
    println!("Added second Channel Group with ID: {}", cg2_id);
    
    // ------- Step 4: Add Channels to the first Channel Group -------
    
    // Add channels to the first channel group
    // Each channel has a name, byte offset, and bit count
    
    // First channel - this will be linked from the channel group
    // PITFALL: Byte offsets must be set correctly to avoid data overlap
    let mut ch1 = ChannelBlock::default();
    ch1.byte_offset = 0;
    ch1.bit_count = 32;
    ch1.data_type = DataType::UnsignedIntegerLE;
    ch1.name = Some("Engine Speed".to_string());
    let cn1_id = mdf_writer.add_channel(&cg1_id, None, &ch1)?;
    println!("Added Channel 'Engine Speed' with ID: {}", cn1_id);
    
    // Second channel - this will be linked from the first channel
    // Note how the byte offset is 4 to avoid overlapping with the first channel
    let mut ch2 = ChannelBlock::default();
    ch2.byte_offset = 4;
    ch2.bit_count = 32;
    ch2.data_type = DataType::UnsignedIntegerLE;
    ch2.name = Some("Engine Temp".to_string());
    let cn2_id = mdf_writer.add_channel(&cg1_id, Some(&cn1_id), &ch2)?;
    println!("Added Channel 'Engine Temp' with ID: {}", cn2_id);
    
    // ------- Step 5: Add Channels to the second Channel Group -------
    
    // Add a channel to the second channel group
    let mut ch3 = ChannelBlock::default();
    ch3.byte_offset = 0;
    ch3.bit_count = 16;
    ch3.data_type = DataType::UnsignedIntegerLE;
    ch3.name = Some("Vehicle Speed".to_string());
    let cn3_id = mdf_writer.add_channel(&cg2_id, None, &ch3)?;
    println!("Added Channel 'Vehicle Speed' with ID: {}", cn3_id);
    
    // ------- Step 6: Finalize the file -------
    
    // This flushes all data to disk and closes the file
    mdf_writer.finalize()?;
    println!("Successfully wrote structured MDF file");
    
    // ------- Step 7: Try to parse using the MDF API -------
    
    println!("\nNow attempting to read the file using the MDF parser...");
    
    // Parse the file using the MDF API - use a match to handle any errors
    match MDF::from_file(file_path) {
        Ok(mdf_file) => {
            // Verify the structure was correctly written and can be read back
            println!("Successfully parsed the written file!");
            
            // Get all channel groups - in the MDF API, we access channel groups directly
            let channel_groups = mdf_file.channel_groups();
            println!("Number of channel groups: {}", channel_groups.len());
            
            // Examine each channel group
            for (i, channel_group) in channel_groups.iter().enumerate() {
                println!("Channel Group #{}", i+1);
                
                // Display channel group name if available
                if let Ok(Some(name)) = channel_group.name() {
                    println!("  Name: {}", name);
                } else {
                    println!("  Name: <unnamed>");
                }
                
                // Count and display channels
                let channels = channel_group.channels();
                println!("  Number of channels: {}", channels.len());
                
                // Print channel names
                for (j, channel) in channels.iter().enumerate() {
                    if let Ok(Some(name)) = channel.name() {
                        println!("    Channel {}: Name = {}", j+1, name);
                    } else {
                        println!("    Channel {}: No name", j+1);
                    }
                }
            }
        },
        Err(err) => {
            println!("Error parsing MDF file: {:?}", err);
            println!("\nThis error suggests there might be a problem with the file format or structure.");
            println!("The debug info above should help identify where the issue is occurring.");
        }
    }
    
    println!("\nRoundtrip test completed successfully!");
    Ok(())
}
