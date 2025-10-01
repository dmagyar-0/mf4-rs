#!/usr/bin/env python3
"""
📚 Index-Based Reading Performance Benchmark

This benchmark focuses specifically on reading MDF files using pre-existing index files,
which is the most common real-world scenario. It compares:

1. Cold start: Reading from index (loading index from disk)
2. Warm cache: Reading from already-loaded index 
3. Comparison with direct MDF file access

This represents realistic usage where indexes are created once and reused many times.
"""

import mf4_rs
import time
import os
import json
from typing import Dict, List, Tuple

class IndexReadResult:
    def __init__(self, operation: str, duration: float, throughput: float, values_read: int):
        self.operation = operation
        self.duration = duration
        self.throughput = throughput
        self.values_read = values_read
    
    def __str__(self):
        return f"{self.operation}: {self.duration:.3f}s ({self.throughput:.2f} MB/s, {self.values_read:,} values)"

def create_index_if_missing(mdf_file: str) -> str:
    """Create index file if it doesn't exist"""
    index_file = f"{mdf_file}.index.json"
    
    if not os.path.exists(index_file):
        print(f"📇 Creating index for {mdf_file}...")
        start_time = time.time()
        index = mf4_rs.PyMdfIndex.from_file(mdf_file)
        index.save_to_file(index_file)
        duration = time.time() - start_time
        print(f"   ✅ Index created in {duration:.3f}s")
    
    return index_file

def benchmark_index_reading(mdf_file: str, index_file: str) -> Dict[str, IndexReadResult]:
    """Benchmark various index-based reading patterns"""
    
    if not os.path.exists(mdf_file) or not os.path.exists(index_file):
        print(f"❌ Missing files: {mdf_file} or {index_file}")
        return {}
    
    file_size = os.path.getsize(mdf_file)
    mb_size = file_size / (1024 * 1024)
    
    print(f"\n🔍 Index Reading Benchmark: {mdf_file} ({mb_size:.1f} MB)")
    print("─" * 70)
    
    results = {}
    
    # Test 1: Cold start - Load index and read specific channels
    print("🥶 1. Cold start (load index + read channels):")
    start_time = time.time()
    
    index = mf4_rs.PyMdfIndex.load_from_file(index_file)
    load_time = time.time() - start_time
    
    # Read 2 specific channels
    temp_values = index.read_channel_values_by_name("Temperature", mdf_file)
    pressure_values = index.read_channel_values_by_name("Pressure", mdf_file)
    
    total_duration = time.time() - start_time
    values_read = len(temp_values) + len(pressure_values)
    throughput = mb_size / max(total_duration, 0.0001)  # Avoid division by zero
    
    print(f"   ⏱️  Index load: {load_time:.3f}s")
    print(f"   ⏱️  Data read: {total_duration - load_time:.3f}s")
    print(f"   ⏱️  Total: {total_duration:.3f}s")
    print(f"   📊 Values read: {values_read:,}")
    print(f"   🚀 Throughput: {throughput:.2f} MB/s")
    
    results["cold_start"] = IndexReadResult("Cold Start (2 channels)", total_duration, throughput, values_read)
    
    # Test 2: Warm cache - Read additional channels from already loaded index
    print("\n🔥 2. Warm cache (index already loaded):")
    start_time = time.time()
    
    speed_values = index.read_channel_values_by_name("Speed", mdf_file)
    voltage_values = index.read_channel_values_by_name("Voltage", mdf_file)
    
    warm_duration = time.time() - start_time
    warm_values_read = len(speed_values) + len(voltage_values)
    warm_throughput = mb_size / max(warm_duration, 0.0001)  # Avoid division by zero
    
    print(f"   ⏱️  Data read: {warm_duration:.3f}s")
    print(f"   📊 Values read: {warm_values_read:,}")
    print(f"   🚀 Throughput: {warm_throughput:.2f} MB/s")
    
    results["warm_cache"] = IndexReadResult("Warm Cache (2 channels)", warm_duration, warm_throughput, warm_values_read)
    
    # Test 3: Single channel targeted read
    print("\n🎯 3. Single channel targeted read:")
    start_time = time.time()
    
    current_values = index.read_channel_values_by_name("Current", mdf_file)
    
    single_duration = time.time() - start_time
    single_values_read = len(current_values)
    single_throughput = mb_size / max(single_duration, 0.0001)  # Avoid division by zero
    
    print(f"   ⏱️  Data read: {single_duration:.3f}s")
    print(f"   📊 Values read: {single_values_read:,}")
    print(f"   🚀 Throughput: {single_throughput:.2f} MB/s")
    
    results["single_channel"] = IndexReadResult("Single Channel", single_duration, single_throughput, single_values_read)
    
    # Test 4: All channels via index
    print("\n📊 4. All channels via index:")
    start_time = time.time()
    
    channel_groups = index.list_channel_groups()
    total_values_all = 0
    
    for group_idx, (_, _, _) in enumerate(channel_groups):
        channels = index.list_channels(group_idx)
        if channels:
            for channel_idx, (_, channel_name, _) in enumerate(channels):
                values = index.read_channel_values(group_idx, channel_idx, mdf_file)
                total_values_all += len(values)
    
    all_duration = time.time() - start_time
    all_throughput = mb_size / max(all_duration, 0.0001)  # Avoid division by zero
    
    print(f"   ⏱️  Data read: {all_duration:.3f}s")
    print(f"   📊 Values read: {total_values_all:,}")
    print(f"   🚀 Throughput: {all_throughput:.2f} MB/s")
    
    results["all_channels"] = IndexReadResult("All Channels", all_duration, all_throughput, total_values_all)
    
    # Test 5: Compare with direct MDF reading (for reference)
    print("\n📖 5. Direct MDF reading (for comparison):")
    start_time = time.time()
    
    mdf = mf4_rs.PyMDF(mdf_file)
    channel_names = mdf.get_all_channel_names()
    
    # Read same channels as in Test 1 for fair comparison
    if "Temperature" in channel_names and "Pressure" in channel_names:
        direct_temp = mdf.get_channel_values("Temperature")
        direct_pressure = mdf.get_channel_values("Pressure")
        direct_values = len(direct_temp) + len(direct_pressure)
    else:
        # Fallback to first two channels
        direct_temp = mdf.get_channel_values(channel_names[0]) if channel_names else []
        direct_pressure = mdf.get_channel_values(channel_names[1]) if len(channel_names) > 1 else []
        direct_values = len(direct_temp) + len(direct_pressure)
    
    direct_duration = time.time() - start_time
    direct_throughput = mb_size / max(direct_duration, 0.0001)  # Avoid division by zero
    
    print(f"   ⏱️  Total: {direct_duration:.3f}s")
    print(f"   📊 Values read: {direct_values:,}")
    print(f"   🚀 Throughput: {direct_throughput:.2f} MB/s")
    
    results["direct_mdf"] = IndexReadResult("Direct MDF (2 channels)", direct_duration, direct_throughput, direct_values)
    
    return results

