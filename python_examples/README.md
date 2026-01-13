# MF4-RS Python Bindings Examples

This directory contains Python examples demonstrating how to use the mf4-rs Python bindings for working with ASAM MDF 4 measurement files.

## Installation

### Prerequisites

1. **Rust toolchain**: Install from [rustup.rs](https://rustup.rs/)
2. **Python 3.8+**: Make sure you have a compatible Python version
3. **maturin**: Install the Python wheel builder for Rust extensions

```bash
pip install maturin
```

### Building the Python Extension

From the root directory of the project:

```bash
# Build in development mode (creates a .pyd/.so file that Python can import)
maturin develop --release

# Or build a wheel for distribution
maturin build --release
```

After running `maturin develop --release`, you should be able to import `mf4_rs` in Python.

**Note**: The `pyo3` feature is automatically enabled by maturin via `pyproject.toml`, so you don't need to specify `--features pyo3`.

### Installation from Wheel

If you built a wheel:

```bash
pip install target/wheels/mf4_rs-*.whl
```

## Examples

### 1. read_file.py - Reading MDF Files

This example shows how to:
- Open an MDF file
- Inspect channel groups and channels
- Read channel values (returns native Python types: float, int, str, bytes)
- Use pandas Series for data analysis
- Look up channels by group name and channel name

```python
import mf4_rs

# Open an MDF file
mdf = mf4_rs.PyMDF("example.mf4")

# Get channel groups
groups = mdf.channel_groups()
print(f"Found {len(groups)} channel groups")

# Get all channel names
names = mdf.get_all_channel_names()

# Read values for a specific channel (returns native Python types)
values = mdf.get_channel_values("Temperature")
# values is a list of Python floats/ints/str/bytes (no wrapper objects!)

# Read channel by group name + channel name (more precise lookup)
engine_temp = mdf.get_channel_values_by_group_and_name("Engine", "Temperature")

# Get channel as pandas Series with time index (requires pandas)
import pandas as pd
series = mdf.get_channel_as_series("Temperature")
# series.index contains time values, series.values contains temperature
print(series.describe())  # Use full pandas functionality!
```

### 2. write_mdf.py - Creating MDF Files

This example demonstrates:
- Creating a new MDF writer
- Adding channel groups and channels
- Writing data records
- Following the master channel pattern for multi-channel files

```python
import mf4_rs

# Create writer
writer = mf4_rs.PyMdfWriter("output.mf4")
writer.init_mdf_file()

# Add channel group
group_id = writer.add_channel_group("Test Group")

# Add time channel (master)
time_ch = writer.add_channel(
    group_id, "Time", mf4_rs.create_data_type_float_le(), 64, None
)
writer.set_time_channel(time_ch)

# Add data channels
temp_ch = writer.add_channel(
    group_id, "Temperature", mf4_rs.create_data_type_float_le(), 32, time_ch
)

# Write data
writer.start_data_block(group_id)
writer.write_record(group_id, [
    mf4_rs.create_float_value(1.0),    # Time
    mf4_rs.create_float_value(25.5),   # Temperature
])
writer.finish_data_block(group_id)
writer.finalize()
```

### 3. index_operations.py - MDF File Indexing

This example shows the powerful indexing system:
- Creating lightweight indexes from MDF files
- Saving/loading indexes to/from JSON
- Fast channel data access using indexes (returns native Python types)
- Getting byte ranges for efficient I/O
- Pandas Series support with automatic time indexing
- Group + name channel lookup

```python
import mf4_rs

# Create index from MDF file
index = mf4_rs.PyMdfIndex.from_file("data.mf4")

# Save index to JSON
index.save_to_file("data_index.json")

# Load index later
index = mf4_rs.PyMdfIndex.load_from_file("data_index.json")

# Read channel data using the index (returns native Python types)
values = index.read_channel_values_by_name("Temperature", "data.mf4")

# Read channel by group name + channel name
engine_temp = index.read_channel_values_by_group_and_name("Engine", "Temperature", "data.mf4")

# Get channel as pandas Series with time index
series = index.read_channel_as_series("Temperature", "data.mf4")
print(series.describe())  # Full pandas functionality!

# Get byte ranges for custom I/O
ranges = index.get_channel_byte_ranges(0, 1)  # group 0, channel 1
```

### 4. pandas_example.py - Pandas Integration

**NEW!** Direct pandas Series support with automatic DatetimeIndex:
- Returns pandas Series objects with absolute timestamps
- **Automatic DatetimeIndex creation** from MDF start time + relative time values
- Automatic master/time channel detection and indexing
- Works with both PyMDF and PyMdfIndex
- Enables full pandas time-series analysis capabilities
- Demonstrates resampling, time-based slicing, and datetime operations

```python
import mf4_rs
import pandas as pd

# Open MDF file
mdf = mf4_rs.PyMDF("data.mf4")

# Get channel as pandas Series with DatetimeIndex
temp_series = mdf.get_channel_as_series("Temperature")
speed_series = mdf.get_channel_as_series("Speed")

# Index is now a DatetimeIndex with absolute timestamps!
print(temp_series.index)  # DatetimeIndex(['2024-01-15 10:30:00', ...])

# Now use full pandas functionality!
print(temp_series.describe())
print(f"Mean: {temp_series.mean():.2f}")
print(f"Max: {temp_series.max():.2f}")

# Time-based operations with absolute timestamps
print(f"Value at 10:30:05: {temp_series.loc['2024-01-15 10:30:05']}")

# Resampling to different time intervals
temp_1s = temp_series.resample('1S').mean()  # Resample to 1-second intervals

# Plot with matplotlib (x-axis shows real timestamps!)
temp_series.plot(title="Temperature over Time")

# Combine multiple series into DataFrame
df = pd.DataFrame({
    'Temperature': temp_series,
    'Speed': speed_series
})
print(df.corr())  # Correlation matrix

# Works with indexes too (faster for repeated access)
index = mf4_rs.PyMdfIndex.from_file("data.mf4")
series = index.read_channel_as_series("Temperature", "data.mf4")
```

### 5. Enhanced Index with Resolved Conversions

**NEW!** The enhanced index system pre-resolves all conversion dependencies:
- All text conversions, nested conversions, and formulas are resolved during index creation
- Perfect for HTTP/remote file access scenarios
- Zero file access needed for conversions during data reading
- Complete self-contained index files

```python
import mf4_rs

# Create enhanced index - automatically resolves all conversions
index = mf4_rs.PyMdfIndex.from_file("data.mf4")

# Check if index has resolved conversion data
has_resolved = index.has_resolved_conversions()
print(f"Enhanced conversions: {has_resolved}")

# Get detailed conversion info
conv_info = index.get_conversion_info(0, 1)  # group 0, channel 1
if conv_info:
    print(f"Conversion type: {conv_info['conversion_type']}")
    if 'resolved_texts' in conv_info:
        print(f"Resolved texts: {len(conv_info['resolved_texts'])}")

# Advanced byte range features for HTTP optimization
total_bytes, range_count = index.get_channel_byte_summary(0, 1)
print(f"Channel data: {total_bytes} bytes in {range_count} ranges")

# Get byte ranges for specific record ranges (perfect for HTTP partial content)
partial_ranges = index.get_channel_byte_ranges_for_records(0, 1, 0, 10)  # first 10 records
partial_bytes = sum(length for _, length in partial_ranges)
savings = (1 - partial_bytes / total_bytes) * 100
print(f"First 10 records: {partial_bytes} bytes ({savings:.1f}% bandwidth savings)")

# Fast channel lookups
channel_info = index.get_channel_info_by_name("Temperature")
if channel_info:
    group_idx, channel_idx, info = channel_info
    print(f"Temperature: Group {group_idx}, Channel {channel_idx}, {info.bit_count} bits")

# Find all channels with same name across groups
all_matches = index.find_all_channels_by_name("Temperature")
print(f"All Temperature channels: {all_matches}")
```

### 6. simple_enhanced_index.py - Quick Start Example

A concise example showing the most important enhanced index features:
- Automatic conversion resolution
- HTTP-optimized byte range calculations
- Name-based channel access
- File size comparison

### 7. enhanced_index_python_example.py - Comprehensive Demo

A complete demonstration including:
- Performance comparisons with direct MDF reading
- HTTP range request simulation
- Advanced search and lookup features
- Detailed analysis of conversion resolution

## Key Features

### Native Python Types

**NEW!** All value methods now return native Python types directly:
- Reading channels returns `list` of `float`, `int`, `str`, or `bytes`
- No wrapper objects - values are immediately usable
- `None` represents invalid/missing samples
- Full type compatibility with pandas, numpy, and other libraries

```python
values = mdf.get_channel_values("Temperature")
# values is List[Optional[float]] - native Python floats!
mean = sum(v for v in values if v is not None) / len([v for v in values if v is not None])
```

### Pandas Integration

**NEW!** Direct pandas Series support with automatic DatetimeIndex:
- `get_channel_as_series(name)` - Returns pandas Series with datetime index
- `read_channel_as_series(name, file)` - Index-based version
- **Automatic DatetimeIndex creation** - Converts MDF start time + relative time to absolute timestamps
- Automatic master/time channel detection (channel_type == 2)
- Falls back to integer index if no master channel or datetime conversion fails
- Validates length matching between master and data channels

**DatetimeIndex Features**:
- Uses MDF file start timestamp (`abs_time` from header)
- Adds relative time values from master channel (in seconds)
- Returns pandas `DatetimeIndex` with absolute timestamps
- Enables time-based slicing, resampling, and time-series operations
- Handles None values (converts to `NaT`)
- Supports integer and float time channels

```python
series = mdf.get_channel_as_series("Temperature")
# series.index is now a pandas DatetimeIndex with absolute timestamps!
print(series.index[0])  # Timestamp('2024-01-15 10:30:00.000000000')
print(series.describe())  # Full pandas functionality!

# Time-based operations
series.resample('1S').mean()  # Resample to 1-second intervals
series.loc['2024-01-15 10:30:00':'2024-01-15 10:30:10']  # Slice by time
```

### Enhanced Channel Lookup

**NEW!** Look up channels by group name + channel name:
- `get_channel_values_by_group_and_name(group, channel)` - Direct MDF access
- `read_channel_values_by_group_and_name(group, channel, file)` - Index-based access
- More precise than global name lookup
- Useful when multiple groups have channels with the same name

```python
# Precise lookup when multiple "Temperature" channels exist
engine_temp = mdf.get_channel_values_by_group_and_name("Engine", "Temperature")
cabin_temp = mdf.get_channel_values_by_group_and_name("Cabin", "Temperature")
```

### Data Types

The Python bindings support all MDF data types:
- `create_data_type_float_le()` - 32/64-bit floats
- `create_data_type_uint_le()` - Unsigned integers
- `create_data_type_string_utf8()` - UTF-8 strings

### Value Creation

Create values for writing:
- `create_float_value(3.14)`
- `create_uint_value(42)`
- `create_int_value(-10)`
- `create_string_value("text")`

### Error Handling

All operations can raise `mf4_rs.MdfException` for MDF-specific errors:

```python
try:
    mdf = mf4_rs.PyMDF("nonexistent.mf4")
except mf4_rs.MdfException as e:
    print(f"MDF Error: {e}")
```

## Multi-Channel File Requirements

When creating MDF files with multiple channels, you **must** follow the master channel pattern:

1. Create a master channel (usually time)
2. Call `set_time_channel()` to designate it as master
3. Add data channels with the master channel as parent
4. Write records with values in the order channels were added

This pattern is required by the MDF 4.1 specification and ensures all channels are properly saved.

## Performance Tips

- Use the indexing system for repeated access to the same files
- Index creation is a one-time cost that enables fast subsequent access
- Indexes contain all metadata needed for data extraction
- Use `write_records()` (when available) instead of multiple `write_record()` calls
- The underlying Rust library uses memory-mapped files for efficient large file handling

## Enhanced Indexing Use Cases

The enhanced indexing system is particularly useful for:
- **HTTP/Remote File Access**: Pre-resolved conversions eliminate additional file requests
- **Cloud Storage Optimization**: Precise byte ranges minimize bandwidth usage
- **Fast channel browsing** without loading entire files
- **Selective data extraction** for specific channels with bandwidth savings up to 90%
- **Metadata caching** for large file collections with full conversion support
- **Remote file analysis** by transferring only small self-contained index files
- **Memory-efficient processing** of specific record ranges
- **Text conversion support** without file access (ValueToText, RangeToText, etc.)
- **Nested conversion chains** fully resolved and stored in index
- **Microservice architectures** where index and data access are separated

## Compatibility

These bindings expose the core functionality of the Rust mf4-rs library:
- Reading/writing MDF 4.1 compliant files
- Support for various data types and conversions
- Memory-efficient handling of large files
- Fast indexing system for metadata and selective data access

The Python API is designed to be intuitive while preserving the performance benefits of the underlying Rust implementation.