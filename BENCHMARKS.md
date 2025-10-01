# MF4-RS Performance Benchmarking Suite

This document provides an overview of the comprehensive performance benchmarking capabilities included with mf4-rs.

## 🎯 Quick Navigation

- **[Main Benchmark Documentation](benchmarks/README.md)** - Comprehensive overview
- **[Rust Benchmarks](benchmarks/rust/README.md)** - Native Rust performance tests
- **[Python Benchmarks](benchmarks/python/README.md)** - Python binding tests
- **[Performance Analysis](benchmarks/analysis/README.md)** - Detailed analysis reports
- **[Test Data](benchmarks/data/README.md)** - Generated test files documentation
- **[Results & Logs](benchmarks/results/README.md)** - Benchmark results format

## 📊 Performance Summary

### Key Results (Windows 11, SSD)
| Metric | Rust | Python | Rust Advantage |
|--------|------|--------|----------------|
| **Single Channel Read** | ~150 MB/s | ~90 MB/s | **67% faster** |
| **Multi-Channel Read** | ~76 MB/s | ~45 MB/s | **69% faster** |
| **File Writing** | ~180 MB/s | ~85 MB/s | **112% faster** |
| **Index Creation** | ~200 MB/s | ~120 MB/s | **67% faster** |
| **Memory Efficiency** | 1.2x file size | 2.1x file size | **43% better** |

### Index-Based Reading Benefits
- **99%+ compression ratio**: Index files are 135-23,526x smaller
- **3-7x faster selective access**: Single channel reading performance
- **Sub-millisecond index loading**: Extremely fast startup
- **Excellent scaling**: Performance consistent across file sizes

## 🚀 Quick Start

### 1. Generate Test Data
```bash
# Create test files of various sizes (1MB to 500MB)
cargo run --example data_generator
```

### 2. Run Rust Benchmarks
```bash
# Core performance benchmarks
cargo run --release --example rust_performance_benchmark

# Index-based reading benchmarks
cargo run --release --example index_reading_benchmark
```

### 3. Run Python Benchmarks
```bash
# Ensure Python environment is set up
maturin develop

# Run Python benchmarks
cd benchmarks/python
python python_performance_benchmark.py
python index_read_benchmark.py
```

### 4. View Results
Results are automatically saved with timestamps and system information. Check:
- Console output for immediate results
- `benchmarks/results/` for detailed logs
- `benchmarks/analysis/` for comprehensive analysis reports

## 🔍 Benchmark Categories

### Core Performance Tests
- **File I/O**: Reading, writing, parsing performance
- **Memory Usage**: Peak and average consumption
- **Data Integrity**: Accuracy validation across operations
- **Scaling Behavior**: Performance across file sizes (1MB-500MB)

### Index-Based Reading Tests
- **Cold Start**: Load index from disk + read data
- **Warm Cache**: Read from pre-loaded index
- **Single vs Multi-Channel**: Access pattern optimization
- **Compression Analysis**: Space efficiency measurement

### Cross-Implementation Comparison
- **Rust vs Python**: Direct performance comparison
- **FFI Overhead**: Python binding performance cost analysis
- **Memory Efficiency**: Resource usage comparison
- **Use Case Optimization**: Scenario-specific recommendations

## 📈 Understanding Results

### Performance Interpretation
- **>100 MB/s**: Excellent performance, likely I/O bound
- **50-100 MB/s**: Good performance, balanced CPU/I/O
- **<50 MB/s**: Processing bound, optimization opportunities exist

### Memory Efficiency Guidelines
- **<2x file size**: Excellent memory efficiency
- **2-3x file size**: Good memory usage
- **>3x file size**: High memory usage, potential improvements needed

### When to Use Each Implementation
- **Choose Rust** for: Production systems, high-throughput processing, resource constraints
- **Choose Python** for: Data analysis, prototyping, integration with scientific computing
- **Use Index-Based Reading** for: Repeated access, selective data reading, large files

## 🛠️ Customization

### Adding New Benchmarks
1. **Rust**: Add new functions to benchmark files in `benchmarks/rust/`
2. **Python**: Add new functions to benchmark scripts in `benchmarks/python/`
3. **Documentation**: Update relevant README files with new benchmark descriptions

### Modifying Test Parameters
- **File sizes**: Edit `TEST_FILE_SIZES` arrays in benchmark files
- **Channel counts**: Modify channel configuration structs
- **Data patterns**: Customize sensor simulation patterns

### Platform Testing
The benchmarks are designed to work across platforms. Results will vary based on:
- **Operating System**: Windows/Linux/macOS differences
- **Storage Type**: SSD vs HDD performance impact
- **Hardware**: CPU, memory, and I/O subsystem capabilities

## 🔧 Advanced Usage

### CI/CD Integration
```yaml
# Example GitHub Actions workflow
- name: Performance Benchmarks
  run: |
    cargo run --release --example rust_performance_benchmark
    python benchmarks/python/python_performance_benchmark.py
    
- name: Check for Regressions
  run: |
    python scripts/analyze_performance_trends.py
```

### Historical Analysis
```python
# Load and analyze historical results
import pandas as pd
results = pd.read_json('benchmarks/results/history/summary.json')
trend_analysis = results.groupby('date')['throughput'].mean()
```

### Custom Visualization
```python
# Generate performance charts
import matplotlib.pyplot as plt
plt.figure(figsize=(10, 6))
plt.plot(dates, rust_throughput, label='Rust', marker='o')
plt.plot(dates, python_throughput, label='Python', marker='s')
plt.xlabel('Date')
plt.ylabel('Throughput (MB/s)')
plt.title('Performance Trends Over Time')
plt.legend()
plt.savefig('performance_trends.png')
```

## 📚 Detailed Documentation

### Analysis Reports
- **[Performance Comparison](benchmarks/analysis/PERFORMANCE_COMPARISON.md)**: Comprehensive Rust vs Python analysis
- **[Index Reading Analysis](benchmarks/analysis/INDEX_READING_PERFORMANCE_ANALYSIS.md)**: Deep dive into index-based reading

### Implementation Details
- **[Rust Benchmarks](benchmarks/rust/README.md)**: Native implementation benchmarking
- **[Python Benchmarks](benchmarks/python/README.md)**: Python binding performance analysis

### Data and Results
- **[Test Data](benchmarks/data/README.md)**: Generated test file specifications
- **[Results Format](benchmarks/results/README.md)**: Output format and analysis tools

## 🤝 Contributing

### Benchmark Contributions
1. Follow existing naming conventions and structure
2. Include both Rust and Python versions when possible
3. Add comprehensive documentation and analysis
4. Test across multiple file sizes and scenarios
5. Update relevant README files

### Result Sharing
Community benchmark results are welcome! Include:
- System specifications (OS, CPU, memory, storage)
- Software versions (Rust, Python, mf4-rs version)
- Methodology notes (any deviations from standard process)
- Analysis and insights from your testing

## 🏆 Optimization Wins

The benchmarking suite has already identified several optimization opportunities:
- **Index-based reading**: 3-7x improvement for selective data access
- **Memory efficiency**: Rust uses ~43% less memory than Python bindings
- **Consistent performance**: Rust maintains throughput across file sizes
- **Compression benefits**: 99%+ space savings with minimal performance cost

These insights directly inform development priorities and optimization efforts for the mf4-rs library.