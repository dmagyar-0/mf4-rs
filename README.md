# mf4-rs

`mf4-rs` is a minimal Rust library for working with ASAM MDF 4 (Measurement Data Format) files. It supports parsing existing files as well as writing new ones through a safe API, implementing a subset of the MDF 4.1 specification sufficient for simple data logging and inspection tasks.

The library ships for three ecosystems, all released together from this repository:

| Ecosystem | Package | Install |
|-----------|---------|---------|
| Rust | [`mf4-rs` on crates.io](https://crates.io/crates/mf4-rs) | `cargo add mf4-rs` |
| Python (PyO3 bindings) | [`mf4-rs` on PyPI](https://pypi.org/project/mf4-rs/) | `pip install mf4-rs` |
| TypeScript / JavaScript (WebAssembly bindings) | `mf4-rs` on npm | `npm install mf4-rs` |

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
- `signal.rs` - `Signal` struct pairing channel values with the group's master/time axis

#### 6. Bindings
- `src/python.rs` - Python bindings (PyO3, `pyo3` feature) — see [Python Bindings](#python-bindings)
- `src/wasm.rs` + `js/` - WebAssembly bindings and the TypeScript npm package (`wasm` feature) — see [TypeScript / JavaScript Bindings](#typescript--javascript-bindings-webassembly)

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
- `visualize_layout.rs` - Dumping the block layout of a file
- `wasm-smoke/` - Browser smoke test for the WebAssembly bindings

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

The public index API is **name-based**: channels and groups are addressed by name, not by positional indices.

#### Basic Indexing Workflow:
```rust
use mf4_rs::index::MdfIndex;

// Create an index from an MDF file (never reads sample data — only metadata)
let index = MdfIndex::from_file("data.mf4")?;

// Save index to JSON for later use
index.save_to_file("data_index.json")?;

// Later: load index and read specific channel data. The data source is not
// serialized with the index, so re-attach it after loading:
let mut index = MdfIndex::load_from_file("data_index.json")?;
index.set_file("data.mf4"); // or index.set_url("https://cdn.example.com/data.mf4")

// Lazy read: fetches only the byte ranges the channel needs and returns a
// Signal (values paired with the group's master/time axis)
let signal = index.read("Temperature")?;
println!("{:?} {:?}", signal.timestamps, signal.values);

// Disambiguate duplicate channel names by group
let engine_temp = index.read_in("Engine", "Temperature")?;

// Or build the index directly over HTTP (requires the `http` feature)
let remote = MdfIndex::from_url("https://cdn.example.com/data.mf4")?;

// Power users: compute raw byte ranges (e.g. for HTTP Range requests)
let ranges = index.byte_ranges("Temperature")?;
```

Custom data sources (S3, caches, …) plug in through the `ByteRangeReader` trait: `index.open(reader)` returns an `MdfReader` with `values(name)` / `signal(name)` methods.

## Python Bindings

`mf4-rs` includes high-performance Python bindings generated using `pyo3`. This allows you to use the library's features directly from Python with minimal overhead. Wheels for Linux, macOS, and Windows are published to [PyPI](https://pypi.org/project/mf4-rs/) on every release.

### Installation

```bash
pip install mf4-rs
# or
uv pip install mf4-rs
```

For development against a local checkout, use `maturin` (requires a Rust compiler):

```bash
pip install maturin
maturin develop --release
```

### Python Examples

Check the `python_examples/` directory for complete scripts:

- `write_file.py` - Creating MDF files
- `read_file.py` - Reading and inspecting files
- `index_operations.py` - Using the indexing system
- `pandas_example.py` - Pandas Series / DatetimeIndex integration
- `visualize_layout.py` - Inspecting the block layout of a file

### Basic Usage

All navigation in the Python API is **by name**. `read()` returns a `pandas.Series` (indexed by the group's master channel as a `DatetimeIndex`); `values()` returns a plain numpy `float64` array (invalid samples are `NaN`).

```python
import mf4_rs

# Writing a file
writer = mf4_rs.MdfWriter("output.mf4")
writer.init_mdf_file()
group = writer.add_channel_group("MyGroup")

# Add channels (the time channel is automatically the master)
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
mdf = mf4_rs.Mdf("output.mf4")
for group in mdf.groups:
    print(f"Group: {group.name}, Channels: {[c.name for c in group.channels]}")

series = mdf.read("Data")     # pandas Series with DatetimeIndex
array = mdf.values("Data")    # numpy float64 array (no pandas needed)
same = mdf["Data"]            # __getitem__ is read()

# Lazy remote/indexed reads
index = mf4_rs.MdfIndex.from_file("output.mf4")
index.save("output_index.json")

index = mf4_rs.MdfIndex.load("output_index.json")
index.source = "output.mf4"   # file path or http(s):// URL
series = index.read("Data")   # fetches only the byte ranges it needs
```

## TypeScript / JavaScript Bindings (WebAssembly)

`mf4-rs` also ships WebAssembly bindings (`src/wasm.rs`, built with `wasm-bindgen`) wrapped in a typed TypeScript package under [`js/`](js/), published to npm as `mf4-rs` on every release. It mirrors the name-based API of the Python bindings: an in-memory `Mdf` reader, an `MdfWriter` producing a `Uint8Array`, and a JSON-serialisable `MdfIndex` for lazy, partial reads of large or remote files over any `RangeSource` (HTTP `fetch`, local file, in-memory bytes, or your own).

```ts
import { Mdf } from "mf4-rs";

const mdf = await Mdf.fromFile("./measurement.mf4");
console.log(mdf.channelNames());
const speed = mdf.values("VehicleSpeed");   // number[] (conversions applied, invalid -> NaN)
const signal = mdf.read("VehicleSpeed");    // { timestamps, values }
mdf.dispose();
```

See [`js/README.md`](js/README.md) for the full API, the writer, remote reads via `MdfIndex` + `FetchRangeSource`, and browser-target notes.

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
- `serde` / `serde_json` - JSON serialization for the index system

Optional feature flags:

- `http` - HTTP range reader (`ureq` + vendored TLS) for `MdfIndex::from_url` and URL sources
- `pyo3` - Python bindings (`pyo3`, `numpy`, `pyo3-stub-gen`; implies `http`)
- `wasm` - WebAssembly bindings (`wasm-bindgen`, `js-sys`, `serde-wasm-bindgen`)

## Releases

Releases are fully automated. Every merge to `main` runs `.github/workflows/release.yml`, which:

1. Reads commit messages since the last `v*` tag.
2. Computes a SemVer bump from [Conventional Commits](https://www.conventionalcommits.org/) prefixes (`feat:` → minor, `fix:`/`perf:` → patch, `feat!:` / `BREAKING CHANGE:` → major; other types do not produce a release).
3. Updates `Cargo.toml`, `pyproject.toml`, `Cargo.lock`, `js/package.json`, `js/package-lock.json`, and `CHANGELOG.md`, then tags `vX.Y.Z`.
4. Builds wheels for Linux (x86_64, aarch64), macOS (x86_64, aarch64), and Windows (x64), plus an sdist, and publishes them to [PyPI](https://pypi.org/project/mf4-rs/) via OIDC trusted publishing.
5. Publishes the Rust crate to [crates.io](https://crates.io/crates/mf4-rs).
6. Builds the WebAssembly bindings with `wasm-pack`, compiles and tests the TypeScript wrapper, and publishes the `mf4-rs` npm package.

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
- **npm**: the `mf4-rs` TypeScript/WebAssembly package from `js/`. The publish runs the package's `prepublishOnly` script (wasm-pack build, TypeScript compile, full js test suite), so a failing build or test aborts the publish. Authentication uses the `NPM_TOKEN` repository secret.
- **GitHub Releases**: a release named `vX.Y.Z` whose body is the same changelog section appended to `CHANGELOG.md`.

### Manual triggers and recovery

- **Re-run a release**: the `Release` workflow accepts `workflow_dispatch`, so a maintainer can re-run it from the Actions tab. If the tag already exists, `bump-and-tag` will fail; in that case re-run only the `publish-pypi` / `publish-crates` / `publish-npm` jobs from the original run.
- **Skip a release intentionally**: use a non-releasing prefix (`chore:`, `docs:`, `refactor:`, `test:`, `ci:`, `build:`, `style:`) for the PR title. The workflow will run, compute `bump=none`, and exit cleanly without creating a tag.
- **First-time setup**: a PyPI Trusted Publisher entry for `mf4-rs` pointing at `dmagyar-0/mf4-rs` workflow `release.yml`, environment `pypi`; a `CARGO_REGISTRY_TOKEN` secret; an `NPM_TOKEN` secret (npm automation token with publish rights on the `mf4-rs` npm package); and — if branch protection blocks `GITHUB_TOKEN` pushes to `main` — a `RELEASE_PAT` secret with `contents:write`. All are complete for this repo except `NPM_TOKEN`, which must be added before the first npm publish.
