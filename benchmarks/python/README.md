# Python Binding Benchmarks

This directory contains Python benchmarks for the mf4-rs library bindings, measuring performance characteristics of the Python API and FFI overhead.

## 📁 Files Overview

### `python_performance_benchmark.py`
**Purpose**: Comprehensive Python performance benchmarking suite

**Features**:
- File I/O operations (read/write/parse)
- Channel data access patterns
- Memory usage tracking
- Cross-comparison with pure Python libraries
- Test data generation capability

**Usage**:
```bash
cd benchmarks/python
python python_performance_benchmark.py
```

**Dependencies**:
```bash
pip install mf4-rs pandas numpy matplotlib psutil
```

### `index_read_benchmark.py`  
**Purpose**: Index-based reading performance analysis

**Test Scenarios**:
- Cold start: Load index + read data
- Warm cache: Pre-loaded index operations
- Single vs multi-channel access
- Index efficiency analysis

**Usage**:
```bash
cd benchmarks/python
python index_read_benchmark.py
```

## 🚀 Quick Start

### Environment Setup
```bash
# Create virtual environment
python -m venv venv
source venv/bin/activate  # or `venv\Scripts\activate` on Windows

# Install dependencies
pip install maturin
maturin develop

# Install optional analysis tools
pip install pandas matplotlib numpy psutil
```

### Running Benchmarks
```bash
# Navigate to benchmark directory
cd benchmarks/python

# Run core performance tests
python python_performance_benchmark.py

# Run index-specific tests
python index_read_benchmark.py

# Run all tests (if you create a runner script)
python run_all_benchmarks.py
```

## 📊 Performance Characteristics

### Typical Results (Windows 11, Python 3.11)

| Operation | Small (1MB) | Medium (10MB) | Large (100MB) | Notes |
|-----------|-------------|---------------|---------------|-------|
| **File Reading** | 76 MB/s | 63 MB/s | 64 MB/s | Direct MDF access |
| **Single Channel** | 122 MB/s | 91 MB/s | 91 MB/s | Index-based |
| **Warm Index Read** | 44 MB/s | 46 MB/s | 45 MB/s | Multi-channel |
| **Cold Index Read** | 43 MB/s | 45 MB/s | 46 MB/s | Load + read |

### FFI Overhead Analysis
- **Function Call Overhead**: ~0.1-0.5ms per FFI call
- **Data Transfer**: ~10-20% performance impact for large arrays
- **Memory Allocation**: Python object creation overhead
- **Type Conversion**: Rust → Python type mapping costs

## 🔧 Environment Configuration

### Python Version Requirements
```bash
# Minimum supported
python --version  # >= 3.8

# Recommended for performance
python --version  # >= 3.11 (faster FFI, better performance)
```

### Memory Settings
```python
# Adjust for large file testing
import sys
sys.setrecursionlimit(5000)  # If needed for deep parsing

# Monitor memory usage
import psutil
import gc

# Example memory tracking in benchmarks
def track_memory():
    process = psutil.Process()
    return process.memory_info().rss / 1024 / 1024  # MB
```

### Performance Optimization
```python
# Disable garbage collection during timing
import gc

def benchmark_operation():
    gc.disable()
    try:
        # Timing code here
        pass
    finally:
        gc.enable()
```

## 📈 Interpreting Results

### Throughput Metrics
- **>80 MB/s**: Excellent for Python bindings
- **50-80 MB/s**: Good performance, typical range
- **<50 MB/s**: May indicate FFI bottlenecks or optimization opportunities

### Memory Usage Patterns
```python
# Memory efficiency check
def analyze_memory_efficiency(file_size_mb, peak_memory_mb):
    ratio = peak_memory_mb / file_size_mb
    
    if ratio < 2.0:
        return "Excellent - minimal memory overhead"
    elif ratio < 3.0:
        return "Good - reasonable memory usage"
    else:
        return "High memory usage - check for memory leaks"
```

### Error Rate Analysis
```python
# Data integrity validation
def validate_data_integrity(original, processed):
    if len(original) != len(processed):
        return False
    
    # Check for data corruption
    import numpy as np
    correlation = np.corrcoef(original, processed)[0,1]
    return correlation > 0.999  # 99.9% correlation threshold
```

## 🐛 Common Issues & Solutions

### Import Errors
```python
# Issue: mf4_rs module not found
# Solution: Ensure maturin develop was run
try:
    import mf4_rs
except ImportError:
    print("❌ mf4_rs not installed. Run: maturin develop")
    sys.exit(1)
```

### Memory Issues
```python
# Issue: Memory usage growing during benchmarks
# Solution: Explicit cleanup
def cleanup_test_data():
    import gc
    gc.collect()  # Force garbage collection
    
    # Clear large objects explicitly
    large_data = None
    del large_data
```

### Performance Inconsistency
```python
# Solution: Multiple iterations with warm-up
def stable_benchmark(operation_func, iterations=5):
    # Warm-up run (not counted)
    operation_func()
    
    times = []
    for _ in range(iterations):
        start = time.time()
        operation_func()
        times.append(time.time() - start)
    
    # Remove outliers and average
    times.sort()
    middle_times = times[1:-1]  # Remove min/max
    return sum(middle_times) / len(middle_times)
```

## 📊 Data Analysis & Visualization

