# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`mf4-rs` is a Rust library for working with ASAM MDF 4 (Measurement Data Format) files. It supports both reading and writing MDF files with a safe API, implementing a subset of the MDF 4.1 specification. The library includes optional Python bindings via PyO3.

## Build and Test Commands

### Rust Development
```bash
# Build the project
cargo build

# Run all tests
cargo test

# Run a specific test file (e.g., api tests)
cargo test --test api

# Run with specific test name filter
cargo test test_name_pattern

# Run examples
cargo run --example write_file
cargo run --example read_file
cargo run --example index_operations
```

### Python Bindings Development
```bash
# Build and install Python bindings for development
maturin develop --release

# Install from source
pip install .
# or
uv pip install .
```

## Architecture

The codebase follows a strict layered architecture. Understanding these layers is critical:

### 1. API Layer (`src/api/`)
- **High-level user-facing API** - This is what external users interact with
- `MDF` - Entry point for reading files from disk
- `ChannelGroup` - Ergonomic access to channel group metadata
- `Channel` - High-level channel representation with value decoding
- These types provide safe abstractions over the raw parsing layer

### 2. Writer Module (`src/writer/mdf_writer/`)
- Split into three modules: `init.rs` (initialization), `data.rs` (data writing), `io.rs` (low-level I/O)
- Guarantees: little-endian encoding, 8-byte alignment, zero-padding
- Uses a closure-based builder pattern for channel/channel group configuration
- Maintains position maps to update block links after writing
- Auto-splits data blocks when they exceed 4MB

### 3. Block Layer (`src/blocks/`)
- **Low-level MDF block implementations** matching the specification exactly
- Each block type (Header, Channel, ChannelGroup, DataGroup, etc.) implements:
  - `BlockParse` trait for parsing from bytes
  - Custom serialization for writing
- `conversion/` subdirectory contains conversion implementations (linear, formula, lookup tables, etc.)
- Common utilities in `common.rs` for block headers and data types

### 4. Parsing Layer (`src/parsing/`)
- Uses `memmap2` for memory-mapped file access - **never loads entire files into memory**
- `mdf_file.rs` - Memory-mapped file management
- `raw_*` modules - Raw block parsers maintaining references to mmap data
- `decoder.rs` - Channel value decoder supporting multiple data types
- **Lazy evaluation** - channels and values are only decoded when accessed

### 5. Index System (`src/index.rs`)
- Creates lightweight JSON indexes containing all metadata needed to read channel data
- Supports the `ByteRangeReader` trait for various data sources (local files, HTTP, S3)
- Allows reading specific channels without parsing the entire file
- Critical for applications that need repeated access to the same files

### 6. Utilities
- `cut.rs` - Time-based file cutting
- `merge.rs` - File merging
- `error.rs` - Centralized error handling with `thiserror`

### 7. Python Bindings (`src/python.rs`)
- Only built when `pyo3` feature is enabled
- Wraps Rust API with Python-friendly interface
- See `python_examples/` for usage patterns

## Key Design Patterns and Concepts

### Memory-Mapped Files
The parser never loads entire files into memory. It uses `memmap2` to create memory-mapped views, which is essential for handling large measurement files (can be GBs in size). Be careful when modifying parsing code to maintain references correctly.

### Lazy Evaluation
Channel groups, channels, and values are lightweight wrappers that only decode data when accessed. This is critical for performance - don't break this pattern by eagerly loading data.

### Block Linking
MDF files use address-based linking between blocks (e.g., a header links to data groups at specific file offsets). The writer maintains position maps to update these links after blocks are written. When modifying the writer, ensure block addresses are correctly updated.

### Builder Pattern for Writer
Channels and channel groups are configured using closures:
```rust
writer.add_channel(&cg, parent, |ch| {
    ch.data_type = DataType::FloatLE;
    ch.name = Some("Temperature".to_string());
    ch.bit_count = 64;
})?;
```

This pattern allows flexible configuration while maintaining type safety.

### Master Channels (Time Channels)
In MDF, each channel group typically has one "master" channel (usually time), and other channels are linked to it as children. When using the writer:
1. Create the master/time channel first
2. Call `writer.set_time_channel(&time_ch_id)?`
3. Pass the time channel ID as the parent when creating data channels

### Data Types and Endianness
The library enforces little-endian encoding. All data types are defined in `src/blocks/common.rs` and map directly to MDF specification types. When adding new data type support, update both the `DataType` enum and the decoder.

### Invalidation Bits
MDF supports "invalidation bits" to mark samples as invalid. These are handled in the decoder and index system. The bit position and flags are stored per-channel.

### Conversions
MDF supports various conversion types (linear, formula, lookup tables) to transform raw values. These are implemented in `src/blocks/conversion/` and applied during decoding. When reading channels, conversions are automatically applied if present.

## Important Implementation Notes

### When Modifying the Parser
- Maintain lifetime relationships between `MDF`, `ChannelGroup`, and `Channel` - they all hold references to the memory-mapped file
- The `MDF` struct owns the memory map; everything else borrows from it
- Be careful with block address calculations - MDF uses absolute file offsets

### When Modifying the Writer
- All blocks must be 8-byte aligned with zero-padding
- Update the position map when writing blocks that other blocks need to reference
- The writer maintains a list of "open" data blocks - ensure they're properly closed
- Call `finalize()` to update all block links before closing the file

### When Modifying the Index System
- The index must be self-contained (no references to the original file)
- All metadata needed for decoding must be serialized
- The `ByteRangeReader` trait allows custom data sources - don't assume local files
- Conversions must be fully serialized with the index

### Python Bindings
- When adding new Rust functionality, consider if it needs Python exposure
- Python types are prefixed with `Py` (e.g., `PyMDF`, `PyMdfWriter`)
- Use `pyo3` macros for automatic Python wrapping
- Test changes with `maturin develop --release` before committing

## Test Organization

- `tests/*.rs` - Integration tests for major features
- `src/blocks/conversion/*_test.rs` - Conversion-specific tests (e.g., `test_deep_chains.rs`)
- Tests often create temporary MDF files to verify round-trip (write then read)

## Common Pitfalls

1. **Don't eagerly load data** - Respect the lazy evaluation pattern
2. **Block alignment** - Writer must maintain 8-byte alignment
3. **Lifetime management** - Parser types have complex lifetime relationships due to memory mapping
4. **Address updates** - When modifying writer, ensure block links are updated correctly
5. **Endianness** - Always use little-endian encoding, the library enforces this
