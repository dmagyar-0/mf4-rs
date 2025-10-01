#!/usr/bin/env python3
"""
Enhanced Index Python Example - Demonstrating Resolved Conversions

This example shows how to use the enhanced MF4 index system from Python,
which pre-resolves all conversion dependencies for optimal performance
in HTTP/remote scenarios.
"""

import mf4_rs
import json
import os
import time
from typing import List, Tuple, Optional, Dict, Any

def create_test_mdf_file(file_path: str) -> None:
    """Create a test MDF file with various data types."""
    print("🔧 Creating test MDF file...")
    
    writer = mf4_rs.PyMdfWriter(file_path)
    writer.init_mdf_file()
    
    # Create a channel group
    group_id = writer.add_channel_group("Test Group")
    
    # Add time channel (master)
    time_ch = writer.add_time_channel(group_id, "Time")
    
    # Add various data channels
    temp_ch = writer.add_float_channel(group_id, "Temperature")
    speed_ch = writer.add_int_channel(group_id, "Speed")
    status_ch = writer.add_int_channel(group_id, "Status")
    
    # Start writing data
    writer.start_data_block(group_id)
    
    # Write sample data
    for i in range(50):
        time_val = mf4_rs.create_float_value(i * 0.1)
        temp_val = mf4_rs.create_float_value(20.0 + 10.0 * (i * 0.05))  # Simulated temperature
        speed_val = mf4_rs.create_uint_value(int(60 + 30 * (i / 50.0)))  # Speed from 60 to 90
        status_val = mf4_rs.create_uint_value(1 if i % 10 == 0 else 0)  # Status every 10th record
        
        writer.write_record(group_id, [time_val, temp_val, speed_val, status_val])
    
    writer.finish_data_block(group_id)
    writer.finalize()
    
    print(f"   ✅ Test MDF file created: {file_path}")

