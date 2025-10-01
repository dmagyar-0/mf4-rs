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
maturin develop --features pyo3

# Or build a wheel for distribution
maturin build --features pyo3
```

After running `maturin develop`, you should be able to import `mf4_rs` in Python.

### Installation from Wheel

If you built a wheel:

```bash
pip install target/wheels/mf4_rs-*.whl
```

## Examples

### 1. parse_mdf.py - Reading MDF Files

This example shows how to:
- Open an MDF file
- Inspect channel groups and channels
- Read channel values

```python
import mf4_rs

# Open an MDF file
mdf = mf4_rs.PyMDF("example.mf4")

# Get channel groups
groups = mdf.channel_groups()
print(f"Found {len(groups)} channel groups")

# Get all channel names
names = mdf.get_all_channel_names()

# Read values for a specific channel
values = mdf.get_channel_values("Temperature")
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

### 3. index_mdf.py - MDF File Indexing

This example shows the powerful indexing system:
- Creating lightweight indexes from MDF files
- Saving/loading indexes to/from JSON
- Fast channel data access using indexes
- Getting byte ranges for efficient I/O

```python
import mf4_rs

# Create index from MDF file
index = mf4_rs.PyMdfIndex.from_file("data.mf4")

# Save index to JSON
index.save_to_file("data_index.json")

# Load index later
index = mf4_rs.PyMdfIndex.load_from_file("data_index.json")

# Read channel data using the index
values = index.read_channel_values_by_name("Temperature", "data.mf4")

# Get byte ranges for custom I/O
ranges = index.get_channel_byte_ranges(0, 1)  # group 0, channel 1
```

### 4. Enhanced Index with Resolved Conversions

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

### 5. simple_enhanced_index.py - Quick Start Example

A concise example showing the most important enhanced index features:
- Automatic conversion resolution
- HTTP-optimized byte range calculations
- Name-based channel access
- File size comparison

### 6. enhanced_index_python_example.py - Comprehensive Demo

A complete demonstration including:
- Performance comparisons with direct MDF reading
- HTTP range request simulation
- Advanced search and lookup features
- Detailed analysis of conversion resolution

## Key Features

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