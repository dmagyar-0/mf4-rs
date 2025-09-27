#!/usr/bin/env python3
"""
Example: Parsing MDF files with mf4-rs Python bindings

This example demonstrates how to:
1. Open an MDF file
2. Inspect channel groups and channels
3. Read channel values
"""

import mf4_rs

def main():
    # Parse an MDF file (assumes you have created one with the Rust examples)
    try:
        mdf = mf4_rs.PyMDF("example.mf4")
        print("Successfully opened MDF file")
        
        # Get all channel groups
        channel_groups = mdf.channel_groups()
        print(f"Found {len(channel_groups)} channel groups")
        
        for i, group in enumerate(channel_groups):
            print(f"\nChannel Group {i}:")
            print(f"  Name: {group.name}")
            print(f"  Comment: {group.comment}")
            print(f"  Channel Count: {group.channel_count}")
            print(f"  Record Count: {group.record_count}")
            
            # Get channels for this group
            channels = mdf.get_channels_for_group(i)
            for j, channel in enumerate(channels):
                print(f"  Channel {j}:")
                print(f"    Name: {channel.name}")
                print(f"    Unit: {channel.unit}")
                print(f"    Data Type: {channel.data_type}")
                print(f"    Bit Count: {channel.bit_count}")
        
        # Get all channel names across all groups
        all_names = mdf.get_all_channel_names()
        print(f"\nAll channel names: {all_names}")
        
        # Read values for a specific channel (if it exists)
        if all_names:
            channel_name = all_names[0]
            print(f"\nReading values for channel '{channel_name}':")
            values = mdf.get_channel_values(channel_name)
            if values:
                print(f"Found {len(values)} values")
                # Show first few values
                for i, value in enumerate(values[:5]):
                    print(f"  Value {i}: {value}")
                if len(values) > 5:
                    print(f"  ... and {len(values) - 5} more values")
            else:
                print("No values found")
        
    except mf4_rs.MdfException as e:
        print(f"MDF Error: {e}")
    except Exception as e:
        print(f"Error: {e}")

if __name__ == "__main__":
    main()