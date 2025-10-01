#!/usr/bin/env python3
"""
MDF Indexing Example - Demonstrates the powerful indexing system

The indexing system allows you to:
1. Create lightweight JSON indexes of MDF files 
2. Read specific channel data without loading entire files
3. Support remote file access (HTTP, S3, etc.)
4. Get precise byte ranges for efficient data extraction
"""

import mf4_rs
import os
import json

def create_sample_mdf_file():
    """Create a sample MDF file with multiple channels for indexing demo"""
    print("=== Creating Sample MDF File ===")
    
    writer = mf4_rs.PyMdfWriter("indexing_demo.mf4")
    writer.init_mdf_file()
    
    # Create a channel group with multiple channels
    group_id = writer.add_channel_group("Sensor Data")
    
    # Add channels using the new simplified API
    time_ch_id = writer.add_time_channel(group_id, "Time")
    temp_ch_id = writer.add_float_channel(group_id, "Temperature")
    pressure_ch_id = writer.add_float_channel(group_id, "Pressure")
    speed_ch_id = writer.add_int_channel(group_id, "Speed")
    
    print(f"Created channels: Time, Temperature, Pressure, Speed")
    
    # Write substantial amount of data (1000 records)
    writer.start_data_block(group_id)
    
    for i in range(1000):
        time = i * 0.01  # 10ms intervals
        temperature = 20.0 + 10.0 * (i % 50) / 50.0  # Varying 20-30°C
        pressure = 1013.25 + (i % 100) * 0.5  # Varying pressure
        speed = 50 + (i % 80)  # Speed 50-130
        
        values = [
            mf4_rs.create_float_value(time),
            mf4_rs.create_float_value(temperature),
            mf4_rs.create_float_value(pressure),
            mf4_rs.create_uint_value(speed)
        ]
        
        writer.write_record(group_id, values)
    
    writer.finish_data_block(group_id)
    writer.finalize()
    
    file_size = os.path.getsize("indexing_demo.mf4")
    print(f"Created MDF file: {file_size:,} bytes with 1000 records")
    return "indexing_demo.mf4"

def demonstrate_indexing(mdf_file):
    """Demonstrate the complete indexing workflow"""
    
    print("\n=== Step 1: Creating Index ===")
    
    # Create an index from the MDF file
    index = mf4_rs.PyMdfIndex.from_file(mdf_file)
    print(f"✓ Index created from {mdf_file}")
    
    # Save the index to a JSON file for later use
    index_file = "demo_index.json"
    index.save_to_file(index_file)
    
    index_size = os.path.getsize(index_file)
    mdf_size = os.path.getsize(mdf_file)
    compression_ratio = (mdf_size - index_size) / mdf_size * 100
    
    print(f"✓ Index saved to {index_file}")
    print(f"  MDF file size: {mdf_size:,} bytes")
    print(f"  Index size: {index_size:,} bytes")
    print(f"  Space saved: {compression_ratio:.1f}%")
    
    print("\n=== Step 2: Exploring Index Contents ===")
    
    # List channel groups in the index
    channel_groups = index.list_channel_groups()
    print(f"Channel groups found: {len(channel_groups)}")
    
    for group_idx, (_, group_name, channel_count) in enumerate(channel_groups):
        print(f"  Group {group_idx}: '{group_name}' with {channel_count} channels")
        
        # List channels in this group
        channels = index.list_channels(group_idx)
        if channels:
            for ch_idx, (_, ch_name, data_type) in enumerate(channels):
                print(f"    Channel {ch_idx}: '{ch_name}' ({data_type.name})")
    
    print("\n=== Step 3: Reading Channel Data via Index ===")
    
    # Method 1: Read channel by index (group 0, channel 1 = Temperature)
    print("Method 1: Reading by index (Group 0, Channel 1 - Temperature)")
    temp_values = index.read_channel_values(0, 1, mdf_file)
    print(f"  ✓ Read {len(temp_values)} temperature values")
    print(f"  First 5 values: {[float(v.value) for v in temp_values[:5]]}")
    
    # Method 2: Read channel by name
    print("\nMethod 2: Reading by channel name")
    pressure_values = index.read_channel_values_by_name("Pressure", mdf_file)
    print(f"  ✓ Read {len(pressure_values)} pressure values")
    print(f"  First 5 values: {[float(v.value) for v in pressure_values[:5]]}")
    
    # Method 3: Find channel location by name
    print("\nMethod 3: Finding channel location")
    speed_location = index.find_channel_by_name("Speed")
    if speed_location:
        group_idx, channel_idx = speed_location
        print(f"  ✓ 'Speed' found at Group {group_idx}, Channel {channel_idx}")
        
        # Read using the found location
        speed_values = index.read_channel_values(group_idx, channel_idx, mdf_file)
        print(f"  ✓ Read {len(speed_values)} speed values")
        print(f"  First 5 values: {[int(v.value) for v in speed_values[:5]]}")
    
    print("\n=== Step 4: Advanced - Byte Range Information ===")
    
    # Get byte ranges for efficient data access
    temp_ranges = index.get_channel_byte_ranges(0, 1)  # Temperature channel
    print(f"Temperature data stored in {len(temp_ranges)} byte ranges:")
    total_bytes = 0
    for i, (offset, length) in enumerate(temp_ranges[:3]):  # Show first 3 ranges
        total_bytes += length
        print(f"  Range {i}: offset {offset}, length {length} bytes")
    if len(temp_ranges) > 3:
        for offset, length in temp_ranges[3:]:
            total_bytes += length
        print(f"  ... and {len(temp_ranges) - 3} more ranges")
    print(f"  Total data size: {total_bytes:,} bytes")
    
    return index