def demonstrate_enhanced_index(mdf_file: str, index_file: str) -> None:
    """Demonstrate the enhanced index functionality."""
    print("\n" + "="*60)
    print("🚀 ENHANCED INDEX DEMONSTRATION")
    print("="*60)
    
    # Step 1: Create enhanced index with resolved conversions
    print("\n📇 Step 1: Creating Enhanced Index")
    start_time = time.time()
    index = mf4_rs.PyMdfIndex.from_file(mdf_file)
    index_creation_time = time.time() - start_time
    print(f"   Index created in {index_creation_time:.3f}s")
    
    # Save to JSON
    index.save_to_file(index_file)
    print(f"   ✅ Enhanced index saved to: {index_file}")
    
    # Get file sizes for comparison
    mdf_size = os.path.getsize(mdf_file)
    index_size = os.path.getsize(index_file)
    compression_ratio = ((mdf_size - index_size) / mdf_size) * 100
    
    print(f"\n💾 File Size Comparison:")
    print(f"   MDF file:   {mdf_size:,} bytes ({mdf_size/1024:.1f} KB)")
    print(f"   Index file: {index_size:,} bytes ({index_size/1024:.1f} KB)")
    print(f"   Compression: {compression_ratio:.1f}% space savings")
    
    # Step 2: Load index and verify enhanced features
    print("\n🔄 Step 2: Loading Index and Testing Features")
    loaded_index = mf4_rs.PyMdfIndex.load_from_file(index_file)
    
    # Check if it's an enhanced index
    is_enhanced = loaded_index.has_resolved_conversions()
    print(f"   Enhanced index (resolved conversions): {'✅ Yes' if is_enhanced else '❌ No'}")
    
    print(f"   File size from index: {loaded_index.get_file_size():,} bytes")
    
    # Step 3: Analyze channel groups and channels
    print("\n📊 Step 3: Channel Structure Analysis")
    channel_groups = loaded_index.list_channel_groups()
    print(f"   Channel groups: {len(channel_groups)}")
    
    for group_idx, group_name, channel_count in channel_groups:
        print(f"   Group {group_idx}: '{group_name}' ({channel_count} channels)")
        
        channels = loaded_index.list_channels(group_idx)
        if channels:
            for ch_idx, ch_name, data_type in channels:
                print(f"     Channel {ch_idx}: '{ch_name}' ({data_type.name})")
                
                # Get conversion info
                conv_info = loaded_index.get_conversion_info(group_idx, ch_idx)
                if conv_info:
                    print(f"       Conversion: {conv_info.get('conversion_type', 'Unknown')}")
                    if 'resolved_texts' in conv_info:
                        texts_count = len(conv_info['resolved_texts'])
                        print(f"       Resolved texts: {texts_count} entries")
                    if conv_info.get('has_resolved_conversions', False):
                        print(f"       Has resolved nested conversions")
                else:
                    print(f"       No conversion")
    
    # Step 4: Demonstrate efficient data reading
    print("\n🎯 Step 4: Efficient Data Reading")
    
    # Read time channel data
    start_time = time.time()
    time_values = loaded_index.read_channel_values_by_name("Time", mdf_file)
    read_time = time.time() - start_time
    
    print(f"   Read {len(time_values)} Time values in {read_time:.4f}s")
    print(f"   First 5 values: {[str(val) for val in time_values[:5]]}")
    
    # Read temperature data
    temp_values = loaded_index.read_channel_values_by_name("Temperature", mdf_file)
    print(f"   Read {len(temp_values)} Temperature values")
    print(f"   Temperature range: {temp_values[0]} to {temp_values[-1]}")
    
    # Step 5: Byte range analysis for HTTP optimization
    print("\n🌐 Step 5: HTTP Optimization Analysis")
    
    # Find temperature channel
    temp_location = loaded_index.find_channel_by_name("Temperature")
    if temp_location:
        group_idx, channel_idx = temp_location
        
        # Get full byte ranges
        full_ranges = loaded_index.get_channel_byte_ranges(group_idx, channel_idx)
        total_bytes, range_count = loaded_index.get_channel_byte_summary(group_idx, channel_idx)
        
        print(f"   Temperature channel byte analysis:")
        print(f"   Total bytes needed: {total_bytes}")
        print(f"   Number of ranges: {range_count}")
        print(f"   Ranges: {full_ranges[:3]}{'...' if len(full_ranges) > 3 else ''}")
        
        # Demonstrate partial reading (first 10 records)
        partial_ranges = loaded_index.get_channel_byte_ranges_for_records(group_idx, channel_idx, 0, 10)
        partial_bytes = sum(length for _, length in partial_ranges)
        savings = ((total_bytes - partial_bytes) / total_bytes) * 100
        
        print(f"\n   Partial reading (first 10 records):")
        print(f"   Bytes for 10 records: {partial_bytes}")
        print(f"   Bandwidth savings: {savings:.1f}%")
        print(f"   HTTP ranges needed: {len(partial_ranges)}")
    
    # Step 6: Name-based search capabilities
    print("\n🔍 Step 6: Advanced Search Features")
    
    # Find all channels by name pattern
    all_temp_channels = loaded_index.find_all_channels_by_name("Temperature")
    print(f"   All 'Temperature' channels: {all_temp_channels}")
    
    # Get detailed channel info by name
    temp_info = loaded_index.get_channel_info_by_name("Temperature")
    if temp_info:
        group_idx, channel_idx, channel_info = temp_info
        print(f"   Temperature channel details:")
        print(f"     Group: {group_idx}, Channel: {channel_idx}")
        print(f"     Data type: {channel_info.data_type.name}")
        print(f"     Bit count: {channel_info.bit_count}")
        print(f"     Unit: {channel_info.unit}")
    
    # Get byte ranges by name (convenient method)
    try:
        temp_ranges_by_name = loaded_index.get_channel_byte_ranges_by_name("Temperature")
        print(f"   Byte ranges by name: {len(temp_ranges_by_name)} ranges")
    except Exception as e:
        print(f"   Byte ranges by name: Error - {e}")

def compare_with_direct_reading(mdf_file: str, index_file: str) -> None:
    """Compare index-based reading with direct MDF reading."""
    print("\n" + "="*60)
    print("⚡ PERFORMANCE COMPARISON")
    print("="*60)
    
    print("\n🐌 Direct MDF Reading:")
    start_time = time.time()
    mdf = mf4_rs.PyMDF(mdf_file)
    
    # Get channel names
    channel_names = mdf.get_all_channel_names()
    
    # Read temperature values directly
    temp_values_direct = mdf.get_channel_values("Temperature")
    direct_time = time.time() - start_time
    
    print(f"   Loaded MDF and read Temperature: {direct_time:.4f}s")
    print(f"   Available channels: {channel_names}")
    if temp_values_direct:
        print(f"   Temperature values: {len(temp_values_direct)}")
    
    print("\n🚀 Enhanced Index Reading:")
    start_time = time.time()
    index = mf4_rs.PyMdfIndex.load_from_file(index_file)
    temp_values_index = index.read_channel_values_by_name("Temperature", mdf_file)
    index_time = time.time() - start_time
    
    print(f"   Loaded index and read Temperature: {index_time:.4f}s")
    print(f"   Temperature values: {len(temp_values_index)}")
    
    # Compare results
    if temp_values_direct and len(temp_values_direct) == len(temp_values_index):
        print(f"   ✅ Results match: Both methods read {len(temp_values_index)} values")
        
        # Check if values are the same
        values_match = all(str(direct) == str(index) for direct, index 
                         in zip(temp_values_direct[:10], temp_values_index[:10]))
        print(f"   ✅ Value consistency: {'Identical' if values_match else 'Different'}")
    
    # Performance analysis
    if index_time > 0:
        speedup = direct_time / index_time
        print(f"\n📈 Performance Analysis:")
        print(f"   Direct reading: {direct_time:.4f}s")
        print(f"   Index reading:  {index_time:.4f}s")
        print(f"   Speedup: {speedup:.1f}x {'faster' if speedup > 1 else 'slower'}")

