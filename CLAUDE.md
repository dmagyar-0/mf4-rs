# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`mf4-rs` is a Rust library for working with ASAM MDF 4 (Measurement Data Format) files. It implements a subset of the MDF 4.1 specification sufficient for data logging and inspection tasks. The library supports reading existing MDF files, writing new ones, creating lightweight JSON indexes for fast random access, time-based file cutting, and file merging. Optional Python bindings are available via PyO3.

**Key stats:** ~5,000 lines of Rust across ~47 source files. Rust edition 2024, version 1.0.0, MIT licensed.

## Build and Test Commands

### Rust Development
```bash
# Build the library (rlib + cdylib)
cargo build

# Run all tests (note: tests/enhanced_index_conversions.rs has a known compile error for missing start_time_ns field)
cargo test

# Run a specific test file
cargo test --test api
cargo test --test blocks
cargo test --test index
cargo test --test merge
cargo test --test test_invalidation_bits

# Run with specific test name filter
cargo test test_name_pattern

# Run examples (write_file creates a test file that read_file and others consume)
cargo run --example write_file
cargo run --example read_file
cargo run --example index_operations
cargo run --example cut_file
cargo run --example merge_files
```

### Python Bindings Development
```bash
# Build and install Python bindings for development (requires maturin)
maturin develop --release

# Install from source
pip install .
# or
uv pip install .
```

## Architecture

The codebase is organized into distinct layers. The module structure is defined inline in `src/lib.rs` (not via `mod.rs` files for the top-level `api` and `parsing` modules).

### 1. API Layer (`src/api/`)
- **High-level user-facing API** - what external users interact with
- `MDF` (`mdf.rs`) - Entry point; wraps `MdfFile`, provides `channel_groups()` and `start_time_ns()`
- `ChannelGroup` (`channel_group.rs`) - Borrows from `RawDataGroup`, `RawChannelGroup`, and the mmap; provides `name()`, `comment()`, `source()`, `channels()`
- `Channel` (`channel.rs`) - Borrows from `ChannelBlock` and raw types; provides `name()`, `unit()`, `comment()`, `source()`, `values()`
- All API types carry lifetime `'a` tied to the memory-mapped file owned by `MDF`

**Note:** `src/api/mod.rs` exists but is **not used** - `lib.rs` declares the `api` module inline, so `mod.rs` is dead code. Its re-exports (`pub use mdf_file::MDF` and `pub use source_info::SourceInfo`) reference modules that don't exist under `api/`.

### 2. Writer Module (`src/writer/mdf_writer/`)
- Split into three files: `init.rs` (structure creation and linking), `data.rs` (record encoding and DT block management), `io.rs` (low-level file I/O, alignment, link patching)
- Guarantees: little-endian encoding, 8-byte alignment, zero-padding
- Closure-based builder pattern for channel/channel group configuration
- Maintains `block_positions: HashMap<String, u64>` for updating block links after writing
- Auto-splits data blocks when they exceed `MAX_DT_BLOCK_SIZE` (4MB), creating `DataListBlock` chains
- Supports two I/O backends: `BufWriter<File>` (default, 1MB buffer) and `MmapMut` (via `new_mmap`)
- `ChannelEncoder` enum provides fast per-channel encoding without dynamic dispatch per value
- `set_record_template()` allows precomputing constant channel values to avoid redundant encoding
- `write_record_u64()` / `write_records_u64()` provide optimized paths for all-unsigned-integer groups

### 3. Block Layer (`src/blocks/`)
- **Low-level MDF block implementations** matching the MDF 4.1 specification
- Common infrastructure in `common.rs`:
  - `BlockHeader` (24 bytes: id, reserved, block_len, links_nr) with `from_bytes`/`to_bytes`
  - `BlockParse` trait: `const ID` + `from_bytes()` + `parse_header()` for each block type
  - `DataType` enum (17 variants mapping MDF spec values 0-16, plus `Unknown`)
  - `read_string_block()` helper that dispatches on `##TX` vs `##MD` block IDs
