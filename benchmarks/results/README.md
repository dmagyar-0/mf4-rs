# Benchmark Results & Logs

This directory stores benchmark execution results, performance logs, and historical data for tracking performance trends over time.

## 📁 File Organization

```
results/
├── README.md                    # This file
├── latest/                      # Most recent benchmark runs
│   ├── rust_performance.log    # Latest Rust benchmark results
│   ├── python_performance.log  # Latest Python benchmark results
│   ├── index_reading.log       # Latest index benchmark results
│   └── summary.json            # Machine-readable summary
├── history/                     # Historical results archive
│   ├── 2025-10-01/             # Results by date
│   ├── 2025-09-30/
│   └── ...
├── comparisons/                 # Cross-run comparisons
│   ├── rust_vs_python.csv     # Comparative analysis data
│   ├── performance_trends.csv  # Historical trend data
│   └── regression_analysis.json # Performance regression tracking
└── reports/                     # Generated analysis reports
    ├── monthly_summary.html    # Human-readable reports
    ├── performance_charts/     # Generated visualizations
    └── anomaly_detection.log  # Performance outlier detection
```

## 📊 Log File Formats

### Standard Benchmark Log Format
```
🔧 Rust Performance Benchmark Suite
=====================================
📅 Timestamp: 2025-10-01 15:30:45 UTC
💻 System: Windows 11, Intel i7, 16GB RAM, NVMe SSD
🏷️  Version: mf4-rs v0.1.0
🔧 Build: Release mode, optimization level 3

📁 Testing file: large_100mb.mf4 (30.5 MB)
├── 📖 Direct file reading: 0.227s (134.23 MB/s, 2M values)
├── 🎯 Single channel access: 0.203s (150.23 MB/s, 1M values)
├── 📊 All channels access: 1.472s (20.74 MB/s, 7M values)
├── 💾 Memory usage: Peak 45.2 MB, Average 23.1 MB
└── ✅ Data integrity: 100% correlation

🎯 Summary Statistics:
├── Best throughput: 150.23 MB/s (Single channel)
├── Average throughput: 101.73 MB/s
├── Memory efficiency: 1.48x file size
└── Success rate: 100% (0 errors)

⏱️  Total benchmark duration: 2.1 seconds
✅ All tests completed successfully
```

### Machine-Readable JSON Format
```json
{
  "timestamp": "2025-10-01T15:30:45Z",
  "version": "0.1.0",
  "system": {
    "os": "Windows 11",
    "cpu": "Intel i7-12700",
    "memory": "16GB",
    "storage": "NVMe SSD"
  },
  "results": [
    {
      "test_name": "direct_file_reading",
      "file_size_mb": 30.5,
      "duration_seconds": 0.227,
      "throughput_mbps": 134.23,
      "values_processed": 2000000,
      "memory_peak_mb": 45.2,
      "memory_average_mb": 23.1,
      "success": true,
      "data_integrity": 1.0
    }
  ],
  "summary": {
    "total_tests": 5,
    "successful_tests": 5,
    "average_throughput": 101.73,
    "peak_throughput": 150.23,
    "total_duration": 2.1
  }
}
```

## 📈 Performance Tracking

### Trend Analysis
Historical performance data is automatically tracked to detect:
- **Performance Regressions**: Significant drops in throughput
- **Memory Leaks**: Increasing memory usage over time
- **Optimization Wins**: Performance improvements from changes
- **Platform Differences**: Variations across operating systems

### Regression Detection
```
Performance Regression Alert:
├── Operation: Single channel reading
├── Previous: 150.23 MB/s (2025-09-30)
├── Current: 142.15 MB/s (2025-10-01)
├── Regression: -5.4% (significant)
└── Action: Investigation recommended
```

### Trend Visualization
Generated charts automatically saved to `reports/performance_charts/`:
- `throughput_trends.png`: Performance over time
- `memory_usage_trends.png`: Memory efficiency tracking
- `rust_vs_python_comparison.png`: Implementation comparison
- `file_size_scaling.png`: Performance scaling analysis

