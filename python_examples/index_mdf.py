#!/usr/bin/env python3
"""
Example: MDF file indexing with mf4-rs Python bindings

This example demonstrates how to:
1. Create an index from an MDF file
2. Save and load indexes to/from JSON
3. Use indexes for fast channel data access
4. Get byte ranges for efficient I/O
"""

import mf4_rs
import json

def main():
    # First, create a test MDF file if it doesn't exist
    create_test_file_if_needed()
    
    try:
        print("=== Creating MDF Index ===")
        
        # Create index from MDF file
        index = mf4_rs.PyMdfIndex.from_file("index_test.mf4")
        print("Created index from MDF file")
        
        # Save index to JSON file
        index.save_to_file("index_test.json")
        print("Saved index to JSON file")
        
        # Load index from JSON file (simulating a fresh start)
        loaded_index = mf4_rs.PyMdfIndex.load_from_file("index_test.json")
        print("Loaded index from JSON file")
        
        print("\n=== Index Structure ===")
        
        # List all channel groups
        channel_groups = loaded_index.list_channel_groups()
        print(f"Channel groups: {len(channel_groups)}")
        
        for group_idx, group_name, channel_count in channel_groups:
            print(f"  Group {group_idx}: '{group_name}' ({channel_count} channels)")
            
            # List channels in this group
            channels = loaded_index.list_channels(group_idx)
            if channels:
                for ch_idx, ch_name, data_type in channels:
                    print(f"    Channel {ch_idx}: '{ch_name}' ({data_type})")
        
        print("\n=== Reading Channel Data via Index ===")
        
        # Find a channel by name
        channel_location = loaded_index.find_channel_by_name("Temperature")
        if channel_location:
            group_idx, ch_idx = channel_location
            print(f"Found 'Temperature' at group {group_idx}, channel {ch_idx}")
            
            # Read channel values using the index
            values = loaded_index.read_channel_values(group_idx, ch_idx, "index_test.mf4")
            print(f"Read {len(values)} values via index")
            
            # Show first few values
            for i, value in enumerate(values[:5]):
                print(f"  Value {i}: {value}")
            
            if len(values) > 5:
                print(f"  ... and {len(values) - 5} more values")
        
        print("\n=== Reading by Channel Name ===")
        
        # Read channel values directly by name
        try:
            values = loaded_index.read_channel_values_by_name("Speed", "index_test.mf4")
            print(f"Read {len(values)} values for 'Speed' channel")
            if values:
                print(f"  First value: {values[0]}")
                print(f"  Last value: {values[-1]}")
        except Exception as e:
            print(f"Error reading 'Speed' channel: {e}")
        
        print("\n=== Byte Range Information ===")
        
        # Get byte ranges for efficient I/O
        if channel_location:
            group_idx, ch_idx = channel_location
            try:
                byte_ranges = loaded_index.get_channel_byte_ranges(group_idx, ch_idx)
                total_bytes = sum(length for _, length in byte_ranges)
                print(f"Temperature channel byte ranges:")
                print(f"  Number of ranges: {len(byte_ranges)}")
                print(f"  Total bytes to read: {total_bytes}")
                
                # Show first few ranges
                for i, (offset, length) in enumerate(byte_ranges[:3]):
                    print(f"  Range {i}: offset={offset}, length={length}")
                
                if len(byte_ranges) > 3:
                    print(f"  ... and {len(byte_ranges) - 3} more ranges")
                    
            except Exception as e:
                print(f"Error getting byte ranges: {e}")
        
        print("\n=== Index vs Direct Comparison ===")
        
        # Compare index-based reading with direct MDF parsing
        try:
            # Direct MDF parsing
            direct_mdf = mf4_rs.PyMDF("index_test.mf4")
            direct_values = direct_mdf.get_channel_values("Temperature")
            
            # Index-based reading
            indexed_values = loaded_index.read_channel_values_by_name("Temperature", "index_test.mf4")
            
            if direct_values and indexed_values:
                print(f"Direct parsing: {len(direct_values)} values")
                print(f"Index-based: {len(indexed_values)} values")
                
                # Compare first few values
                match_count = 0
                for i in range(min(len(direct_values), len(indexed_values), 10)):
                    if str(direct_values[i]) == str(indexed_values[i]):
                        match_count += 1
                
                print(f"First 10 values match: {match_count}/10")
            
        except Exception as e:
            print(f"Error in comparison: {e}")
            
    except mf4_rs.MdfException as e:
        print(f"MDF Error: {e}")
    except Exception as e:
        print(f"Error: {e}")

def create_test_file_if_needed():
    """Create a test MDF file for indexing demonstration"""
    import os
    
    if os.path.exists("index_test.mf4"):
        print("Test file already exists")
        return
    
    print("Creating test file for indexing demo...")
    
    try:
        writer = mf4_rs.PyMdfWriter("index_test.mf4")
        writer.init_mdf_file()
        
        # Add channel group
        group_id = writer.add_channel_group("IndexTestGroup")
        
        # Create data types
        float_type = mf4_rs.create_data_type_float_le()
        uint_type = mf4_rs.create_data_type_uint_le()
        
        # Add time channel
        time_ch = writer.add_channel(group_id, "Time", float_type, 64, None)
        writer.set_time_channel(time_ch)
        
        # Add data channels
        temp_ch = writer.add_channel(group_id, "Temperature", float_type, 32, time_ch)
        speed_ch = writer.add_channel(group_id, "Speed", uint_type, 32, time_ch)
        
        # Write data
        writer.start_data_block(group_id)
        
        import math
        for i in range(50):
            time = i * 0.1
            temp = 25.0 + 10.0 * math.sin(time)
            speed = int(60 + 20 * math.cos(time * 0.5))
            
            values = [
                mf4_rs.create_float_value(time),
                mf4_rs.create_float_value(temp),
                mf4_rs.create_uint_value(speed)
            ]
            
            writer.write_record(group_id, values)
        
        writer.finish_data_block(group_id)
        writer.finalize()
        
        print("Test file 'index_test.mf4' created")
        
    except Exception as e:
        print(f"Error creating test file: {e}")

if __name__ == "__main__":
    main()