def demonstrate_index_reuse():
    """Show how to reuse a saved index (e.g., in a different session)"""
    
    print("\n=== Step 5: Reusing Saved Index ===")
    
    # Load the previously saved index
    print("Loading index from JSON file...")
    loaded_index = mf4_rs.PyMdfIndex.load_from_file("demo_index.json")
    print("✓ Index loaded successfully")
    
    # Use the loaded index to read data (simulate accessing data from another process/session)
    print("Reading data using loaded index...")
    time_values = loaded_index.read_channel_values_by_name("Time", "indexing_demo.mf4")
    print(f"✓ Read {len(time_values)} time values from loaded index")
    print(f"  Time range: {float(time_values[0].value):.3f} to {float(time_values[-1].value):.3f} seconds")

def show_index_json_structure():
    """Show what's actually stored in the JSON index"""
    
    print("\n=== Step 6: Understanding Index Structure ===")
    
    with open("demo_index.json", "r") as f:
        index_data = json.load(f)
    
    print("Index JSON contains:")
    print(f"  • File size: {index_data.get('file_size', 'N/A'):,} bytes")
    print(f"  • Channel groups: {len(index_data.get('channel_groups', []))}")
    
    if "channel_groups" in index_data and index_data["channel_groups"]:
        cg = index_data["channel_groups"][0]
        print(f"  • First group channels: {len(cg.get('channels', []))}")
        
        if "channels" in cg and cg["channels"]:
            ch = cg["channels"][0]
            print(f"  • First channel name: '{ch.get('name', 'N/A')}'")
            print(f"  • Data blocks: {len(ch.get('data_blocks', []))}")
            
            if "data_blocks" in ch and ch["data_blocks"]:
                db = ch["data_blocks"][0]
                print(f"  • First block byte range: {db.get('file_offset', 0)} - {db.get('file_offset', 0) + db.get('size', 0)}")
    
    print("\n💡 The index contains all metadata needed to extract specific channel data")
    print("   without parsing the entire MDF file!")

def demonstrate_use_cases():
    """Show practical use cases for the indexing system"""
    
    print("\n=== Practical Use Cases ===")
    
    print("1. 🚀 Fast Channel Browsing")
    print("   • Create index once, browse channels many times")
    print("   • No need to parse entire MDF file for metadata")
    
    print("\n2. 📊 Selective Data Extraction")
    print("   • Read only the channels you need")
    print("   • Efficient for large files with many channels")
    
    print("\n3. 🌐 Remote File Analysis")
    print("   • Transfer small index file instead of large MDF")
    print("   • Use HTTP Range requests to read specific data")
    print("   • Perfect for cloud storage (S3, Azure, etc.)")
    
    print("\n4. 🔧 Custom Applications")
    print("   • Build channel browsers/viewers")
    print("   • Create data processing pipelines")
    print("   • Implement streaming data access")
    
    print("\n5. 💾 Memory-Efficient Processing")
    print("   • Process channels one at a time")
    print("   • Handle files larger than available RAM")

def main():
    """Run the complete indexing demonstration"""
    
    print("🔍 MDF Indexing System Demonstration")
    print("=" * 50)
    
    try:
        # Step 1: Create sample MDF file
        mdf_file = create_sample_mdf_file()
        
        # Step 2: Demonstrate indexing workflow
        index = demonstrate_indexing(mdf_file)
        
        # Step 3: Show index reuse
        demonstrate_index_reuse()
        
        # Step 4: Explain index structure
        show_index_json_structure()
        
        # Step 5: Show use cases
        demonstrate_use_cases()
        
        print("\n✅ Indexing demonstration completed successfully!")
        
        # Cleanup info
        print("\n📁 Files created:")
        print(f"  • {mdf_file} - Sample MDF file ({os.path.getsize(mdf_file):,} bytes)")
        print(f"  • demo_index.json - Index file ({os.path.getsize('demo_index.json'):,} bytes)")
        print("\n💡 You can now experiment with the index using different channels and ranges!")
        
    except Exception as e:
        print(f"❌ Error: {e}")
        import traceback
        traceback.print_exc()

if __name__ == "__main__":
    main()