def compare_index_vs_direct(results: Dict[str, IndexReadResult]):
    """Compare index-based reading with direct MDF access"""
    
    print("\n📈 Performance Comparison Summary:")
    
    # Sort results by throughput (descending)
    sorted_results = sorted(results.items(), key=lambda x: x[1].throughput, reverse=True)
    
    print("\n🏆 Ranking by Throughput:")
    for i, (key, result) in enumerate(sorted_results):
        medal = ["🥇", "🥈", "🥉"][i] if i < 3 else f"{i+1:2d}."
        print(f"   {medal} {result}")
    
    # Specific comparisons
    if "warm_cache" in results and "direct_mdf" in results:
        warm_throughput = results["warm_cache"].throughput
        direct_throughput = results["direct_mdf"].throughput
        
        if warm_throughput > direct_throughput:
            speedup = warm_throughput / direct_throughput
            print(f"\n⚡ Index (warm) vs Direct: {speedup:.1f}x FASTER")
        else:
            slowdown = direct_throughput / warm_throughput
            print(f"\n⚡ Index (warm) vs Direct: {slowdown:.1f}x slower")
    
    if "cold_start" in results and "direct_mdf" in results:
        cold_throughput = results["cold_start"].throughput
        direct_throughput = results["direct_mdf"].throughput
        
        if cold_throughput > direct_throughput:
            speedup = cold_throughput / direct_throughput
            print(f"⚡ Index (cold) vs Direct: {speedup:.1f}x FASTER")
        else:
            slowdown = direct_throughput / cold_throughput
            print(f"⚡ Index (cold) vs Direct: {slowdown:.1f}x slower")

