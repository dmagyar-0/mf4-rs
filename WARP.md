# WARP.md

This file provides guidance to WARP (warp.dev) when working with code in this repository.

## Project Overview

`mf4-rs` is a minimal Rust library for working with ASAM MDF 4 (Measurement Data Format) files. It supports parsing existing files and writing new ones through a safe API, implementing a subset of the MDF 4.1 specification sufficient for simple data logging and inspection tasks.

## Architecture

### High-Level Structure

The codebase is organized into distinct layers:

#### 1. API Layer (`src/api/`)
- **High-level user-facing API** for working with MDF files
- `MDF` - Main entry point for parsing files from disk
- `ChannelGroup` - Wrapper providing ergonomic access to channel group metadata
- `Channel` - High-level channel representation with value decoding

#### 2. Writer Module (`src/writer/`)
- **MdfWriter** - Core writer for creating MDF 4.1-compliant files
- Guarantees little-endian encoding, 8-byte alignment, and zero-padding
- Handles block linking and manages open data blocks during writing
- Supports both single record writing (`write_record`) and batch operations (`write_records`)

#### 3. Block Layer (`src/blocks/`)
- **Low-level MDF block implementations** matching the specification
- Each block type (HeaderBlock, ChannelBlock, ChannelGroupBlock, etc.) has parsing and serialization
- Conversion system supporting various data transformations (linear, formula, lookup tables)
- Common utilities for block headers and data type handling

#### 4. Parsing Layer (`src/parsing/`)
- **File parsing and memory management** using memory-mapped files
- Raw block parsers that maintain references to memory-mapped data
- Channel value decoder supporting multiple data types
- Lazy evaluation - channels and values are decoded on demand

#### 5. Utilities (`src/`)
- `cut.rs` - Time-based file cutting functionality
- `merge.rs` - File merging utilities
- `error.rs` - Centralized error handling
- `index.rs` - MDF file indexing system for fast metadata-based access

### Key Design Patterns

**Memory-Mapped File Access**: The parser uses `memmap2` to avoid loading entire files into memory, enabling efficient handling of large measurement files.

**Lazy Evaluation**: Channel groups, channels, and values are created as lightweight wrappers that decode data only when accessed.

**Builder Pattern**: The writer uses closure-based configuration for channels and channel groups, allowing flexible setup while maintaining type safety.

**Block Linking**: The MDF format uses address-based linking between blocks. The writer maintains a position map to update links after blocks are written.

## Development Commands

### Building and Testing
```bash
# Build the project
cargo build

# Run all tests
cargo test

# Run specific test file
cargo test --test api

# Build and run examples
cargo run --example cut_file
cargo run --example write_records
cargo run --example parse_file
cargo run --example mdf_index_example
cargo run --example byte_ranges_example
cargo run --example http_range_reader_example
```

### Examples
The project includes comprehensive examples in the `examples/` directory:

- `cut_file.rs` - Creates an MDF file and demonstrates time-based cutting
- `write_records.rs` - Shows batch record writing with `write_records`
- `parse_file.rs` - Demonstrates parsing and inspecting MDF files
- `python_equivalent.rs` - Comparison with Python asammdf library functionality
- `multi_groups_with_data.rs` - Working with multiple channel groups

### Working with MDF Files

#### Basic File Creation Pattern:
```rust
let mut writer = MdfWriter::new("output.mf4")?;
writer.init_mdf_file()?;
let cg = writer.add_channel_group(None, |_| {})?;
writer.add_channel(&cg, None, |ch| {
    ch.data_type = DataType::UnsignedIntegerLE;
    ch.name = Some("Channel Name".into());
})?;
writer.start_data_block_for_cg(&cg, 0)?;
writer.write_record(&cg, &[DecodedValue::UnsignedInteger(42)])?;
writer.finish_data_block(&cg)?;
writer.finalize()?;
```

#### Basic File Parsing Pattern:
```rust
let mdf = MDF::from_file("input.mf4")?;
for group in mdf.channel_groups() {
    for channel in group.channels() {
        let values = channel.values()?;
        // Process values...
    }
}
```

#### Multiple Channel Creation Requirements:

When creating MDF files with multiple channels, the `MdfWriter` has specific requirements for proper channel structure:

**Master Channel Pattern**: For files with multiple channels, you must establish a "master" channel (typically a time channel) before adding data channels:

```rust
// Create master channel (usually time)
let time_ch_id = writer.add_channel(&cg_id, None, |ch| {
    ch.data_type = DataType::FloatLE;
    ch.name = Some("Time".to_string());
    ch.bit_count = 64;
})?;
writer.set_time_channel(&time_ch_id)?; // Mark as master channel

// Add data channels with master as parent
writer.add_channel(&cg_id, Some(&time_ch_id), |ch| {
    ch.data_type = DataType::UnsignedIntegerLE;
    ch.name = Some("DataChannel".to_string());
    ch.bit_count = 32;
})?;
```

**Why This Pattern is Required**:
- The MDF 4.1 specification requires proper channel hierarchy for multi-channel groups
- Without a designated master channel, only the last channel added may be properly saved
- The `set_time_channel()` call establishes the channel as type 2 (master/time channel)
- Data channels should reference the master channel as their parent

