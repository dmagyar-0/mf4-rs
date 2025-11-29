#!/usr/bin/env python3
"""
Index Operations Example

Demonstrates key features of the enhanced MF4 index system in Python.
"""

import mf4_rs
import os

def main():
    print("🚀 Index Operations Example")
    print("=" * 40)
    
    # Create a simple test file
    mdf_file = "index_example.mf4"
    index_file = "index_example.json"
    
    try:
        # Clean up existing files
        for f in [mdf_file, index_file]:
            if os.path.exists(f):
                os.remove(f)
        
        print("1️⃣ Creating test MDF file...")
        create_simple_mdf(mdf_file)
        
        print("2️⃣ Creating enhanced index...")
        # Create enhanced index - this resolves all conversions automatically
        index = mf4_rs.PyMdfIndex.from_file(mdf_file)
        index.save_to_file(index_file)
        
        # Check if it's enhanced
        has_resolved = index.has_resolved_conversions()
        print(f"   Enhanced conversions: {has_resolved}")
        
        print("3️⃣ Using the enhanced index...")
        # Load from JSON
        loaded_index = mf4_rs.PyMdfIndex.load_from_file(index_file)
        
        # List available data
        groups = loaded_index.list_channel_groups()
        print(f"   Channel groups: {len(groups)}")
        
        for group_idx, group_name, channel_count in groups:
            print(f"   Group {group_idx}: {channel_count} channels")
            
            channels = loaded_index.list_channels(group_idx)
            if channels:
                for ch_idx, ch_name, data_type in channels:
                    print(f"     - {ch_name} ({data_type.name})")
        
        print("4️⃣ Reading data efficiently...")
        # Read by name - fastest way
        temp_values = loaded_index.read_channel_values_by_name("Temperature", mdf_file)
        print(f"   Temperature: {len(temp_values)} values")
        print(f"   Range: {temp_values[0]} to {temp_values[-1]}")
        
        print("5️⃣ HTTP optimization features...")
        # Find channel location
        temp_location = loaded_index.find_channel_by_name("Temperature")
        if temp_location:
            group_idx, channel_idx = temp_location
            
            # Get byte range info
            total_bytes, range_count = loaded_index.get_channel_byte_summary(group_idx, channel_idx)
            print(f"   Temperature data: {total_bytes} bytes in {range_count} ranges")
            
            # Get byte ranges for partial reading
            partial_ranges = loaded_index.get_channel_byte_ranges_for_records(group_idx, channel_idx, 0, 5)
            partial_bytes = sum(length for _, length in partial_ranges)
            savings = (1 - partial_bytes / total_bytes) * 100
            
            print(f"   First 5 records: {partial_bytes} bytes ({savings:.1f}% bandwidth savings)")
            print(f"   HTTP range: bytes={partial_ranges[0][0]}-{partial_ranges[0][0] + partial_ranges[0][1] - 1}")
        
        print("\n✅ Enhanced index features demonstrated!")
        print("\n🎯 Key Benefits:")
        print("   • Index contains all data needed for conversions")
        print("   • Perfect for HTTP/remote file access")
        print("   • Precise byte range calculations")
        print("   • Fast name-based lookups")
        print("   • Much smaller than original MDF files")
        
        # Show file sizes
        mdf_size = os.path.getsize(mdf_file)
        index_size = os.path.getsize(index_file)
        print(f"\n📊 File Sizes:")
        print(f"   MDF:   {mdf_size:,} bytes")
        print(f"   Index: {index_size:,} bytes ({index_size/mdf_size*100:.1f}% of original)")
        
    except Exception as e:
        print(f"❌ Error: {e}")
        import traceback
        traceback.print_exc()
    
    finally:
        # Cleanup
        for f in [mdf_file, index_file]:
            if os.path.exists(f):
                os.remove(f)

def create_simple_mdf(file_path):
    """Create a simple MDF file for testing."""
    writer = mf4_rs.PyMdfWriter(file_path)
    writer.init_mdf_file()
    
    # Create channel group
    group = writer.add_channel_group("Test Data")
    
    # Add channels
    time_ch = writer.add_time_channel(group, "Time")
    temp_ch = writer.add_float_channel(group, "Temperature")
    rpm_ch = writer.add_int_channel(group, "RPM")
    
    # Write data
    writer.start_data_block(group)
    
    for i in range(20):
        time_val = mf4_rs.create_float_value(i * 0.1)
        temp_val = mf4_rs.create_float_value(20 + i * 2.5)  # Temperature 20-67.5°C
        rpm_val = mf4_rs.create_uint_value(1000 + i * 50)  # RPM 1000-1950
        
        writer.write_record(group, [time_val, temp_val, rpm_val])
    
    writer.finish_data_block(group)
    writer.finalize()

if __name__ == "__main__":
    main()