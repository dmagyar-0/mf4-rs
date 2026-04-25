# mf4-rs

`mf4-rs` is a minimal Rust library for working with ASAM MDF 4 (Measurement Data Format) files. It supports parsing existing files as well as writing new ones through a safe API, implementing a subset of the MDF 4.1 specification sufficient for simple data logging and inspection tasks.

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

## Usage

### Building and Testing
```bash
# Build the project
cargo build

# Run all tests
cargo test

# Run specific test file
cargo test --test api
```

### Examples

The project includes simplified examples in the `examples/` directory:

- `write_file.rs` - Comprehensive example of writing MDF files with multiple channels
- `read_file.rs` - Demonstrates parsing and inspecting MDF files
- `index_operations.rs` - Shows advanced indexing, byte-range reading, and conversion resolution
- `merge_files.rs` - Merging multiple MF4 files
- `cut_file.rs` - Time-based file cutting
- `python_equivalent.rs` - Comparison with Python functionality

Run them with:
```bash
cargo run --example write_file
cargo run --example read_file
cargo run --example index_operations
```

### Working with MDF Files

#### Basic File Creation Pattern:
```rust
use mf4_rs::writer::MdfWriter;
use mf4_rs::blocks::common::DataType;
use mf4_rs::parsing::decoder::DecodedValue;

let mut writer = MdfWriter::new("output.mf4")?;
writer.init_mdf_file()?;
let cg = writer.add_channel_group(None, |_| {})?;

// Create master channel (usually time)
let time_ch_id = writer.add_channel(&cg, None, |ch| {
    ch.data_type = DataType::FloatLE;
    ch.name = Some("Time".to_string());
    ch.bit_count = 64;
})?;
writer.set_time_channel(&time_ch_id)?; // Mark as master channel

// Add data channels with master as parent
writer.add_channel(&cg, Some(&time_ch_id), |ch| {
    ch.data_type = DataType::UnsignedIntegerLE;
    ch.name = Some("DataChannel".to_string());
    ch.bit_count = 32;
})?;

writer.start_data_block_for_cg(&cg, 0)?;
writer.write_record(&cg, &[
    DecodedValue::Float(1.0),              // Time
    DecodedValue::UnsignedInteger(42),     // Data
])?;
writer.finish_data_block(&cg)?;
writer.finalize()?;
```

#### Basic File Parsing Pattern:
```rust
use mf4_rs::api::mdf::MDF;

let mdf = MDF::from_file("input.mf4")?;
for group in mdf.channel_groups() {
    println!("channels: {}", group.channels().len());
    for channel in group.channels() {
        let values = channel.values()?;
        // Process values...
    }
}
```

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
```

## Python Bindings

`mf4-rs` includes high-performance Python bindings generated using `pyo3`. This allows you to use the library's features directly from Python with minimal overhead.

### Installation

You can install the package directly using `pip` or `uv` (requires a Rust compiler):

```bash
pip install .
# or
uv pip install .
```

For development, you can use `maturin`:

```bash
# Install maturin
pip install maturin

# Build and install in current environment
maturin develop --release
```

### Python Examples

Check the `python_examples/` directory for complete scripts:

- `write_file.py` - Creating MDF files
- `read_file.py` - Reading and inspecting files
- `index_operations.py` - Using the indexing system

### Basic Usage

```python
import mf4_rs

# Writing a file
writer = mf4_rs.PyMdfWriter("output.mf4")
writer.init_mdf_file()
group = writer.add_channel_group("MyGroup")

# Add channels
time_ch = writer.add_time_channel(group, "Time")
data_ch = writer.add_float_channel(group, "Data")

# Write data
writer.start_data_block(group)
writer.write_record(group, [
    mf4_rs.create_float_value(0.1),  # Time
    mf4_rs.create_float_value(42.0)  # Data
])
writer.finish_data_block(group)
writer.finalize()

# Reading a file
mdf = mf4_rs.PyMDF("output.mf4")
for group in mdf.channel_groups():
    print(f"Group: {group.name}, Channels: {group.channel_count}")
