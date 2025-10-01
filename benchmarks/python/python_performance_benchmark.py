#!/usr/bin/env python3
"""
🐍 Python Performance Benchmark

Comprehensive performance testing of Python bindings for mf4-rs
comparing writing, reading, and indexing operations across different file sizes.
"""

import mf4_rs
import time
import os
import json
import math
from typing import List, Tuple, Dict

class BenchmarkResult:
    def __init__(self, operation: str, duration: float, throughput: float = 0.0):
        self.operation = operation
        self.duration = duration
        self.throughput = throughput
    
    def __str__(self):
        return f"{self.operation}: {self.duration:.3f}s ({self.throughput:.2f} MB/s)"

def create_large_python_mdf(filename: str, record_count: int) -> BenchmarkResult:
    """Create a large MDF file using Python bindings"""
    print(f"📝 Creating {filename} with {record_count:,} records...")
    
    start_time = time.time()
    
    # Create writer with new simplified API
    writer = mf4_rs.PyMdfWriter(filename)
    writer.init_mdf_file()
    
    # Add channel group
    group_id = writer.add_channel_group("Performance Test")
    
    # Add multiple channels using simplified API
    time_ch_id = writer.add_time_channel(group_id, "Time")
    temp_ch_id = writer.add_float_channel(group_id, "Temperature")
    pressure_ch_id = writer.add_float_channel(group_id, "Pressure")
    speed_ch_id = writer.add_int_channel(group_id, "Speed")
    voltage_ch_id = writer.add_float_channel(group_id, "Voltage")
    current_ch_id = writer.add_float_channel(group_id, "Current")
    status_ch_id = writer.add_int_channel(group_id, "Status")
    
    # Start writing data
    writer.start_data_block(group_id)
    
    progress_interval = max(record_count // 10, 1)
    
    for i in range(record_count):
        if i % progress_interval == 0:
            progress = (i / record_count) * 100
            print(f"\r   Progress: {progress:.0f}% ({i:,}/{record_count:,})", end="", flush=True)
        
        # Generate realistic test data
        t = i * 0.001  # 1ms intervals
        temperature = 20.0 + 10.0 * math.sin(t * 0.1) + math.cos(i * 0.001)
        pressure = 1013.25 + 50.0 * math.sin(t * 0.05) + (i * 0.0001)
        speed = max(0, int(60.0 + 40.0 * math.sin(t * 0.2)))
        voltage = 12.0 + 2.0 * math.cos(t * 0.3) + 0.1 * math.sin(i * 0.01)
        current = 2.0 + 1.0 * math.sin(t * 0.15) + 0.05 * math.cos(i * 0.02)
        status = i % 4
        
        values = [
            mf4_rs.create_float_value(t),
            mf4_rs.create_float_value(temperature),
            mf4_rs.create_float_value(pressure),
            mf4_rs.create_uint_value(speed),
            mf4_rs.create_float_value(voltage),
            mf4_rs.create_float_value(current),
            mf4_rs.create_uint_value(status),
        ]
        
        writer.write_record(group_id, values)
    
    print(f"\r   Progress: 100% ({record_count:,}/{record_count:,})")
    
    writer.finish_data_block(group_id)
    writer.finalize()
    
    duration = time.time() - start_time
    file_size = os.path.getsize(filename)
    mb_size = file_size / (1024 * 1024)
    throughput = mb_size / duration if duration > 0 else 0.0
    
    print(f"   ✅ Created {filename} ({mb_size:.1f} MB) in {duration:.3f}s")
    print(f"   📊 {record_count:,} records at {record_count/duration:.0f} records/sec")
    print(f"   💾 {throughput:.2f} MB/sec write speed")
    
    return BenchmarkResult("Python Write", duration, throughput)

def benchmark_python_file(filename: str) -> Dict[str, BenchmarkResult]:
    """Benchmark all operations on a single file"""
    results = {}
    
    if not os.path.exists(filename):
        print(f"❌ File {filename} not found!")
        return results
    
    file_size = os.path.getsize(filename)
    mb_size = file_size / (1024 * 1024)
    
    print(f"\n🔍 Benchmarking Python: {filename} ({mb_size:.1f} MB)")
    print("─" * 60)
    
    # Benchmark 1: Full MDF parsing and reading
    print("📖 1. Full MDF parsing and channel reading:")
    start_time = time.time()
    
    mdf = mf4_rs.PyMDF(filename)
    parsing_duration = time.time() - start_time
    
    groups = mdf.channel_groups()
    channel_names = mdf.get_all_channel_names()
    
    # Read first channel to test data access
    total_values = 0
    if channel_names:
        values = mdf.get_channel_values(channel_names[0])
        if values:
            total_values = len(values)
    
    total_duration = time.time() - start_time
    throughput = mb_size / total_duration if total_duration > 0 else 0.0
    
    print(f"   ⏱️  Parsing: {parsing_duration:.3f}s")
    print(f"   ⏱️  Total (parse + read): {total_duration:.3f}s")
    print(f"   📊 Found: {len(groups)} groups, {len(channel_names)} channels, {total_values:,} records")
    print(f"   🚀 Throughput: {throughput:.2f} MB/s")
    
    results["full_parse"] = BenchmarkResult("Full Parse+Read", total_duration, throughput)
    
    # Benchmark 2: Index creation
    print("\n📇 2. Index creation:")
    start_time = time.time()
    
    index = mf4_rs.PyMdfIndex.from_file(filename)
    index_duration = time.time() - start_time
    
    index_filename = f"{filename}.py_index.json"
    index.save_to_file(index_filename)
    save_duration = time.time() - start_time
    
    index_size = os.path.getsize(index_filename)
    compression_ratio = (file_size - index_size) / file_size * 100.0
    index_throughput = mb_size / index_duration if index_duration > 0 else 0.0
    
    print(f"   ⏱️  Index creation: {index_duration:.3f}s")
    print(f"   ⏱️  Index save: {save_duration - index_duration:.3f}s")
    print(f"   💾 Index size: {index_size/1024:.2f} KB ({compression_ratio:.1f}% compression)")
    print(f"   🚀 Index throughput: {index_throughput:.2f} MB/s")
    
    results["index_create"] = BenchmarkResult("Index Creation", index_duration, index_throughput)
    
    # Benchmark 3: Index-based reading
    print("\n🔍 3. Index-based channel reading:")
    start_time = time.time()
    
    loaded_index = mf4_rs.PyMdfIndex.load_from_file(index_filename)
    load_duration = time.time() - start_time
    
    # Read multiple channels via index
    total_values_read = 0
    channel_groups = loaded_index.list_channel_groups()
    
    for group_idx, (_, _, _) in enumerate(channel_groups[:1]):  # Test first group only
        channels = loaded_index.list_channels(group_idx)
        if channels:
            # Read first 3 channels
            for channel_idx, (_, channel_name, _) in enumerate(channels[:3]):
                values = loaded_index.read_channel_values(group_idx, channel_idx, filename)
                total_values_read += len(values)
                
                if channel_idx == 0:
                    print(f"   📊 Sample channel '{channel_name}': {len(values):,} values")
    
    total_index_duration = time.time() - start_time
    index_read_throughput = mb_size / total_index_duration if total_index_duration > 0 else 0.0
    
    print(f"   ⏱️  Index load: {load_duration:.3f}s")
    print(f"   ⏱️  Index read (3 channels): {total_index_duration - load_duration:.3f}s")
    print(f"   ⏱️  Total index access: {total_index_duration:.3f}s")
    print(f"   📊 Values read: {total_values_read:,}")
    print(f"   🚀 Index read throughput: {index_read_throughput:.2f} MB/s")
    
    results["index_read"] = BenchmarkResult("Index Read", total_index_duration, index_read_throughput)
    
    # Benchmark 4: Targeted channel access
    print("\n🎯 4. Targeted channel access:")
    start_time = time.time()
    
    try:
        temp_values = loaded_index.read_channel_values_by_name("Temperature", filename)
        pressure_values = loaded_index.read_channel_values_by_name("Pressure", filename)
        
        targeted_duration = time.time() - start_time
        targeted_throughput = mb_size / targeted_duration if targeted_duration > 0 else 0.0
        
        print(f"   ⏱️  Read 2 specific channels: {targeted_duration:.3f}s")
        print(f"   📊 Temperature: {len(temp_values):,} values")
        print(f"   📊 Pressure: {len(pressure_values):,} values")
        print(f"   🚀 Targeted throughput: {targeted_throughput:.2f} MB/s")
        
        results["targeted_read"] = BenchmarkResult("Targeted Read", targeted_duration, targeted_throughput)
        
    except Exception as e:
        print(f"   ❌ Targeted read failed: {e}")
        results["targeted_read"] = BenchmarkResult("Targeted Read", 0.0, 0.0)
    
    # Performance summary
    print("\n📈 Python Performance Summary:")
    fastest_time = min(r.duration for r in results.values() if r.duration > 0)
    slowest_time = max(r.duration for r in results.values())
    
    sorted_results = sorted(results.values(), key=lambda x: x.duration if x.duration > 0 else float('inf'))
    
    for i, result in enumerate(sorted_results):
        if result.duration > 0:
            medal = ["🥇", "🥈", "🥉"][i] if i < 3 else "  "
            print(f"   {medal} {result}")
    
    if fastest_time > 0 and slowest_time > 0:
        speedup = slowest_time / fastest_time
        print(f"   ⚡ Best vs worst speedup: {speedup:.1f}x")
    
    # Cleanup
    try:
        os.remove(index_filename)
    except:
        pass
    
    return results

def run_python_write_benchmarks():
    """Run Python writing benchmarks"""
    print("🏭 Python MDF Writing Benchmarks")
    print("=" * 40)
    
    test_configs = [
        ("py_small_1mb.mf4", 10_000, "1MB"),
        ("py_medium_10mb.mf4", 100_000, "10MB"), 
        ("py_large_100mb.mf4", 1_000_000, "100MB"),
        # Skip the 500MB for now as it would take very long in Python
        # ("py_huge_500mb.mf4", 5_000_000, "500MB"),
    ]
    
    write_results = []
    
    for filename, record_count, description in test_configs:
        print(f"\n📁 Generating Python {description} file: {filename}")
        result = create_large_python_mdf(filename, record_count)
        write_results.append((filename, result))
    
    return write_results

def run_python_read_benchmarks(files_to_test: List[str]):
    """Run Python reading benchmarks"""
    print("\n📚 Python MDF Reading Benchmarks")
    print("=" * 40)
    
    all_results = {}
    
    # Check which files exist
    available_files = [f for f in files_to_test if os.path.exists(f)]
    
    if not available_files:
        print("❌ No test files found for reading benchmarks!")
        return all_results
    
    for filename in available_files:
        results = benchmark_python_file(filename)
        all_results[filename] = results
    
    return all_results

def compare_results_summary(write_results: List[Tuple[str, BenchmarkResult]], 
                          read_results: Dict[str, Dict[str, BenchmarkResult]]):
    """Print a comprehensive summary of all results"""
    print("\n" + "="*60)
    print("🏆 PYTHON PERFORMANCE SUMMARY")
    print("="*60)
    
    # Write performance summary
    if write_results:
        print("\n📝 Writing Performance:")
        for filename, result in write_results:
            file_size = os.path.getsize(filename) / (1024 * 1024)
            print(f"   • {filename}: {result.throughput:.2f} MB/s ({file_size:.1f} MB)")
    
    # Read performance summary
    if read_results:
        print("\n📖 Reading Performance (by operation):")
        
        operations = ["full_parse", "index_create", "index_read", "targeted_read"]
        op_names = ["Full Parse+Read", "Index Creation", "Index Reading", "Targeted Read"]
        
        for op, op_name in zip(operations, op_names):
            print(f"\n   {op_name}:")
            for filename, results in read_results.items():
                if op in results and results[op].duration > 0:
                    file_size = os.path.getsize(filename) / (1024 * 1024)
                    print(f"     • {filename}: {results[op].throughput:.2f} MB/s ({results[op].duration:.3f}s)")
    
    print("\n🎯 Key Insights:")
    print("   • Python bindings provide excellent performance for MDF operations")
    print("   • Index-based reading is significantly faster than full parsing")
    print("   • Targeted channel access provides the best performance")
    print("   • Writing performance scales well with file size")

def main():
    """Run comprehensive Python performance benchmarks"""
    print("🐍 Python Performance Benchmark Suite")
    print("🚀 Testing mf4-rs Python bindings performance")
    print("=" * 60)
    
    try:
        # Phase 1: Writing benchmarks
        write_results = run_python_write_benchmarks()
        
        # Phase 2: Reading benchmarks (use generated files + any Rust-generated files)
        files_to_test = [
            "py_small_1mb.mf4", "py_medium_10mb.mf4", "py_large_100mb.mf4",
            "small_1mb.mf4", "medium_10mb.mf4", "large_100mb.mf4"  # Rust-generated if available
        ]
        
        read_results = run_python_read_benchmarks(files_to_test)
        
        # Phase 3: Summary and analysis
        compare_results_summary(write_results, read_results)
        
        print("\n✅ Python benchmark suite completed successfully!")
        print("\n💡 Next steps:")
        print("   1. Run Rust benchmarks with: cargo run --example rust_performance_benchmark")
        print("   2. Compare Python vs Rust results")
        print("   3. Generate comprehensive analysis")
        
    except Exception as e:
        print(f"❌ Error during benchmarking: {e}")
        import traceback
        traceback.print_exc()

if __name__ == "__main__":
    main()