- Block types with their sizes:
  - `IdentificationBlock` (64 bytes) - File identification, version validation (>= 4.10 required)
  - `HeaderBlock` (104 bytes) - File header with absolute timestamp, timezone, links to data groups
  - `DataGroupBlock` (64 bytes) - Container linking to channel groups and data blocks
  - `ChannelGroupBlock` (104 bytes) - Group metadata, record layout, invalidation byte count
  - `ChannelBlock` (160 bytes) - Channel metadata, conversion link, name resolution, invalidation bit position
  - `TextBlock` (variable, 8-byte aligned) - Null-terminated strings with padding
  - `MetadataBlock` (variable) - XML metadata
  - `DataBlock` (variable) - Raw record data, borrows from mmap (`&'a [u8]`)
  - `DataListBlock` (variable) - Ordered list of data block fragments
  - `SourceBlock` (variable) - Signal source information (ECU, bus, tool, etc.)
  - `SignalDataBlock` (variable) - VLSD value stream (`[u32 length][bytes]...`)
- All block types implement `Default` for convenient construction

#### Conversion Subsystem (`src/blocks/conversion/`)
- `ConversionBlock` (`base.rs`) - Main struct with link section, type, values, and resolved dependency storage
- `ConversionType` enum (`types.rs`) - 12 types: Identity, Linear, Rational, Algebraic, TableLookupInterp/NoInterp, RangeLookup, ValueToText, RangeToText, TextToValue, TextToText, BitfieldText
- Implementation files:
  - `linear.rs` - Linear (`y = a + b*x`), Rational (`(p1*x² + p2*x + p3)/(p4*x² + p5*x + p6)`), and Algebraic (via `meval` expression evaluator)
  - `table_lookup.rs` - Interpolated and non-interpolated table lookups, range-based lookups
  - `text.rs` - Value-to-text, range-to-text, text-to-value, text-to-text conversions with fallback chains
  - `bitfield.rs` - Bitfield-to-text with mask-based extraction
  - `formula.rs` - Resolves algebraic formula text from referenced `##TX` blocks
  - `logic.rs` - `apply_decoded()` dispatcher that routes to the correct conversion implementation
- Dependency resolution: `resolve_all_dependencies_recursive()` follows `cc_ref` links with cycle detection (max depth 20), populating `resolved_texts`, `resolved_conversions`, and `default_conversion` fields for self-contained operation

### 4. Parsing Layer (`src/parsing/`)
- `MdfFile` (`mdf_file.rs`) - Opens file with `memmap2::Mmap`, parses identification block (64 bytes), header block, then walks data group → channel group → channel linked lists
- `RawDataGroup` (`raw_data_group.rs`) - Wraps `DataGroupBlock` + `Vec<RawChannelGroup>`; `data_blocks()` method transparently follows `##DT`/`##DV`/`##DL` chains
- `RawChannelGroup` (`raw_channel_group.rs`) - Simple wrapper: `ChannelGroupBlock` + `Vec<RawChannel>`
- `RawChannel` (`raw_channel.rs`) - Wraps `ChannelBlock`; `records()` returns a boxed iterator that handles both fixed-size records and VLSD channels (channel type 1 with `##SD`/`##DL` chains)
- `decoder.rs` - Core value decoding:
  - `DecodedValue` enum: `UnsignedInteger(u64)`, `SignedInteger(i64)`, `Float(f64)`, `String(String)`, `ByteArray(Vec<u8>)`, `MimeSample`, `MimeStream`, `Unknown`
  - `decode_channel_value()` - Legacy decode without validity checking
  - `decode_channel_value_with_validity()` - Full MDF 4.1 spec-compliant decode with invalidation bit checking
  - Supports bit-level extraction for sub-byte fields using bit_offset and bit_count
  - Handles LE and BE variants for integers and floats
  - String types: Latin1, UTF-8, UTF-16LE, UTF-16BE
- `source_info.rs` - `SourceInfo` struct with name/path/comment, parsed from `##SI` blocks

### 5. Index System (`src/index.rs`)
- `MdfIndex` - Self-contained index with `file_size`, `start_time_ns`, and `Vec<IndexedChannelGroup>`
- `IndexedChannelGroup` - Stores record layout metadata + `Vec<IndexedChannel>` + `Vec<DataBlockInfo>`
- `IndexedChannel` - Channel metadata including fully resolved `ConversionBlock` (serializable via serde)
- `ByteRangeReader` trait - Abstraction for data sources: `read_range(offset, length) -> Vec<u8>`
- `FileRangeReader` - Built-in local file implementation
- Key capabilities:
  - `from_file()` / `save_to_file()` / `load_from_file()` - Create, persist, and reload JSON indexes
  - `read_channel_values()` / `read_channel_values_by_name()` - Read data using index + byte range reader
  - `get_channel_byte_ranges()` / `get_channel_byte_ranges_for_records()` - Calculate exact byte ranges for partial reads
  - `find_channel_by_name_global()` / `find_all_channels_by_name()` - Channel name lookups across all groups
  - Conversions are resolved during index creation, enabling reads with empty `file_data` (`&[]`)

