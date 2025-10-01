#!/usr/bin/env python3
"""
Example: Writing MDF files with mf4-rs Python bindings

This example demonstrates the new simplified API:
1. Create a new MDF writer
2. Add channel groups and channels (automatic linking & bit counts)
3. Write data records
4. Finalize the file

Features of the new API:
- Automatic sequential channel linking (no manual master_channel_id)
- Automatic bit count selection based on data type
- Convenience methods (add_time_channel, add_float_channel, add_int_channel)
- Automatic time/master channel setup
"""

import mf4_rs
import math

def main():
    try:
        # Create a new MDF writer
        writer = mf4_rs.PyMdfWriter("python_example.mf4")
        print("Created MDF writer")
        
        # Initialize the MDF file structure
        writer.init_mdf_file()
        print("Initialized MDF file")
        
        # Add a channel group
        group_id = writer.add_channel_group("Test Group")
        print(f"Added channel group: {group_id}")
        
        # Use the new simplified API - no manual linking or bit counts needed!
        
        # Add time channel (automatically sets as master channel)
        time_ch_id = writer.add_time_channel(group_id, "Time")
        print(f"Added time channel: {time_ch_id} (automatic: FloatLE 32-bit, master channel)")
        
        # Add data channels (automatically linked sequentially)
        temp_ch_id = writer.add_float_channel(group_id, "Temperature")
        print(f"Added temperature channel: {temp_ch_id} (automatic: FloatLE 32-bit, linked)")
        
        speed_ch_id = writer.add_int_channel(group_id, "Speed")
        print(f"Added speed channel: {speed_ch_id} (automatic: UnsignedIntegerLE 32-bit, linked)")
        
        # Start data block
        writer.start_data_block(group_id)
        print("Started data block")
        
        # Write sample data
        print("Writing data records...")
        for i in range(100):
            time = i * 0.01  # 10ms intervals
            temperature = 20.0 + 5.0 * math.sin(time * 2.0)  # Varying temperature
            speed = int(math.sin(time * 10.0) * 50.0 + 60.0)  # Varying speed
            
            # Create record values (must match the order channels were added)
            values = [
                mf4_rs.create_float_value(time),
                mf4_rs.create_float_value(temperature),
                mf4_rs.create_uint_value(speed)
            ]
            
            writer.write_record(group_id, values)
            
            if i % 20 == 0:
                print(f"  Wrote record {i}: time={time:.3f}, temp={temperature:.2f}, speed={speed}")
        
        # Finish data block
        writer.finish_data_block(group_id)
        print("Finished data block")
        
        # Finalize the writer
        writer.finalize()
        print("MDF file 'python_example.mf4' created successfully!")
        
        # Now try to read the file we just created
        print("\nTesting the created file:")
        test_read_created_file()
        
    except mf4_rs.MdfException as e:
        print(f"MDF Error: {e}")
    except Exception as e:
        print(f"Error: {e}")

def test_read_created_file():
    """Test reading the file we just created"""
    try:
        mdf = mf4_rs.PyMDF("python_example.mf4")
        
        # Get channel groups
        groups = mdf.channel_groups()
        print(f"Created file has {len(groups)} channel groups")
        
        if groups:
            group = groups[0]
            print(f"Group has {group.channel_count} channels and {group.record_count} records")
            
            # Get channel names
            names = mdf.get_all_channel_names()
            print(f"Channel names: {names}")
            
            # Read a few values from each channel
            for name in names[:3]:  # Limit to first 3 channels
                values = mdf.get_channel_values(name)
                if values and len(values) > 0:
                    print(f"Channel '{name}': first value = {values[0]}, last value = {values[-1]}")
        
    except Exception as e:
        print(f"Error reading created file: {e}")

if __name__ == "__main__":
    main()