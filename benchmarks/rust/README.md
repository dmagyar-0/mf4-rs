# Rust Native Benchmarks

This directory contains native Rust benchmarks for the mf4-rs library, providing baseline performance measurements without FFI overhead.

## 📁 Files Overview

### `data_generator.rs`
**Purpose**: Generate test MDF files of various sizes for benchmarking

**Features**:
- Creates files from 1MB to 500MB with realistic data patterns
- 7 channels with different data types (float, integer, boolean)
- Automotive sensor simulation (Temperature, Pressure, Speed, etc.)
- Performance metrics during generation

**Usage**:
```bash
cargo run --example data_generator
```

**Generated Files**:
- `small_1mb.mf4` (1MB, 10K samples)
- `medium_10mb.mf4` (10MB, 100K samples)  
- `large_100mb.mf4` (100MB, 1M samples)
- `huge_500mb.mf4` (500MB, 5M samples)

### `rust_performance_benchmark.rs` 
**Purpose**: Core performance benchmarking suite for Rust implementation

**Test Categories**:
1. **File Reading**: Parse and load MDF files
2. **Channel Access**: Extract individual channel data
3. **Memory Usage**: Peak and average consumption
4. **Write Performance**: Create new MDF files
5. **Index Creation**: Generate metadata indexes

**Metrics Measured**:
- Throughput (MB/s)
- Duration (seconds)
- Memory usage (MB)
- Data integrity validation

**Usage**:
```bash
cargo run --example rust_performance_benchmark
```

### `index_reading_benchmark.rs`
**Purpose**: Specialized benchmarks for index-based reading patterns

**Test Scenarios**:
1. **Cold Start**: Load index from disk + read data
2. **Warm Cache**: Read from pre-loaded index
3. **Single Channel**: Targeted channel access
4. **All Channels**: Bulk reading via index
5. **Direct Comparison**: Index vs direct MDF reading

**Key Features**:
- Index compression analysis
- Performance scaling across file sizes
- Access pattern optimization insights

**Usage**:
```bash
cargo run --example index_reading_benchmark
```

## 🚀 Running All Benchmarks

### Sequential Execution
```bash
# Generate test data first
cargo run --example data_generator

# Run core benchmarks
cargo run --example rust_performance_benchmark

# Run index benchmarks
cargo run --example index_reading_benchmark
```

### Batch Script
Create `run_all_rust_benchmarks.sh` (or `.bat` for Windows):
```bash
#!/bin/bash
echo "🔧 Generating test data..."
cargo run --example data_generator

echo "📊 Running core performance benchmarks..."
cargo run --example rust_performance_benchmark > results/rust_core_$(date +%Y%m%d_%H%M%S).log

echo "🔍 Running index benchmarks..."  
cargo run --example index_reading_benchmark > results/rust_index_$(date +%Y%m%d_%H%M%S).log

echo "✅ All Rust benchmarks completed!"
```

## 📈 Performance Characteristics

### Typical Results (Windows 11, SSD)

| Operation | Small (1MB) | Medium (10MB) | Large (100MB) | Huge (500MB) |
|-----------|-------------|---------------|---------------|--------------|
| **File Reading** | 125 MB/s | 130 MB/s | 134 MB/s | 130 MB/s |
| **Single Channel** | 154 MB/s | 155 MB/s | 150 MB/s | 151 MB/s |
| **Warm Index Read** | 80 MB/s | 77 MB/s | 77 MB/s | 76 MB/s |
| **Cold Index Read** | 73 MB/s | 75 MB/s | 76 MB/s | 75 MB/s |

### Key Observations
- **Consistent Performance**: Throughput remains stable across file sizes
- **Single Channel Dominance**: Best performance for selective access
- **Index Efficiency**: 99%+ compression with fast access
- **Memory Efficiency**: Minimal overhead, predictable usage patterns

## 🔧 Customization

### Modifying Test Parameters

Edit the benchmark files to customize:

```rust
// File sizes to test
const TEST_FILE_SIZES: &[&str] = &[
    "small_1mb.mf4",
    "medium_10mb.mf4", 
    "large_100mb.mf4",
    "huge_500mb.mf4"
];

// Channel configurations
const CHANNEL_CONFIGS: &[ChannelConfig] = &[
    ChannelConfig { name: "Temperature", data_type: DataType::Float, samples: 10000 },
    ChannelConfig { name: "Pressure", data_type: DataType::Float, samples: 10000 },
    // Add more channels...
];
```

### Adding New Benchmarks

1. **Create New Function**:
```rust
fn benchmark_new_operation() -> Result<BenchmarkResult, MdfError> {
    let start = Instant::now();
    
    // Your operation here
    
    let duration = start.elapsed();
    Ok(BenchmarkResult {
        operation: "New Operation".to_string(),
        duration: duration.as_secs_f64(),
        throughput: calculate_throughput(data_size, duration),
        // ...
    })
}
```

2. **Add to Main Loop**:
```rust
fn main() -> Result<(), MdfError> {
    // Existing benchmarks...
    
    let new_result = benchmark_new_operation()?;
    println!("📊 {}", new_result);
    
    Ok(())
}
```

## 🐛 Troubleshooting

### Common Issues

**"File not found" errors**:
```bash
# Ensure test data exists
cargo run --example data_generator
```

**Compilation errors**:
```bash
# Clean rebuild
cargo clean
cargo build --examples
```

**Performance inconsistency**:
- Close other applications during benchmarking
- Run multiple iterations for stable averages
- Check disk space (>2GB free recommended)

### Debug Mode vs Release Mode

**Development**:
```bash
cargo run --example rust_performance_benchmark
```

**Performance Testing** (recommended):
```bash
cargo run --release --example rust_performance_benchmark
```

Release mode provides ~2-5x performance improvement and more realistic results.

## 📊 Output Format

### Console Output
```
🔧 Rust Performance Benchmark Suite
=====================================

📁 Testing file: large_100mb.mf4 (30.5 MB)
├── 📖 Direct file reading: 0.227s (134.23 MB/s)
├── 🎯 Single channel access: 0.203s (150.23 MB/s)  
├── 📊 All channels access: 1.472s (20.74 MB/s)
└── 💾 Memory usage: Peak 45.2 MB, Average 23.1 MB

✅ Benchmarks completed successfully!
```

### Log File Format
Detailed logs saved to `results/` directory with:
- Timestamp and system info
- Complete test results with metadata
- Error logs and warnings
- Performance trends and analysis

## 🤝 Contributing

When adding Rust benchmarks:

1. **Follow Naming Convention**: `benchmark_[operation]_[variant].rs`
2. **Include Error Handling**: Proper `Result<T, MdfError>` usage
3. **Add Documentation**: Rustdoc comments for public functions
4. **Performance Focus**: Optimize for realistic usage patterns
5. **Cross-platform**: Test on Windows, Linux, macOS if possible

### Code Style
```rust
/// Benchmarks XYZ operation across different file sizes
/// 
/// # Returns
/// - `BenchmarkResult` with timing and throughput data
/// 
/// # Errors  
/// - Returns `MdfError` if file operations fail
fn benchmark_xyz_operation(file_path: &str) -> Result<BenchmarkResult, MdfError> {
    // Implementation
}
```