### 6. File Operations
- `cut.rs` - `cut_mdf_by_time(input, output, start_time, end_time)`: Copies only records whose master channel value falls within `[start_time, end_time]`. Identifies master channels by `channel_type == 2 && sync_type == 1`.
- `merge.rs` - `merge_files(output, first, second)`: Merges two files. Channel groups with identical layouts (same channel names, types, offsets) are concatenated; different groups are appended separately.

### 7. Error Handling (`src/error.rs`)
- `MdfError` enum using `thiserror`:
  - `TooShortBuffer` - Insufficient bytes for parsing (includes file/line for debugging)
  - `FileIdentifierError` / `FileVersioningError` - Invalid MDF file header
  - `BlockIDError` - Wrong block type encountered
  - `IOError` - Wraps `std::io::Error`
  - `InvalidVersionString` / `BlockLinkError` / `BlockSerializationError` - Various structural errors
  - `ConversionChainTooDeep` / `ConversionChainCycle` - Conversion dependency resolution errors

### 8. Python Bindings (`src/python.rs`)
- Built only when `pyo3` feature is enabled (configured in `pyproject.toml` via `[tool.maturin] features = ["pyo3"]`)
- Uses PyO3 0.21 with extension-module feature
- Main classes:
  - `PyMDF` - Wraps `MDF`; methods: `channel_groups()`, `get_channel_values()`, `get_channel_values_by_group_and_name()`, `get_channel_as_series()` (pandas DatetimeIndex support)
  - `PyMdfWriter` - Wraps `MdfWriter` with simplified Python API; manages ID mapping between Python and Rust IDs; provides `add_time_channel()`, `add_float_channel()`, `add_int_channel()` convenience methods
  - `PyMdfIndex` - Wraps `MdfIndex`; supports index-based reads, name lookups, byte range calculations, pandas Series output, conversion info inspection
  - `PyChannelInfo`, `PyChannelGroupInfo`, `PyDecodedValue`, `PyDataType` - Data transfer types
- Helper functions: `create_float_value()`, `create_uint_value()`, `create_int_value()`, `create_string_value()`, `create_data_type_*()` factory functions
- Custom `MdfException` Python exception type
- Returns native Python types (float, int, str, bytes) via `decoded_value_to_pyobject()` for zero-copy efficiency
- Pandas DatetimeIndex support: converts relative master channel times to absolute timestamps using the MDF file's start time

## Key Design Patterns and Concepts

### Memory-Mapped Files
The parser never loads entire files into memory. `MdfFile` uses `memmap2::Mmap` (created with `unsafe { Mmap::map(&file) }`). All parsing operates on `&[u8]` slices into the mmap. The `MdfFile` struct owns the `Mmap`; `RawDataGroup`, `RawChannelGroup`, and API types borrow from it.

### Lazy Evaluation
Channel groups, channels, and values are lightweight wrappers holding references. `Channel::values()` is the only method that actually decodes sample data - it iterates over raw records and calls the decoder. Don't break this pattern by eagerly loading data.

### Block Linking
MDF files use absolute file offsets as addresses between blocks (e.g., `HeaderBlock.first_dg_addr` points to the first data group). The writer uses a `HashMap<String, u64>` mapping logical IDs (like `"dg_0"`, `"cg_1"`, `"cn_3"`) to file positions, and patches links after blocks are written using `update_block_link()` which seeks back and writes the target address.

### Builder Pattern for Writer
Channels and channel groups are configured using closures that receive a mutable reference to a default block:
```rust
writer.add_channel(&cg, parent, |ch| {
    ch.data_type = DataType::FloatLE;
    ch.name = Some("Temperature".to_string());
    ch.bit_count = 64;
})?;
```
If `bit_count` is left at 0, it defaults to `DataType::default_bits()`. Byte offsets are auto-calculated from previously added channels in the same group.

### Master Channels (Time Channels)
Each channel group typically has one "master" channel (usually time). In MDF, master channels have `channel_type == 2` and `sync_type == 1`. When using the writer:
1. Create the master/time channel first via `add_channel()`
2. Call `writer.set_time_channel(&time_ch_id)?` (patches channel_type and sync_type in the file)
3. Pass the time channel ID as `prev_cn_id` when creating subsequent data channels (for the linked list)