def demonstrate_http_scenario(index_file: str, mdf_file: str) -> None:
    """Simulate HTTP range request scenario."""
    print("\n" + "="*60)
    print("🌐 HTTP RANGE REQUEST SIMULATION")
    print("="*60)
    
    index = mf4_rs.PyMdfIndex.load_from_file(index_file)
    
    # Find Speed channel for demonstration
    speed_location = index.find_channel_by_name("Speed")
    if not speed_location:
        print("   ❌ Speed channel not found")
        return
    
    group_idx, channel_idx = speed_location
    
    print(f"\n📊 Speed Channel Analysis:")
    print(f"   Location: Group {group_idx}, Channel {channel_idx}")
    
    # Get full channel info
    total_bytes, range_count = index.get_channel_byte_summary(group_idx, channel_idx)
    full_ranges = index.get_channel_byte_ranges(group_idx, channel_idx)
    
    print(f"   Full data: {total_bytes} bytes in {range_count} ranges")
    
    # Simulate reading different record ranges
    scenarios = [
        (0, 10, "First 10 records"),
        (10, 20, "Records 10-29"),
        (30, 20, "Last 20 records"),
    ]
    
    print(f"\n🔍 HTTP Range Request Scenarios:")
    
    for start_record, record_count, description in scenarios:
        try:
            ranges = index.get_channel_byte_ranges_for_records(group_idx, channel_idx, start_record, record_count)
            range_bytes = sum(length for _, length in ranges)
            bandwidth_savings = ((total_bytes - range_bytes) / total_bytes) * 100
            
            print(f"\n   {description}:")
            print(f"     Records: {start_record} to {start_record + record_count - 1}")
            print(f"     Bytes needed: {range_bytes} ({bandwidth_savings:.1f}% savings)")
            print(f"     HTTP requests: {len(ranges)}")
            
            # Show the actual HTTP range headers that would be used
            if ranges:
                print(f"     Sample Range header:", end="")
                for i, (offset, length) in enumerate(ranges[:2]):
                    range_end = offset + length - 1
                    print(f" bytes={offset}-{range_end}", end="")
                    if i < len(ranges[:2]) - 1:
                        print(",", end="")
                if len(ranges) > 2:
                    print(f" ... +{len(ranges) - 2} more")
                else:
                    print()
        except Exception as e:
            print(f"   {description}: Error - {e}")

def main():
    """Main function demonstrating enhanced MF4 index functionality."""
    print("🎯 Enhanced MF4 Index Python Example")
    print("=====================================")
    print("This example demonstrates the enhanced index system with resolved conversions.")
    print("Perfect for HTTP/remote file access scenarios!\n")
    
    # File paths
    mdf_file = "python_enhanced_example.mf4"
    index_file = "python_enhanced_example.json"
    
    try:
        # Clean up any existing files
        for file_path in [mdf_file, index_file]:
            if os.path.exists(file_path):
                os.remove(file_path)
        
        # Step 1: Create test data
        create_test_mdf_file(mdf_file)
        
        # Step 2: Demonstrate enhanced index features
        demonstrate_enhanced_index(mdf_file, index_file)
        
        # Step 3: Performance comparison
        compare_with_direct_reading(mdf_file, index_file)
        
        # Step 4: HTTP scenario simulation
        demonstrate_http_scenario(index_file, mdf_file)
        
        print("\n" + "="*60)
        print("🎉 ENHANCED INDEX EXAMPLE COMPLETED!")
        print("="*60)
        print("\n✅ Key Benefits Demonstrated:")
        print("   • All conversion dependencies resolved during index creation")
        print("   • No file access needed for conversions during data reading")
        print("   • Optimal for HTTP/remote file scenarios")
        print("   • Precise byte range calculations for bandwidth optimization")
        print("   • Fast name-based channel lookups")
        print("   • Complete backward compatibility")
        
        print("\n🔧 Files created:")
        print(f"   • {mdf_file} - Test MDF file")
        print(f"   • {index_file} - Enhanced JSON index")
        
        print("\n💡 Usage Tips:")
        print("   • Create indexes once, use many times")
        print("   • Index files are portable and much smaller than MDF files")
        print("   • Use byte ranges for efficient HTTP partial content requests")
        print("   • Name-based access is faster than scanning channel groups")
        
    except Exception as e:
        print(f"\n❌ Error: {e}")
        import traceback
        traceback.print_exc()
    
    finally:
        # Clean up
        for file_path in [mdf_file, index_file]:
            if os.path.exists(file_path):
                os.remove(file_path)
                print(f"🧹 Cleaned up: {file_path}")

if __name__ == "__main__":
    main()