### Basic Performance Plotting
```python
import matplotlib.pyplot as plt
import numpy as np

def plot_performance_comparison(rust_results, python_results):
    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12, 5))
    
    # Throughput comparison
    file_sizes = ['1MB', '10MB', '100MB', '500MB']
    x = np.arange(len(file_sizes))
    
    ax1.bar(x - 0.2, rust_results, 0.4, label='Rust', alpha=0.8)
    ax1.bar(x + 0.2, python_results, 0.4, label='Python', alpha=0.8)
    ax1.set_xlabel('File Size')
    ax1.set_ylabel('Throughput (MB/s)')
    ax1.set_title('Performance Comparison')
    ax1.legend()
    
    # Efficiency ratio
    ratios = [r/p for r, p in zip(rust_results, python_results)]
    ax2.plot(file_sizes, ratios, 'o-', linewidth=2, markersize=8)
    ax2.set_ylabel('Rust/Python Ratio')
    ax2.set_title('Performance Ratio')
    ax2.grid(True, alpha=0.3)
    
    plt.tight_layout()
    plt.savefig('performance_comparison.png', dpi=300, bbox_inches='tight')
    plt.show()
```

### Statistical Analysis
```python
import pandas as pd
from scipy import stats

def analyze_benchmark_statistics(results_df):
    """Analyze benchmark results with statistical methods"""
    
    summary = results_df.groupby('operation').agg({
        'throughput': ['mean', 'std', 'min', 'max'],
        'duration': ['mean', 'std'],
        'memory_usage': 'mean'
    }).round(2)
    
    print("📊 Statistical Summary:")
    print(summary)
    
    # Confidence intervals
    for operation in results_df['operation'].unique():
        data = results_df[results_df['operation'] == operation]['throughput']
        mean_val = data.mean()
        confidence_interval = stats.t.interval(
            confidence=0.95, 
            df=len(data)-1, 
            loc=mean_val, 
            scale=stats.sem(data)
        )
        print(f"{operation}: {mean_val:.1f} ± {abs(confidence_interval[1] - mean_val):.1f} MB/s (95% CI)")
```

## 🔧 Customization Examples

### Adding Custom Metrics
```python
class CustomBenchmarkResult:
    def __init__(self):
        self.operation = ""
        self.duration = 0.0
        self.throughput = 0.0
        self.memory_peak = 0.0
        self.cpu_usage = 0.0
        self.custom_metric = 0.0  # Add your own metrics
    
    def __str__(self):
        return f"{self.operation}: {self.duration:.3f}s ({self.throughput:.2f} MB/s, {self.memory_peak:.1f} MB peak)"

# Usage in benchmark
def benchmark_with_custom_metrics(operation_func):
    result = CustomBenchmarkResult()
    
    # Start monitoring
    start_memory = track_memory()
    start_time = time.time()
    
    # Run operation
    operation_result = operation_func()
    
    # Collect metrics
    result.duration = time.time() - start_time
    result.memory_peak = track_memory()
    result.custom_metric = calculate_custom_metric(operation_result)
    
    return result
```

### Test Data Customization
```python
# Generate custom test patterns
def generate_custom_test_data(pattern_type="automotive"):
    patterns = {
        "automotive": {
            "RPM": lambda t: 2000 + 1000 * np.sin(t * 0.1),
            "Speed": lambda t: 80 + 20 * np.sin(t * 0.05),
            "Temperature": lambda t: 90 + 10 * np.sin(t * 0.02)
        },
        "industrial": {
            "Pressure": lambda t: 15 + 5 * np.sin(t * 0.3),
            "Flow": lambda t: 100 + 25 * np.cos(t * 0.2),
            "Vibration": lambda t: np.random.normal(0, 0.1)
        }
    }
    
    return patterns.get(pattern_type, patterns["automotive"])
```

## 🤝 Contributing

### Adding New Benchmarks
1. **Follow naming convention**: `benchmark_[operation]_[variant]()`
2. **Include error handling**: Try/except with meaningful error messages
3. **Add timing precision**: Use `time.perf_counter()` for high precision
4. **Memory tracking**: Include memory usage measurements
5. **Data validation**: Verify results accuracy

### Code Style Guidelines
```python
def benchmark_new_operation(file_path: str, iterations: int = 3) -> BenchmarkResult:
    """
    Benchmark a new operation with proper documentation.
    
    Args:
        file_path: Path to test MDF file
        iterations: Number of test iterations
        
    Returns:
        BenchmarkResult with timing and performance metrics
        
    Raises:
        FileNotFoundError: If test file doesn't exist
        ValueError: If iterations < 1
    """
    if iterations < 1:
        raise ValueError("Iterations must be >= 1")
    
    if not os.path.exists(file_path):
        raise FileNotFoundError(f"Test file not found: {file_path}")
    
    # Implementation...
    return result
```

### Testing Your Benchmarks
```python
# Unit test example for benchmark functions
import unittest

class TestBenchmarks(unittest.TestCase):
    def setUp(self):
        # Create minimal test file
        self.test_file = "test_small.mf4"
        # Generate test file...
    
    def test_benchmark_accuracy(self):
        result = benchmark_file_reading(self.test_file)
        
        # Verify result structure
        self.assertIsNotNone(result.duration)
        self.assertGreater(result.throughput, 0)
        
        # Verify reasonable performance bounds
        self.assertLess(result.duration, 10.0)  # Should complete in <10s
        
    def tearDown(self):
        # Cleanup test files
        if os.path.exists(self.test_file):
            os.remove(self.test_file)
```