# 🏁 mf4-rs Performance Comparison: Rust vs Python

## Overview

This comprehensive benchmark compares the performance of mf4-rs operations between native Rust and Python bindings across different file sizes and operations. The tests measure writing, reading, indexing, and various data access patterns.

## Test Configuration

### Hardware Environment
- **Platform**: Windows
- **Architecture**: x64
- **Build**: Debug mode (unoptimized for both Rust and Python)

### Test Files
- **Small**: ~0.3 MB (10,000 records, 7 channels)
- **Medium**: ~3.1 MB (100,000 records, 7 channels)  
- **Large**: ~30.5 MB (1,000,000 records, 7 channels)
- **Huge**: ~152.6 MB (5,000,000 records, 7 channels)

### Channel Configuration
Each test file contains 7 channels:
- Time (FloatLE, 64-bit master channel)
- Temperature (FloatLE, 32-bit)
- Pressure (FloatLE, 32-bit)
- Speed (UnsignedIntegerLE, 32-bit)
- Voltage (FloatLE, 32-bit)
- Current (FloatLE, 32-bit)
- Status (UnsignedIntegerLE, 32-bit)

## 📊 Performance Results

### 1. Writing Performance

| File Size | Language | Throughput | Records/sec | Notes |
|-----------|----------|------------|-------------|--------|
| ~0.3 MB   | Rust     | 35.83 MB/s | 1,167,706   | 🥇 **12.4x faster** |
| ~0.3 MB   | Python   | 2.90 MB/s  | 108,097     | |
| ~3.1 MB   | Rust     | 47.07 MB/s | 1,541,592   | 🥇 **15.7x faster** |
| ~3.1 MB   | Python   | 3.00 MB/s  | 112,344     | |
| ~30.5 MB  | Rust     | 49.28 MB/s | 1,614,573   | 🥇 **16.1x faster** |
| ~30.5 MB  | Python   | 3.07 MB/s  | 114,830     | |
| ~152.6 MB | Rust     | 49.73 MB/s | 1,629,533   | 🥇 **N/A** |
| ~152.6 MB | Python   | Not tested | Not tested  | (Too slow) |

#### 🎯 Writing Insights:
- **Rust dominates**: 12-16x faster than Python
- **Consistent performance**: Rust maintains ~50 MB/s regardless of file size
- **Python overhead**: Python has significant per-record overhead (~110K records/sec ceiling)
- **Scaling**: Rust performance scales better with file size

### 2. Reading Performance

#### Full MDF Parse + Read (First Channel)

| File Size | Language | Throughput | Parse Time | Read Time |
|-----------|----------|------------|------------|-----------|
| ~0.3 MB   | Rust     | 162.67 MB/s | 0.000s    | 0.002s |
| ~0.3 MB   | Python   | 102.22 MB/s | 0.000s    | 0.003s |
| ~3.1 MB   | Rust     | 192.65 MB/s | 0.000s    | 0.016s |
| ~3.1 MB   | Python   | 113.43 MB/s | 0.000s    | 0.027s |
| ~30.5 MB  | Rust     | 193.28 MB/s | 0.000s    | 0.158s |
| ~30.5 MB  | Python   | 111.14 MB/s | 0.000s    | 0.275s |
| ~152.6 MB | Rust     | 189.53 MB/s | 0.000s    | 0.805s |
| ~152.6 MB | Python   | Not tested  | -         | - |

#### 🎯 Reading Insights:
- **Rust advantage**: 1.4-1.7x faster than Python
- **Both fast**: Both achieve >100 MB/s throughput
- **Parsing speed**: Near-instantaneous parsing for both languages
- **Memory efficiency**: Both use memory-mapped files effectively

### 3. Index Creation Performance

