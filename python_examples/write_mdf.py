#!/usr/bin/env python3
"""
Example: Writing MDF files with mf4-rs Python bindings

This example demonstrates how to:
1. Create a new MDF writer
2. Add channel groups and channels
3. Write data records
4. Finalize the file
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
        
        # Create data types for channels
        float_type = mf4_rs.create_data_type_float_le()
        uint_type = mf4_rs.create_data_type_uint_le()
        
        # Add a time channel (master channel)
        time_ch_id = writer.add_channel(
            group_id=group_id,
            name="Time",
            data_type=float_type,
            bit_count=64,
            master_channel_id=None  # No master for the time channel itself
        )
        print(f"Added time channel: {time_ch_id}")
        
        # Set the time channel as master
        writer.set_time_channel(time_ch_id)
        print("Set time channel as master")
        
        # Add data channels with the time channel as master
        temp_ch_id = writer.add_channel(
            group_id=group_id,
            name="Temperature",
            data_type=float_type,
            bit_count=32,
            master_channel_id=time_ch_id
        )
        print(f"Added temperature channel: {temp_ch_id}")
        
        speed_ch_id = writer.add_channel(
            group_id=group_id,
            name="Speed",
            data_type=uint_type,
            bit_count=32,
            master_channel_id=time_ch_id
        )
        print(f"Added speed channel: {speed_ch_id}")
        
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