**Record Writing**: When writing records with multiple channels, provide values in the order channels were added:
```rust
writer.write_record(&cg_id, &[
    DecodedValue::Float(1.0),              // Time channel (first)
    DecodedValue::UnsignedInteger(42),     // Data channel (second)
])?;
```

**Single Channel Files**: For single-channel files, you can create channels without the master pattern:
```rust
writer.add_channel(&cg_id, None, |ch| {
    ch.data_type = DataType::UnsignedIntegerLE;
    ch.name = Some("SingleChannel".to_string());
    ch.bit_count = 32;
})?;
// No set_time_channel() call needed
```

### Data Types and Conversions

The library supports various MDF data types through the `DataType` enum:
- `UnsignedIntegerLE` / `SignedIntegerLE` - Integer types
- `FloatLE` - Floating point (32/64-bit based on bit_count)
- `ByteArray` / `MimeSample` / `MimeStream` - Binary data

Conversion blocks handle data transformations:
- Linear conversions
- Formula-based conversions
- Value-to-text mapping
- Bitfield extraction

### Testing Strategy

Tests are organized in the `tests/` directory:
- `api.rs` - High-level API roundtrip tests
- `blocks.rs` - Low-level block serialization/parsing tests
- `merge.rs` - File merging functionality tests
- `index.rs` - MDF indexing system and name-based lookup tests

The test suite includes roundtrip testing (write → read → verify) to ensure data integrity across the entire pipeline.

**Testing Multi-Channel Files**: When writing tests that create multiple channels, follow the master channel pattern to ensure all channels are properly saved:
```rust
// Correct pattern for multi-channel test files
let master_ch_id = writer.add_channel(&cg_id, None, |ch| { /* config */ })?;
writer.set_time_channel(&master_ch_id)?;
writer.add_channel(&cg_id, Some(&master_ch_id), |ch| { /* config */ })?;
```
This ensures consistent behavior between tests and prevents issues where only some channels are saved to the test MDF files.

### MDF Indexing System

The library includes a powerful indexing system that allows you to:
1. **Create lightweight JSON indexes** of MDF files containing all metadata needed for data access
2. **Read channel data without full file parsing** using only the index and targeted file I/O
3. **Serialize/deserialize indexes** for persistent storage and sharing
4. **Support multiple data sources** through the `ByteRangeReader` trait (local files, HTTP, S3, etc.)

#### Basic Indexing Workflow:
```rust
// Create an index from an MDF file
let index = MdfIndex::from_file("data.mf4")?;

// Save index to JSON for later use
index.save_to_file("data_index.json")?;

// Later: load index and read specific channel data
let loaded_index = MdfIndex::load_from_file("data_index.json")?;

// Option 1: Use built-in file reader
let mut file_reader = FileRangeReader::new("data.mf4")?;
let channel_values = loaded_index.read_channel_values(0, 1, &mut file_reader)?;

// Option 2: Use HTTP range reader (production)
let mut http_reader = HttpRangeReader::new("https://cdn.example.com/data.mf4".to_string());
let channel_values = loaded_index.read_channel_values(0, 1, &mut http_reader)?;

// Option 3: Get exact byte ranges for custom reading
let byte_ranges = loaded_index.get_channel_byte_ranges(0, 1)?; // Returns Vec<(offset, length)>
let partial_ranges = loaded_index.get_channel_byte_ranges_for_records(0, 1, 100, 50)?; // Records 100-149
```

#### ByteRangeReader Trait:
The system uses a trait-based approach for data access, allowing you to implement custom readers:
```rust
pub trait ByteRangeReader {
    type Error;
    fn read_range(&mut self, offset: u64, length: u64) -> Result<Vec<u8>, Self::Error>;
}
```

Built-in implementations:
- `FileRangeReader` - For local file access
- HTTP reader example - Shows how to implement HTTP Range requests

#### Index Contents:
- Channel metadata (names, units, data types, offsets, conversions)
- Data block locations and sizes in the original file  
- Record layout information for efficient data extraction
- File validation information (size)

#### Use Cases:
- **Fast channel browsing** without loading entire files
- **Selective data extraction** for specific channels
- **Metadata caching** for large file collections
- **Remote file analysis** by transferring only small index files
- **Custom file readers** with precise byte range requests
- **Streaming/partial downloads** of large files over network
- **Memory-efficient processing** of specific record ranges

### Performance Considerations

- Use `write_records` for batch operations instead of multiple `write_record` calls
- Data blocks automatically split when they exceed 4MB to maintain performance
- Memory-mapped file access minimizes memory usage for large files
- Channel values are decoded lazily only when accessed
- **Use indexing for repeated access** to the same files to avoid re-parsing overhead

### Dependencies

- `nom` - Binary parsing combinators
- `byteorder` - Endianness handling  
- `memmap2` - Memory-mapped file I/O
- `meval` - Mathematical expression evaluation for formula conversions
- `thiserror` - Error handling derive macros