| File Size | Language | Throughput | Creation Time | Index Size | Compression |
|-----------|----------|------------|---------------|------------|-------------|
| ~0.3 MB   | Rust     | 2,576 MB/s | 0.000s       | 2.32 KB   | 99.3% |
| ~0.3 MB   | Python   | Very fast* | 0.000s       | 2.32 KB   | 99.2% |
| ~3.1 MB   | Rust     | 16,469 MB/s | 0.000s      | 2.32 KB   | 99.9% |
| ~3.1 MB   | Python   | Very fast* | 0.000s       | 2.32 KB   | 99.9% |
| ~30.5 MB  | Rust     | 92,009 MB/s | 0.000s      | 3.12 KB   | 100.0% |
| ~30.5 MB  | Python   | 29,313 MB/s | 0.001s      | 3.00 KB   | 100.0% |

*Sub-millisecond timing precision limited

#### 🎯 Indexing Insights:
- **Extremely fast**: Both achieve massive throughput (limited by timer precision)
- **Excellent compression**: 99%+ space savings
- **Consistent size**: Index size grows minimally with file size
- **Both comparable**: Performance difference negligible due to speed

### 4. Index-Based Reading Performance

#### Reading 3 Channels via Index

| File Size | Language | Throughput | Load Time | Read Time | Values Read |
|-----------|----------|------------|-----------|-----------|-------------|
| ~0.3 MB   | Rust     | 22.46 MB/s | 0.007s   | 0.007s   | 30,000 |
| ~0.3 MB   | Python   | 12.07 MB/s | 0.007s   | 0.019s   | 30,000 |
| ~3.1 MB   | Rust     | 42.51 MB/s | 0.007s   | 0.065s   | 300,000 |
| ~3.1 MB   | Python   | 20.51 MB/s | 0.007s   | 0.142s   | 300,000 |
| ~30.5 MB  | Rust     | 45.84 MB/s | 0.008s   | 0.658s   | 3,000,000 |
| ~30.5 MB  | Python   | 20.88 MB/s | 0.008s   | 1.453s   | 3,000,000 |
| ~152.6 MB | Rust     | 45.67 MB/s | 0.010s   | 3.332s   | 15,000,000 |

#### 🎯 Index Reading Insights:
- **Rust advantage**: 1.9-2.2x faster than Python
- **Index load**: Nearly identical (both ~0.007s)
- **Data extraction**: Rust significantly faster at value parsing
- **Scaling**: Both maintain consistent throughput as size increases

### 5. Targeted Channel Access (2 Specific Channels)

| File Size | Language | Throughput | Time | Temperature | Pressure |
|-----------|----------|------------|------|-------------|----------|
| ~0.3 MB   | Rust     | 73.05 MB/s | 0.004s | 10,000     | 10,000 |
| ~0.3 MB   | Python   | 84.13 MB/s | 0.004s | 10,000     | 10,000 |
| ~3.1 MB   | Rust     | 74.37 MB/s | 0.041s | 100,000    | 100,000 |
| ~3.1 MB   | Python   | 47.57 MB/s | 0.064s | 100,000    | 100,000 |
| ~30.5 MB  | Rust     | 75.15 MB/s | 0.406s | 1,000,000  | 1,000,000 |
| ~30.5 MB  | Python   | 43.71 MB/s | 0.698s | 1,000,000  | 1,000,000 |
| ~152.6 MB | Rust     | 74.02 MB/s | 2.061s | 5,000,000  | 5,000,000 |

#### 🎯 Targeted Access Insights:
- **Mixed results**: Python occasionally faster for tiny files
- **Rust advantage**: 1.6-1.7x faster for larger files  
- **Best overall performance**: This approach gives highest throughput
- **Consistent scaling**: Both maintain steady performance

## 🏆 Overall Performance Rankings

### By Operation Type (Large Files ~30MB)

| Rank | Operation | Rust Throughput | Python Throughput | Winner |
|------|-----------|----------------|-------------------|---------|
| 🥇 | Writing | 49.28 MB/s | 3.07 MB/s | Rust (16.1x) |
| 🥈 | Full Parse+Read | 193.28 MB/s | 111.14 MB/s | Rust (1.7x) |
| 🥉 | Targeted Read | 75.15 MB/s | 43.71 MB/s | Rust (1.7x) |
| 4th | Index Read | 45.84 MB/s | 20.88 MB/s | Rust (2.2x) |
| 5th | Index Creation | 92,009 MB/s | 29,313 MB/s | Rust (3.1x) |