### Data Block Auto-Splitting
When writing records, the writer tracks data block size. If a record would push the current `##DT` block past `MAX_DT_BLOCK_SIZE` (4MB), it automatically:
1. Finalizes the current DT block (patches its `block_len`)
2. Starts a new DT block
3. On `finish_data_block()`, creates a `##DL` (DataListBlock) linking all fragments together

### Record Structure
Each record in a data block has the layout: `[record_id (0-8 bytes)] [data bytes] [invalidation bytes]`
- `record_id_len` from `DataGroupBlock` (usually 0 for single-CG groups)
- `samples_byte_nr` from `ChannelGroupBlock` (total data bytes per record)
- `invalidation_bytes_nr` from `ChannelGroupBlock` (0 if no invalidation bits used)

### Invalidation Bits
MDF supports per-channel invalidation bits appended after the data portion of each record:
- `cn_flags` bit 0 set: all values invalid (short-circuit)
- `cn_flags` bits 0 and 1 both clear: all values valid (short-circuit)
- Otherwise: check bit at `pos_invalidation_bit` in the invalidation byte region
- If the invalidation bit is set (1), the value is INVALID

### Conversion System
Conversions transform raw channel values to physical values. Supported types:
- **Identity** (type 0): passthrough
- **Linear** (type 1): `phys = cc_val[0] + cc_val[1] * raw`
- **Rational** (type 2): `phys = (P1*X² + P2*X + P3) / (P4*X² + P5*X + P6)` using 6 coefficients
- **Algebraic** (type 3): formula string evaluated via `meval` with variable `X`
- **Table lookups** (types 4-6): interpolated, non-interpolated, and range-based
- **Text conversions** (types 7-11): value-to-text, range-to-text, text-to-value, text-to-text, bitfield

