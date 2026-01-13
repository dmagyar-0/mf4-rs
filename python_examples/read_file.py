#!/usr/bin/env python3
"""
Example: Parsing MDF files with mf4-rs Python bindings

This example demonstrates how to:
1. Open an MDF file
2. Inspect channel groups and channels
3. Read channel values (returns native Python types)
4. Use pandas Series for data analysis
5. Look up channels by group name and channel name
"""

import mf4_rs

def main():
    # Parse an MDF file (assumes you have created one with write_file.py)
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
        # Returns native Python types (float, int, str, bytes) - no wrapper objects!
        if all_names:
            channel_name = all_names[0]
            print(f"\nReading values for channel '{channel_name}':")
            values = mdf.get_channel_values(channel_name)
            if values:
                print(f"Found {len(values)} values (native Python types!)")
                # Show first few values
                for i, value in enumerate(values[:5]):
                    # value is a native Python float/int/str/bytes, or None for invalid samples
                    print(f"  Value {i}: {value} (type: {type(value).__name__})")
                if len(values) > 5:
                    print(f"  ... and {len(values) - 5} more values")

                # Can use Python operations directly on values
                valid_values = [v for v in values if v is not None]
                if valid_values and isinstance(valid_values[0], (int, float)):
                    avg = sum(valid_values) / len(valid_values)
                    print(f"  Average (valid values): {avg:.2f}")
            else:
                print("No values found")

        # NEW: Read channel by group name + channel name (more precise lookup)
        print("\n--- Enhanced Channel Lookup ---")
        print("Looking up channel by group name + channel name...")
        # This is useful when multiple groups have channels with the same name
        first_group = channel_groups[0] if channel_groups else None
        if first_group and first_group.name and all_names:
            result = mdf.get_channel_values_by_group_and_name(first_group.name, all_names[0])
            if result:
                print(f"Found channel '{all_names[0]}' in group '{first_group.name}'")
                print(f"Values: {len(result)} samples")
            else:
                print(f"Channel not found in group '{first_group.name}'")

        # NEW: Get channel as pandas Series (requires pandas)
        print("\n--- Pandas Integration ---")
        try:
            import pandas as pd
            print("pandas is installed - trying Series conversion...")

            if all_names:
                series = mdf.get_channel_as_series(all_names[0])
                if series is not None:
                    print(f"Successfully converted '{all_names[0]}' to pandas Series!")
                    print(f"  Series length: {len(series)}")
                    print(f"  Index (time): {series.index[0]:.3f} to {series.index[-1]:.3f}")
                    if isinstance(series.iloc[0], (int, float)):
                        print(f"  Value range: {series.min():.2f} to {series.max():.2f}")
                        print(f"  Mean: {series.mean():.2f}")
                    print(f"  First 3 values:")
                    print(series.head(3))
                else:
                    print(f"Channel '{all_names[0]}' not found")
        except ImportError:
            print("pandas not installed - skipping Series examples")
            print("Install pandas with: pip install pandas")
        except Exception as e:
            print(f"Note: {e}")

    except mf4_rs.MdfException as e:
        print(f"MDF Error: {e}")
    except Exception as e:
        print(f"Error: {e}")

if __name__ == "__main__":
    main()