## 🔧 Automated Analysis

### Statistical Analysis
Each benchmark run includes:
- **Confidence Intervals**: 95% statistical confidence
- **Outlier Detection**: Automated anomaly identification  
- **Correlation Analysis**: Performance factor relationships
- **Trend Significance**: Statistical trend testing

### Performance Alerts
Automated alerts triggered for:
- **>10% performance degradation** from baseline
- **Memory usage increase >20%** from baseline
- **Test failure rate >5%** in any category
- **Significant platform differences** (>25% variance)

## 📊 Using Results Data

### Loading Historical Data
```python
import json
import pandas as pd
from pathlib import Path

def load_benchmark_history(days=30):
    """Load recent benchmark results for analysis"""
    results = []
    
    for date_dir in Path('benchmarks/results/history').iterdir():
        if date_dir.is_dir():
            summary_file = date_dir / 'summary.json'
            if summary_file.exists():
                with open(summary_file) as f:
                    data = json.load(f)
                    data['date'] = date_dir.name
                    results.append(data)
    
    return pd.DataFrame(results)

# Usage
df = load_benchmark_history()
print(df.groupby('date')['summary.average_throughput'].mean())
```

### Performance Analysis Examples
```python
# Detect performance regressions
def detect_regressions(df, threshold=0.1):
    df_sorted = df.sort_values('date')
    baseline = df_sorted.head(10)['summary.average_throughput'].mean()
    recent = df_sorted.tail(10)['summary.average_throughput'].mean()
    
    regression = (baseline - recent) / baseline
    if regression > threshold:
        return f"⚠️  Regression detected: {regression:.1%}"
    return "✅ Performance stable"

# Memory efficiency analysis  
def analyze_memory_trends(df):
    return df.groupby('date').agg({
        'results.memory_peak_mb': 'mean',
        'results.memory_average_mb': 'mean',
        'file_size_mb': 'mean'
    }).assign(
        memory_efficiency = lambda x: x['results.memory_peak_mb'] / x['file_size_mb']
    )
```

### Cross-Platform Comparison
```bash
# Generate platform comparison report
python analysis/compare_platforms.py \
    --windows results/history/2025-10-01/windows/ \
    --linux results/history/2025-10-01/linux/ \
    --macos results/history/2025-10-01/macos/ \
    --output reports/platform_comparison.html
```

## 🚀 Automated Reporting

### Daily Summaries
Automated daily reports generated with:
- Performance summary statistics
- Comparison with previous day
- Trend analysis and alerts
- System resource utilization

### Monthly Analysis
Comprehensive monthly reports include:
- Performance trend analysis
- Regression/improvement identification
- Cross-platform comparisons
- Optimization recommendations

### CI/CD Integration
Results can be integrated with continuous integration:
```yaml
# GitHub Actions example
- name: Run Performance Benchmarks
  run: |
    cargo run --example rust_performance_benchmark > results/ci_benchmark.log
    python benchmarks/python/python_performance_benchmark.py >> results/ci_benchmark.log
    
- name: Check Performance Regression
  run: |
    python scripts/check_regression.py results/ci_benchmark.log
```

## 🧹 Cleanup & Archival

### Automatic Cleanup
Results are automatically archived:
- **Daily logs**: Kept for 30 days
- **Weekly summaries**: Kept for 6 months  
- **Monthly reports**: Kept indefinitely
- **Raw logs**: Compressed after 7 days

### Manual Cleanup Commands
```bash
# Remove logs older than 30 days
find results/history -name "*.log" -mtime +30 -delete

# Compress old results
for dir in results/history/*/; do
    if [ $(date -d "$(basename "$dir")" +%s) -lt $(date -d "30 days ago" +%s) ]; then
        tar -czf "${dir}.tar.gz" "$dir" && rm -rf "$dir"
    fi
done

# Windows PowerShell equivalent
Get-ChildItem "results\history" -Directory | 
    Where-Object { $_.CreationTime -lt (Get-Date).AddDays(-30) } |
    ForEach-Object { Compress-Archive $_.FullName "$($_.FullName).zip"; Remove-Item $_.FullName -Recurse }
```