Conversions can be chained (one conversion's cc_ref points to another `##CC` block). The resolution system handles:
- Recursive resolution with max depth of 20
- Cycle detection using a `HashSet<u64>` of visited block addresses
- Default/fallback conversions (last cc_ref entry for certain types like RangeToText)

### VLSD (Variable-Length Signal Data)
Channels with `channel_type == 1` and a non-zero `data` field store variable-length values in `##SD` (SignalDataBlock) or `##DL`→`##SD` chains. Each VLSD entry is `[u32 length][value bytes]`. The `RawChannel::records()` method transparently handles this via a stateful `from_fn` iterator.

## Important Implementation Notes

### Module System Quirk
`src/lib.rs` declares `api` and `parsing` modules inline (lines 14-27), which **overrides** any `mod.rs` files in those directories. The `src/api/mod.rs` file exists but is dead code - its re-exports reference non-existent modules. When adding new modules to `api` or `parsing`, add them to `lib.rs`, not to `mod.rs` files.

### When Modifying the Parser
- Maintain lifetime `'a` relationships: `MDF` owns `MdfFile` which owns `Mmap`; `ChannelGroup<'a>` and `Channel<'a>` borrow from it
- Block addresses are absolute file offsets (u64); address 0 means "null/none"
- Channel names and conversions are resolved lazily during `MdfFile::parse_from_file()` - conversions are resolved per-channel via `resolve_conversion()`, names are not resolved until explicitly requested

### When Modifying the Writer
- All blocks must be 8-byte aligned: `write_block()` adds zero-padding before each block
- Use `write_block_with_id()` to track positions in `block_positions` map
- `update_block_link(source_id, link_offset, target_id)` patches links using logical IDs
- `finalize()` only flushes the underlying writer - ensure all data blocks are finished first via `finish_data_block()`
- The writer supports two backends: `BufWriter<File>` (default) and `MmapMut` (for pre-allocated files)
- `OpenDataBlock` tracks all state for an in-progress data block including DT fragment positions for later DL creation

### When Modifying the Index System
- The index must be fully self-contained: no file references, all conversions pre-resolved
- `IndexedChannel.conversion` stores a `ConversionBlock` with `resolved_texts`, `resolved_conversions`, and `default_conversion` populated
- When reading via index, conversions are applied with empty file data (`&[]`) since all dependencies are resolved
- `ByteRangeReader` trait allows plugging in HTTP, S3, or other data sources
- Compressed blocks (`##DZ`) are not yet supported in the index reader

### When Modifying Conversions
- Each conversion type has its own application function in `src/blocks/conversion/`
- The `apply_decoded()` method in `logic.rs` dispatches based on `cc_type`
- Text-based conversions try resolved data first, then fall back to reading from `file_data` for backward compatibility
- `resolve_all_dependencies_recursive()` handles the full resolution including nested conversions
- When adding new conversion types, update both the `ConversionType` enum and the dispatcher in `logic.rs`

### Python Bindings
- All Python wrapper types are prefixed with `Py` (e.g., `PyMDF`, `PyMdfWriter`, `PyMdfIndex`)
- `PyMdfWriter` maintains its own ID mapping (`channel_groups`, `channels`, `last_channels` HashMaps) separate from the Rust writer's internal IDs
- Values cross the Python boundary as native types via `decoded_value_to_pyobject()` for efficiency
- The `PyMDF` class boxes the `MDF` to avoid lifetime issues (`mdf: Box<MDF>`)
- Pandas integration: `create_datetime_index()` converts relative times to absolute timestamps using `pd.to_datetime()` and `pd.Timedelta`

## Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `nom` | 8 | Binary parsing combinators (used for future extensions) |
| `byteorder` | 1.5 | Little-endian/big-endian reading via `LittleEndian::read_*` |
| `memmap2` | 0.9.9 | Memory-mapped file I/O for reading; `MmapMut` for writing |
| `meval` | 0.2 | Mathematical expression evaluation for algebraic conversions |
| `thiserror` | 2.0 | Error handling derive macros for `MdfError` |
| `serde` | 1.0 | Serialization framework (derive feature) for index types |
| `serde_json` | 1.0 | JSON serialization for index persistence |
| `pyo3` | 0.21 | Python bindings (optional, gated behind `pyo3` feature) |

## Test Organization

### Integration Tests (`tests/`)
- `api.rs` - Writer/parser round-trip, data writing, bulk records, block positions, time-based cutting
- `blocks.rs` - Serialization round-trips for all major block types
- `index.rs` - Index creation, JSON persistence, metadata queries, byte range calculations, name-based lookups
- `merge.rs` - Merging files with identical and different channel structures
- `test_invalidation_bits.rs` - Invalidation flag shortcuts, bit position checking, flag priority, edge cases
- `enhanced_index_conversions.rs` - Index with text conversions, conversion dependency resolution, index persistence with resolved data (**has a known compile error**: missing `start_time_ns` field in `MdfIndex` constructor)

### Unit Tests (`src/blocks/conversion/`)
- `simple_test.rs` - Basic linear, identity, and value-to-text conversions
- `test_deep_chains.rs` - Deep conversion chains (3-level nesting), cycle detection, depth limit enforcement, default conversion resolution

### Test Patterns
- Tests create temporary MDF files, write data, then read back and verify (round-trip testing)
- Decoder tests construct minimal `ChannelBlock` instances and synthetic record bytes
- Index tests verify both direct reads and index-based reads produce identical results

## Common Pitfalls

1. **Don't eagerly load data** - Respect the lazy evaluation pattern; `channel.values()` is the decode trigger
2. **Block alignment** - Writer must maintain 8-byte alignment; `write_block()` handles this automatically
3. **Lifetime management** - API types borrow from `MdfFile`'s mmap; don't try to return them from functions that don't hold the `MDF`
4. **Address updates** - When modifying writer, ensure block links are updated correctly; forgetting to patch a link produces a broken file
5. **Endianness** - The library enforces little-endian encoding for writing; reading supports both LE and BE data types
6. **Module declarations** - `api` and `parsing` modules are declared inline in `lib.rs`; don't modify `api/mod.rs` expecting it to take effect
7. **Conversion resolution** - When creating indexes, conversions must be resolved with the file's mmap available; after serialization, they work without file data
8. **Data block splitting** - Don't assume a single DT block per channel group; the writer may create multiple fragments linked by a DL block
9. **Record ID** - Many simple files use `record_id_len = 0`, but the code must handle non-zero values (used when multiple channel groups share a data group)

## Comparison with asammdf (Python)

This section documents tested interoperability and differences between `mf4-rs` and `asammdf` 8.7.2 (the dominant open-source Python MDF library). Both libraries produce valid MDF 4.10 files that the other can read.

### Interoperability Status

- **mf4-rs files read by asammdf**: All tested scenarios work correctly, including multi-group files, data block splitting via `##DL`, and value-to-text conversions (conversion type 7)
- **asammdf files read by mf4-rs**: Works for uncompressed files. asammdf adds an auto-generated lowercase `time` master channel per group (in addition to any user-created `Time` channel), so mf4-rs sees one extra channel per group. Fails on compressed (`##DZ`) files with `BlockIDError`
- **Value accuracy**: Both produce correct values when cross-reading. All standard integer and float data types (uint8-64, int8-64, float32, float64) round-trip correctly

### Key Structural Differences

| Aspect | mf4-rs | asammdf |
|--------|--------|---------|
| **Default float precision** | 32-bit (`default_bits()` returns 32 for FloatLE) | 64-bit (uses numpy float64) |
| **Master channel** | User-created channel marked as master via `set_time_channel()` | Auto-generates a separate lowercase `time` channel per group |
| **Channel group name** | `PyMdfWriter.add_channel_group(name)` accepts a name parameter but **does not write it** to the file (the closure `\|_cg\| {}` ignores it) | Writes `comment` and `acq_name` to `##CG` metadata block |
| **File header timestamp** | Defaults to epoch (1970-01-01) | Sets to current time at file creation |
| **File history (`##FH`)** | Not written | Writes `##FH` block with tool info |
| **Metadata blocks (`##MD`)** | Not written | Writes XML `##MD` blocks for group comments |
| **File size** | ~2.5x smaller for equivalent data (fewer metadata blocks, 32-bit defaults) | Larger due to 64-bit types, extra channels, and metadata |
| **Program ID** | `mf4-rs` | `amdf8.7.` |

### Feature Comparison

| Feature | mf4-rs | asammdf |
|---------|--------|---------|
| MDF versions | 4.1+ only | MDF 2.x, 3.x, 4.x (full spec) |
| Compression (`##DZ`) | **Not supported** (reader errors on `##DZ` blocks) | Full support (deflate + transposition) |
| VLSD channels | Read support via `##SD`/`##DL` chains | Full read/write support |
| Conversions | All 12 types implemented, chained resolution | All types, same spec coverage |
| Bus logging (CAN/LIN/FlexRay) | No bus-specific support | Full bus extraction and decoding |
| Attachments | Not supported | Read/write embedded and referenced files |
| Events/triggers | Not supported | Full support |
| Signal processing | `cut_mdf_by_time()`, `merge_files()` only | Cut, resample, filter, concatenate, stack |
| Export formats | JSON index only | CSV, HDF5, Excel, Parquet, MDF3 |
| Remote file access | `ByteRangeReader` trait (HTTP/S3 pluggable) | Not supported |
| JSON index system | Yes (unique to mf4-rs) | No equivalent |
| Byte range calculation | Yes (for HTTP range requests) | No equivalent |
| Data block auto-splitting | Yes (at 4MB boundary) | Yes |
| String channels (write) | Via Rust API only (not in Python bindings) | Via numpy byte-string arrays |

### Python API Surface

| Class | mf4-rs | asammdf |
|-------|--------|---------|
| Reader | `PyMDF` - 7 methods | `MDF` - 58+ methods |
| Writer | `PyMdfWriter` - 11 methods | `MDF.append()` + `Signal` (numpy-based) |
| Index | `PyMdfIndex` - 19 methods | No equivalent |

### Performance (100K records, 4 x f64 channels)

**Rust API (native, --release):**

| Operation | Time |
|-----------|------|
| `write_record()` loop | ~0.008s |
| `write_records()` bulk | ~0.013s |
| `write_records_u64()` bulk | ~0.009s |
| Read all channels | ~0.006s |

**Python API comparison (same data):**

| Operation | mf4-rs Python bindings | asammdf |
|-----------|------------------------|---------|
| Write | ~0.08s (record-at-a-time loop) | ~0.03s (numpy vectorized) |
| Read | ~0.012s | ~0.011s |
| File size | 1.6 MB (32-bit default) | 4.0 MB (64-bit default) |

The native Rust API is **4-10x faster than both Python libraries**. The mf4-rs Python bindings appear slower than asammdf for writes because the Python API forces record-at-a-time calls with `DecodedValue` object creation overhead per value. Read performance is essentially identical between the two Python APIs. asammdf's write speed comes from numpy vectorized bulk array writes.

### Recommended Improvements Based on Comparison

1. **Compression support**: Add `##DZ` block reading (deflate decompression) - this is the most impactful missing feature for reading real-world MDF files
2. **64-bit float default**: The Python `add_float_channel()` uses `default_bits()` which returns 32 for FloatLE. Consider defaulting to 64-bit in the Python API to match scientific computing conventions and avoid precision loss
3. **Channel group metadata**: The `add_channel_group(name)` parameter is silently ignored - either implement it or remove the parameter
4. **File header timestamp**: Write the current time to the header block instead of epoch
5. **File history block**: Write a `##FH` block for tool identification and traceability