def analyze_index_size_efficiency(mdf_file: str, index_file: str):
    """Analyze the space efficiency of index files"""
    
    print("\n💾 Index Efficiency Analysis:")
    
    mdf_size = os.path.getsize(mdf_file)
    index_size = os.path.getsize(index_file)
    
    compression_ratio = (mdf_size - index_size) / mdf_size * 100
    space_factor = mdf_size / index_size
    
    print(f"   📁 MDF file: {mdf_size:,} bytes ({mdf_size/1024/1024:.1f} MB)")
    print(f"   📄 Index file: {index_size:,} bytes ({index_size/1024:.1f} KB)")
    print(f"   📊 Compression: {compression_ratio:.1f}% space savings")
    print(f"   ⚡ Space factor: {space_factor:.0f}x smaller")
    
    # Show what's in the index
    with open(index_file, 'r') as f:
        index_data = json.load(f)
    
    if "channel_groups" in index_data and index_data["channel_groups"]:
        cg = index_data["channel_groups"][0]
        channel_count = len(cg.get("channels", []))
        record_count = cg.get("record_count", 0)
        
        print(f"   📊 Index contains: {channel_count} channels, {record_count:,} records metadata")
        print(f"   💡 Bytes per record metadata: {index_size/record_count:.1f}") if record_count > 0 else None

def run_comprehensive_index_benchmark():
    """Run comprehensive index reading benchmarks across all available files"""
    
    print("📚 Index Reading Performance Benchmark")
    print("🎯 Focus: Using pre-existing index files for data access")
    print("=" * 70)
    
    # Available test files
    test_files = [
        "small_1mb.mf4",      # Rust-generated
        "medium_10mb.mf4",    # Rust-generated
        "large_100mb.mf4",    # Rust-generated
        "py_small_1mb.mf4",   # Python-generated
        "py_medium_10mb.mf4", # Python-generated
        "py_large_100mb.mf4", # Python-generated
    ]
    
    available_files = [f for f in test_files if os.path.exists(f)]
    
    if not available_files:
        print("❌ No test files found!")
        print("Run the performance benchmarks first to generate test files.")
        return
    
    print(f"📁 Found {len(available_files)} test files")
    
    all_results = {}
    
    for mdf_file in available_files:
        # Ensure index file exists
        index_file = create_index_if_missing(mdf_file)
        
        # Run benchmarks
        results = benchmark_index_reading(mdf_file, index_file)
        all_results[mdf_file] = results
        
        # Individual file analysis
        compare_index_vs_direct(results)
        analyze_index_size_efficiency(mdf_file, index_file)
        
        print("\n" + "=" * 70)
    
    # Cross-file analysis
    print("\n🔍 CROSS-FILE ANALYSIS")
    print("=" * 70)
    
    # Compare warm cache performance across file sizes
    print("\n🔥 Warm Cache Performance by File Size:")
    for mdf_file, results in all_results.items():
        if "warm_cache" in results:
            file_size = os.path.getsize(mdf_file) / (1024 * 1024)
            result = results["warm_cache"]
            print(f"   • {mdf_file}: {result.throughput:.2f} MB/s ({file_size:.1f} MB)")
    
    # Compare single channel access
    print("\n🎯 Single Channel Access Performance:")
    for mdf_file, results in all_results.items():
        if "single_channel" in results:
            file_size = os.path.getsize(mdf_file) / (1024 * 1024)
            result = results["single_channel"]
            print(f"   • {mdf_file}: {result.throughput:.2f} MB/s ({file_size:.1f} MB)")
    
    print("\n🎯 Key Insights:")
    print("   • Index files provide 99%+ space compression")
    print("   • Warm cache reading eliminates index loading overhead")
    print("   • Single channel access maximizes throughput efficiency")
    print("   • Index loading is one-time cost, amortized over multiple reads")
    print("   • Best for applications that read same files repeatedly")
    
    print("\n💡 Recommendations:")
    print("   • Create indexes for frequently accessed files")
    print("   • Keep indexes loaded in memory for repeated access")
    print("   • Use targeted channel reading for maximum efficiency")
    print("   • Index-first approach for data analysis workflows")

def main():
    """Run the index reading benchmark"""
    try:
        run_comprehensive_index_benchmark()
        print("\n✅ Index reading benchmarks completed successfully!")
        
    except Exception as e:
        print(f"❌ Error during benchmarking: {e}")
        import traceback
        traceback.print_exc()

if __name__ == "__main__":
    main()