## 📊 Custom Analysis Scripts

### Performance Comparison Script
```python
#!/usr/bin/env python3
"""Generate performance comparison report"""

import argparse
import matplotlib.pyplot as plt
import pandas as pd

def generate_comparison_report(rust_results, python_results, output_path):
    # Load results
    rust_df = pd.read_json(rust_results)
    python_df = pd.read_json(python_results)
    
    # Create comparison plots
    fig, axes = plt.subplots(2, 2, figsize=(12, 10))
    
    # Throughput comparison
    axes[0,0].bar(['Rust', 'Python'], 
                  [rust_df['throughput'].mean(), python_df['throughput'].mean()])
    axes[0,0].set_title('Average Throughput')
    axes[0,0].set_ylabel('MB/s')
    
    # Memory efficiency
    rust_memory_ratio = rust_df['memory_peak'] / rust_df['file_size']
    python_memory_ratio = python_df['memory_peak'] / python_df['file_size']
    
    axes[0,1].bar(['Rust', 'Python'], 
                  [rust_memory_ratio.mean(), python_memory_ratio.mean()])
    axes[0,1].set_title('Memory Efficiency')
    axes[0,1].set_ylabel('Peak Memory / File Size')
    
    # File size scaling
    axes[1,0].scatter(rust_df['file_size'], rust_df['throughput'], label='Rust', alpha=0.7)
    axes[1,0].scatter(python_df['file_size'], python_df['throughput'], label='Python', alpha=0.7)
    axes[1,0].set_xlabel('File Size (MB)')
    axes[1,0].set_ylabel('Throughput (MB/s)')
    axes[1,0].set_title('Performance Scaling')
    axes[1,0].legend()
    
    # Performance ratio over time
    axes[1,1].plot(rust_df.index, rust_df['throughput'] / python_df['throughput'])
    axes[1,1].set_title('Rust/Python Performance Ratio')
    axes[1,1].set_ylabel('Ratio')
    axes[1,1].axhline(y=1.0, color='r', linestyle='--', alpha=0.5)
    
    plt.tight_layout()
    plt.savefig(output_path, dpi=300, bbox_inches='tight')
    print(f"📊 Comparison report saved to: {output_path}")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description='Generate performance comparison')
    parser.add_argument('--rust', required=True, help='Rust results JSON file')
    parser.add_argument('--python', required=True, help='Python results JSON file') 
    parser.add_argument('--output', default='comparison_report.png', help='Output chart file')
    
    args = parser.parse_args()
    generate_comparison_report(args.rust, args.python, args.output)
```

## 🔒 Data Integrity

### Result Validation
All benchmark results include:
- **Checksum verification**: Detect corrupted log files
- **Schema validation**: Ensure proper JSON format
- **Range checking**: Validate performance metrics are reasonable
- **Consistency checks**: Cross-validate related measurements

### Backup Strategy
- **Local backup**: Daily backup to `results/backup/`
- **Git tracking**: Version control for analysis scripts
- **Archive export**: Monthly export for long-term storage

## 🤝 Contributing Results

When contributing benchmark results:

1. **Include system info**: Hardware, OS, software versions
2. **Run multiple iterations**: At least 3 runs for statistical validity
3. **Document methodology**: Any deviations from standard process
4. **Validate integrity**: Ensure results pass automated checks
5. **Add analysis**: Include interpretation and insights

### Result Submission Format
```json
{
  "contributor": "username",
  "submission_date": "2025-10-01",
  "system_info": {
    "os": "Ubuntu 22.04",
    "cpu": "AMD Ryzen 7 5800X",
    "memory": "32GB DDR4-3200",
    "storage": "Samsung 980 Pro NVMe"
  },
  "methodology": "Standard benchmark suite, 5 iterations per test",
  "results": { /* benchmark data */ },
  "notes": "Additional optimizations: CPU governor set to performance"
}
```