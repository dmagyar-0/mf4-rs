# MF4-RS Performance Benchmarks

This directory contains comprehensive benchmarks and performance analysis tools for the mf4-rs library, comparing Rust native performance with Python bindings across various operations.

## 📁 Directory Structure

```
benchmarks/
├── README.md                    # This file - benchmark overview
├── rust/                        # Native Rust benchmarks
│   ├── README.md               # Rust benchmark documentation
│   ├── data_generator.rs       # Generate test MDF files
│   ├── rust_performance_benchmark.rs    # Core Rust performance tests
│   └── index_reading_benchmark.rs       # Index-based reading benchmarks
├── python/                      # Python binding benchmarks
│   ├── README.md               # Python benchmark documentation
│   ├── python_performance_benchmark.py # Core Python performance tests
│   └── index_read_benchmark.py          # Index-based reading benchmarks
├── data/                        # Generated test data files
│   └── README.md               # Test data documentation
├── results/                     # Benchmark results and logs
│   └── README.md               # Results documentation
└── analysis/                    # Performance analysis reports
    ├── README.md               # Analysis documentation
    ├── PERFORMANCE_COMPARISON.md          # Core performance comparison
    └── INDEX_READING_PERFORMANCE_ANALYSIS.md  # Index performance analysis
```

## 🎯 Benchmark Categories

### 1. Core Performance Benchmarks
- **File I/O**: Reading, writing, parsing large MDF files
- **Memory Usage**: Peak memory consumption and efficiency
- **Throughput**: Data processing rates across file sizes
- **Scalability**: Performance with varying data complexity

### 2. Index-Based Reading Benchmarks  
- **Cold Start**: Loading index from disk + data access
- **Warm Cache**: Reading from pre-loaded index
- **Selective Access**: Single channel vs multi-channel reading
- **Space Efficiency**: Index compression and storage overhead

### 3. Data Generation Tools
- **Test File Generator**: Create MDF files of various sizes (1MB - 500MB)
- **Channel Patterns**: Realistic automotive sensor data patterns
- **Performance Validation**: Measure generation speed and file integrity

## 🚀 Quick Start

### Prerequisites
```bash
# Rust toolchain
cargo --version

# Python environment with mf4-rs bindings
pip install maturin
maturin develop

# For data analysis (optional)
pip install pandas matplotlib numpy
```

### Running Basic Benchmarks

1. **Generate Test Data**:
   ```bash
   # Rust generator
   cargo run --bin benchmarks/rust/data_generator

   # Or Python generator
   cd benchmarks/python && python python_performance_benchmark.py
   ```

2. **Run Rust Benchmarks**:
   ```bash
   # Core performance
   cargo run --bin benchmarks/rust/rust_performance_benchmark

   # Index reading
   cargo run --bin benchmarks/rust/index_reading_benchmark
   ```

3. **Run Python Benchmarks**:
   ```bash
   cd benchmarks/python
   
   # Core performance
   python python_performance_benchmark.py
   
   # Index reading
   python index_read_benchmark.py
   ```

## 📊 Key Metrics Measured

### Performance Metrics
- **Throughput**: MB/s for various operations
- **Duration**: Operation completion times
- **Memory**: Peak and average memory usage
- **CPU**: Processing efficiency

### Quality Metrics
- **Accuracy**: Data integrity validation
- **Compression**: Index space efficiency
- **Reliability**: Error rates and edge cases

## 🏆 Benchmark Results Summary

> **Last Updated**: October 2025  
> **Environment**: Windows 11, SSD storage, 16GB RAM

### Rust vs Python Performance
| Operation | Rust (MB/s) | Python (MB/s) | Rust Advantage |
|-----------|-------------|---------------|----------------|
| **Single Channel Read** | ~150 | ~90 | **67% faster** |
| **Multi-Channel Read** | ~76 | ~45 | **69% faster** |
| **File Writing** | ~180 | ~85 | **112% faster** |
| **Index Creation** | ~200 | ~120 | **67% faster** |

### Index Efficiency
- **Compression Ratio**: 99%+ space savings
- **Access Speed**: 3-7x faster for selective reading
- **Memory Overhead**: <0.1 bytes per record

## 🔧 Customizing Benchmarks

### Adding New Test Cases
1. Create new benchmark function in appropriate file
2. Follow existing naming conventions (`benchmark_operation_name`)
3. Include timing, memory, and accuracy measurements
4. Update documentation with new results

### Modifying Test Parameters
```rust
// Rust example - customize in benchmark files
const TEST_SIZES: &[usize] = &[1_000_000, 10_000_000, 100_000_000];
const CHANNEL_COUNTS: &[usize] = &[5, 10, 20, 50];
```

```python
# Python example - customize in benchmark files
TEST_SIZES = [1_000_000, 10_000_000, 100_000_000]
CHANNEL_COUNTS = [5, 10, 20, 50]
```

## 📈 Understanding Results

### Throughput Interpretation
- **>100 MB/s**: Excellent performance, I/O bound
- **50-100 MB/s**: Good performance, mixed CPU/I/O
- **<50 MB/s**: Processing bound, optimization needed

### When to Use Each Implementation
- **Rust**: Production systems, high-throughput processing, resource-constrained environments
- **Python**: Data analysis, prototyping, integration with scientific computing stack

## 🤝 Contributing

When adding benchmarks:
1. Include both Rust and Python versions for comparison
2. Test across multiple file sizes and complexities  
3. Document methodology and interpretation guidelines
4. Update analysis reports with new findings

## 📚 Further Reading

- [Performance Comparison Analysis](analysis/PERFORMANCE_COMPARISON.md)
- [Index Reading Performance Analysis](analysis/INDEX_READING_PERFORMANCE_ANALYSIS.md)
- [Rust Benchmark Details](rust/README.md)
- [Python Benchmark Details](python/README.md)