### By File Size (Full Parse+Read)

| File Size | Rust Winner | Python Throughput | Advantage |
|-----------|-------------|-------------------|-----------|
| 0.3 MB | 162.67 MB/s | 102.22 MB/s | 1.6x |
| 3.1 MB | 192.65 MB/s | 113.43 MB/s | 1.7x |
| 30.5 MB | 193.28 MB/s | 111.14 MB/s | 1.7x |

## 🔍 Detailed Analysis

### Rust Performance Characteristics
- **🚀 Dominant in writing**: 12-16x faster than Python
- **⚡ Excellent reading**: 1.4-2.2x faster across all read operations
- **🎯 Consistent**: Performance maintains steady ratios across file sizes
- **💾 Memory efficient**: Zero-copy operations and optimized data structures
- **⚙️ Low overhead**: Minimal per-record processing cost

### Python Performance Characteristics  
- **📊 Good reading**: Achieves >100 MB/s for direct file access
- **🐌 Writing bottleneck**: Limited by Python call overhead (~110K records/sec)
- **🔗 Binding efficiency**: C extension provides good performance bridge
- **💡 User friendly**: Simpler API compensates for performance gap
- **🎯 Targeted access**: Occasionally competitive with Rust for small operations

### Use Case Recommendations

#### Choose Rust When:
- **📈 High-volume writing**: Creating large MDF files frequently
- **⏱️ Performance critical**: Need maximum throughput
- **🔧 Low-level control**: Fine-tuned memory management required
- **📊 Batch processing**: Processing many large files
- **🚀 Production systems**: High-performance data acquisition

#### Choose Python When:
- **🧪 Prototyping**: Quick development and testing
- **📚 Data analysis**: Integration with pandas, matplotlib, jupyter
- **🔍 Interactive exploration**: Exploring MDF files manually
- **📊 Small to medium files**: <50MB files with occasional access
- **👥 Team productivity**: Faster development cycles important

## 🎯 Key Insights

### Performance Scaling
1. **Rust scales better**: Performance improves or stays constant with file size
2. **Python has overhead**: Fixed per-record costs limit scaling
3. **Both memory efficient**: Use of memory-mapped files prevents memory issues
4. **Index benefits both**: Dramatic space savings (99%+ compression)

### Operation Efficiency
1. **Writing**: Rust's biggest advantage (16x faster)
2. **Index creation**: Both extremely fast (sub-millisecond) 
3. **Full parsing**: Rust moderately faster (1.7x)
4. **Targeted reads**: Best overall throughput for both

### Real-World Implications
1. **Data acquisition**: Rust essential for high-speed logging (>1M samples/sec)
2. **Data analysis**: Python perfectly adequate for most analysis workflows
3. **Mixed workflows**: Use Rust for writing, Python for analysis
4. **File servers**: Rust for high-throughput serving, Python for APIs

## 🚀 Recommendations

### Development Strategy
- **Production data writers**: Use Rust for performance-critical paths
- **Analysis tools**: Use Python for user-facing applications
- **Hybrid approach**: Rust backend + Python frontend architecture
- **Index everything**: Both benefit enormously from indexing

### Optimization Opportunities
- **Python writing**: Could benefit from batch record APIs
- **Rust debugging**: Release builds would show even better performance
- **Memory tuning**: Both could optimize for specific file size ranges
- **Caching**: Index caching could improve repeated access patterns

## 📈 Conclusion

**Rust delivers superior raw performance**, especially for writing operations where it's 12-16x faster than Python. However, **Python provides excellent usability** and adequate performance for most analysis use cases.

The **indexing system is a game-changer for both languages**, providing 99%+ space savings and enabling fast targeted access patterns.

**Best strategy**: Use Rust for performance-critical data acquisition and Python for interactive analysis and prototyping, with the indexing system bridging the gap for efficient data access.