```

## Performance

`mf4-rs` is designed for high performance:
- Use `write_records` for batch operations instead of multiple `write_record` calls
- Data blocks automatically split when they exceed 4MB to maintain performance
- Memory-mapped file access minimizes memory usage for large files
- Channel values are decoded lazily only when accessed
- **Use indexing for repeated access** to the same files to avoid re-parsing overhead

*Note: Previous benchmarks have been removed as they are being updated.*

## Dependencies

- `nom` - Binary parsing combinators
- `byteorder` - Endianness handling  
- `memmap2` - Memory-mapped file I/O
- `meval` - Mathematical expression evaluation for formula conversions
- `thiserror` - Error handling derive macros

## Releases

Releases are fully automated. Every merge to `main` runs `.github/workflows/release.yml`, which:

1. Reads commit messages since the last `v*` tag.
2. Computes a SemVer bump from [Conventional Commits](https://www.conventionalcommits.org/) prefixes (`feat:` → minor, `fix:`/`perf:` → patch, `feat!:` / `BREAKING CHANGE:` → major; other types do not produce a release).
3. Updates `Cargo.toml`, `pyproject.toml`, `Cargo.lock`, and `CHANGELOG.md`, then tags `vX.Y.Z`.
4. Builds wheels for Linux (x86_64, aarch64), macOS (x86_64, aarch64), and Windows (x64), plus an sdist, and publishes them to [PyPI](https://pypi.org/project/mf4-rs/) via OIDC trusted publishing.
5. Publishes the Rust crate to [crates.io](https://crates.io/crates/mf4-rs).

PR titles are linted against the convention by `.github/workflows/pr-title.yml`. Because PRs are squash-merged, **the PR title is the source of truth** for both the changelog and the version bump. See [`CHANGELOG.md`](CHANGELOG.md) for release history.

### How PR titles map to version bumps

| PR title prefix (or footer)                                       | Bump        | Example                                              |
|-------------------------------------------------------------------|-------------|------------------------------------------------------|
| `feat!:` / `<type>!:` / body contains `BREAKING CHANGE:`          | **major**   | `feat!: drop Python 3.8 support`                     |
| `feat:`                                                           | **minor**   | `feat: add ##DZ block decompression for index reads` |
| `fix:` / `perf:`                                                  | **patch**   | `fix: handle empty CHANGELOG.md on first release`    |
| `chore:` / `docs:` / `refactor:` / `test:` / `ci:` / `build:` / `style:` | _no release_ | `docs: clarify VLSD record format`                   |

If multiple commits land between releases, the **highest** bump wins. Runs with no qualifying commits are a clean no-op — no tag is created and no artifacts are published, which is why the `chore(release): vX.Y.Z` commit pushed back by the workflow does not trigger another release.

### What gets published

Each successful release run publishes, in parallel, after the tag is pushed:

- **PyPI**: `mf4-rs` wheels for Linux (x86_64, aarch64), macOS (x86_64 on 13, aarch64 on 14), and Windows (x64), plus an sdist. Authentication uses [PyPI Trusted Publishing](https://docs.pypi.org/trusted-publishers/) (OIDC) — no long-lived API token is stored. The `pypi` GitHub Actions environment gates this job.
- **crates.io**: the `mf4-rs` Rust crate, gated by a `cargo publish --dry-run` step. Authentication uses the `CARGO_REGISTRY_TOKEN` repository secret.
- **GitHub Releases**: a release named `vX.Y.Z` whose body is the same changelog section appended to `CHANGELOG.md`.

### Manual triggers and recovery

- **Re-run a release**: the `Release` workflow accepts `workflow_dispatch`, so a maintainer can re-run it from the Actions tab. If the tag already exists, `bump-and-tag` will fail; in that case re-run only the `publish-pypi` / `publish-crates` jobs from the original run.
- **Skip a release intentionally**: use a non-releasing prefix (`chore:`, `docs:`, `refactor:`, `test:`, `ci:`, `build:`, `style:`) for the PR title. The workflow will run, compute `bump=none`, and exit cleanly without creating a tag.
- **First-time setup** (already complete for this repo): a PyPI Trusted Publisher entry for `mf4-rs` pointing at `dmagyar-0/mf4-rs` workflow `release.yml`, environment `pypi`; a `CARGO_REGISTRY_TOKEN` secret; and — if branch protection blocks `GITHUB_TOKEN` pushes to `main` — a `RELEASE_PAT` secret with `contents:write`.
