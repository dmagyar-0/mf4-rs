# mf4-rs

`mf4-rs` is a minimal Rust library for working with ASAM MDF 4 (Measurement Data Format) files. It supports parsing existing files as well as writing new ones through a safe API, implementing a subset of the MDF 4.1 specification sufficient for simple data logging and inspection tasks.

The library ships for three ecosystems, all released together from this repository:

| Ecosystem | Package | Install |
|-----------|---------|---------|
| Rust | [`mf4-rs` on crates.io](https://crates.io/crates/mf4-rs) | `cargo add mf4-rs` |
| Python (PyO3 bindings) | [`mf4-rs` on PyPI](https://pypi.org/project/mf4-rs/) | `pip install mf4-rs` |
| TypeScript / JavaScript (WebAssembly bindings) | `mf4-rs` on npm | `npm install mf4-rs` |

**The API is the same in all three languages** — the same classes (`Mdf`, `MdfIndex`, `MdfWriter`), the same methods, the same types. It is specified in [`docs/API.md`](docs/API.md) and enforced by a contract test per language.

```python
mdf = mf4_rs.Mdf.from_file("recording.mf4")        # Python
speed = mdf.read("Speed", group="Engine")
```
```rust
let mdf = Mdf::from_file("recording.mf4")?;         // Rust
let speed = mdf.read("Speed", &ReadOptions::new().group("Engine"))?;
```
```ts
const mdf = await Mdf.fromFile("recording.mf4");    // TypeScript
const speed = mdf.read("Speed", { group: "Engine" });
```

## Architecture

### High-Level Structure

The codebase is organized into distinct layers:

#### 1. API Layer (`src/api/`)
- **High-level user-facing API** for working with MDF files
- `Mdf` - Whole-file reader implementing the cross-language contract (`groups`, `read`, `values`, …)
- `ChannelGroup` / `Channel` - Borrowed block wrappers underneath it, for raw block access
- `src/types.rs` - The contract's plain data types (`GroupInfo`, `ChannelInfo`, `ConversionInfo`, `ReadOptions`, `TimeRange`)

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
- `column.rs` - Streaming conversion to column-oriented MDF 4.20 files optimised for cloud reads
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
- `column_oriented.rs` - Converting a file to the column-oriented MDF 4.20 layout
- `wasm-smoke/` - Browser smoke test for the WebAssembly bindings

Run them with:
```bash
cargo run --example write_file
cargo run --example read_file
cargo run --example index_operations
```

### Working with MDF Files

Everything below is the contract documented in [`docs/API.md`](docs/API.md); the Python and TypeScript APIs have the same methods.

#### Writing a file
```rust
use mf4_rs::{DecodedValue, MdfWriter};

let mut writer = MdfWriter::new("output.mf4")?;
writer.init_mdf_file()?;
let group = writer.add_channel_group(Some("Engine"))?;

writer.add_time_channel(&group, "Time")?;            // master channel, add it first
let speed = writer.add_float_channel(&group, "Speed")?;
writer.add_int_channel(&group, "Gear")?;
writer.add_linear_conversion(&speed, 0.0, 3.6)?;     // m/s -> km/h

writer.start_data_block(&group)?;
writer.write_record(&group, &[
    DecodedValue::Float(0.0),
    DecodedValue::Float(12.5),
    DecodedValue::UnsignedInteger(3),
])?;
writer.finish_data_block(&group)?;
writer.finalize()?;
```

For bulk data use `write_columns` (one slice per channel). The closure-based building blocks (`add_channel_with`, `add_channel_group_with`, …) remain available when you need full control over the written blocks.

#### Reading a file
```rust
use mf4_rs::{Mdf, ReadOptions};

let mdf = Mdf::from_file("output.mf4")?;
for group in mdf.groups()? {
    println!("{:?}: {} records, channels {:?}", group.name, group.record_count, group.channel_names);
}

let speed = mdf.read("Speed", None)?;    // Signal { name, unit, timestamps, values }
let numbers = mdf.values("Speed", None)?; // Vec<f64>, NaN for invalid samples

// Narrow by group and/or read only a time or record window
let window = mdf.read("Speed", &ReadOptions::new().group("Engine").time(Some(1.0), Some(2.5)))?;

mdf.time_range("Speed", None)?;          // first/last timestamp
mdf.conversion("Speed", None)?;          // the conversion read/values apply
```

### Lazy and remote reads with `MdfIndex`

`MdfIndex` has the same reading API as `Mdf`, but holds only metadata: it remembers a data **source** (a file path or URL) and fetches just the bytes each call needs — with HTTP range requests for URLs. The index serialises to JSON, so a file needs to be indexed only once.

```rust
use mf4_rs::{MdfIndex, ReadOptions};

// Index a local file (never reads sample data) and save the index
let index = MdfIndex::from_file("data.mf4")?;
index.save("data_index.json")?;

// Later: load the index. The source is not serialised, so re-attach it.
let mut index = MdfIndex::load("data_index.json")?;
index.set_source("data.mf4");            // or "https://cdn.example.com/data.mf4"

let signal = index.read("Temperature", None)?;
let engine_temp = index.read("Temperature", &ReadOptions::new().group("Engine"))?;
let last_minute = index.values("Temperature", &ReadOptions::new().time(Some(540.0), None))?;

// Or index a remote file directly, fetching only its metadata (`http` feature)
let remote = MdfIndex::from_url("https://cdn.example.com/data.mf4")?;

// Power users: compute raw byte ranges (e.g. for HTTP Range requests)
let ranges = index.byte_ranges("Temperature", None)?;
```

Custom data sources (S3, caches, …) plug in through the `ByteRangeReader` trait: `index.open(reader)` returns an `MdfReader` with the same `read` / `values` / `time_range` methods.

### Column-oriented MDF 4.20 for cloud reads

A regular (row-oriented) MDF file interleaves every channel of a group record
by record, so reading one signal over HTTP still downloads all of its group's
data. `mf4_rs::column::to_column_oriented` rewrites any uncompressed MDF 4 file
(sorted or unsorted) into a **column-oriented MDF 4.20** file built for
single-signal cloud reads:

- **One channel group per signal**, its data blocks (default 4 MiB, whole
  records, tunable via `block_size`) written **back to back** and listed by an
  equal-length `##DL` block — one range request fetches the whole signal.
- **Shared timestamps** via the MDF 4.2 *remote master* (`cg_cg_master`, channel
  group flag bit 3): each source group's time channel is written once into its
  own group and referenced by all of its signals. Source groups whose time axes
  are byte-identical share a single time group (`share_identical_time`).
- **All metadata at the front** (`##ID`/`##HD`/`##FH`/`##DG`/`##CG`/`##CN`/`##TX`/
  `##MD`/`##CC`/`##SI`/`##DL`), so the whole structure — including every data
  block's location and size — comes from one prefix fetch. Attachments go to
  the end of the file.
- **Streaming and lossless**: the layout is computed from the metadata first,
  then the source is read once in chunks and each channel's raw bytes are
  appended to small per-channel buffers flushed straight to their final
  offsets. Memory stays bounded by `memory_budget` (default 64 MiB) whatever
  the file size; nothing is decoded, so conversions, units, comments, sources,
  invalidation bits and VLSD (`##SD`) channels carry over unchanged.

```rust
use mf4_rs::{to_column_oriented, ColumnOptions};

let report = to_column_oriented("recording.mf4", "recording.col.mf4", &ColumnOptions::default())?;
println!("{} signals, {} time groups, metadata prefix {} bytes",
         report.signal_groups, report.time_groups, report.metadata_size);
```

The output reads like any other MDF file: `Mdf`, `MdfIndex`, the Python and
TypeScript bindings and asammdf all resolve the remote master transparently
(`read("Speed")` comes back with its shared timestamps). Lower-level entry
points: `to_column_oriented_with(reader, sink, options)` over any
`ByteRangeReader` and `ColumnSink`, and `to_column_oriented_bytes` in memory.

Range requests for a cold read of one signal from a fresh file (measured in
`tests/column.rs` / `tests/test_column_oriented.py` / `js/test/column.test.ts`):

| Step | Column-oriented file | Row-oriented original |
|------|----------------------|------------------------|
| Build the index (`MdfIndex::from_url`) | size probe + **1** prefix fetch; the data region is never touched | size probe + metadata + data-block headers |
| Read one numeric signal | **2** (values + shared time; **1** when they are adjacent) | same count, but downloads every channel of the group |
| Bytes to read one 400k-sample f64 signal (values + timestamps) of a 9-channel group | **6.3 MB** | 56 MB |
| Extra for a lazily resolved conversion / a VLSD `##SD` stream | +1 each | +1 each |

Converting a 1 GB file (3 groups × 21 channels × 2M records) takes about 4.5 s
with a peak resident set of ~70 MB. Not supported yet: compressed (`##DZ`)
input. Array (`##CA`) channels and VLSD data stored in VLSD channel groups are
skipped and listed in `ColumnReport::skipped_channels`.

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

The Python API is the contract in [`docs/API.md`](docs/API.md): `read()` returns a `Signal` (numpy `values` + `timestamps`, with `.to_series()` for pandas), `values()` returns a plain numpy `float64` array.

```python
import mf4_rs

# Writing a file
writer = mf4_rs.MdfWriter("output.mf4")
writer.init_mdf_file()
group = writer.add_channel_group("MyGroup")
writer.add_time_channel(group, "Time")       # master channel, add it first
writer.add_float_channel(group, "Data")

writer.start_data_block(group)
writer.write_record(group, [0.1, 42.0])      # plain values, one per channel
writer.finish_data_block(group)
writer.finalize()

# Reading a file
mdf = mf4_rs.Mdf.from_file("output.mf4")
for group in mdf.groups:
    print(f"Group: {group.name}, Channels: {group.channel_names}")

signal = mdf.read("Data")        # Signal: .values, .timestamps, .name, .unit
series = signal.to_series()      # pandas Series with a DatetimeIndex
array = mdf.values("Data")       # numpy float64 array
same = mdf["Data"]               # __getitem__ is read()
part = mdf.read("Data", start_time=0.0, end_time=1.0)   # or start_record/record_count

# Column-oriented MDF 4.20 copy for cloud reads (streaming, bounded memory)
report = mf4_rs.to_column_oriented("output.mf4", "output.col.mf4")
print(report)  # ColumnReport(signal_groups=..., time_groups=..., ...)

# Lazy remote/indexed reads — same reading API as Mdf
index = mf4_rs.MdfIndex.from_file("output.mf4")
index.save("output_index.json")

index = mf4_rs.MdfIndex.load("output_index.json")
index.set_source("output.mf4")   # file path or http(s):// URL
signal = index.read("Data")      # fetches only the byte ranges it needs
```

## TypeScript / JavaScript Bindings (WebAssembly)

`mf4-rs` also ships WebAssembly bindings (`src/wasm.rs`, built with `wasm-bindgen`) wrapped in a typed TypeScript package under [`js/`](js/), published to npm as `mf4-rs` on every release. It exposes the same API as the Rust crate and the Python package ([`docs/API.md`](docs/API.md)): an in-memory `Mdf` reader, an `MdfWriter` producing a `Uint8Array`, and a JSON-serialisable `MdfIndex` for lazy, partial reads of large or remote files over any `RangeSource` (HTTP `fetch`, local file, in-memory bytes, or your own).

```ts
import { Mdf } from "mf4-rs";

const mdf = await Mdf.fromFile("./measurement.mf4");
console.log(mdf.channelNames());
const speed = mdf.values("VehicleSpeed");   // Float64Array (conversions applied, invalid -> NaN)
const signal = mdf.read("VehicleSpeed");    // Signal { name, unit, timestamps, values }
const part = mdf.read("VehicleSpeed", { group: "Engine", startTime: 1.0, endTime: 2.5 });
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
- **npm**: the `mf4-rs` TypeScript/WebAssembly package from `js/`. The publish runs the package's `prepublishOnly` script (wasm-pack build, TypeScript compile, full js test suite), so a failing build or test aborts the publish. Authentication uses [npm Trusted Publishing](https://docs.npmjs.com/trusted-publishers) (OIDC) — no long-lived token is stored, and npm attaches a provenance attestation automatically.
- **GitHub Releases**: a release named `vX.Y.Z` whose body is the same changelog section appended to `CHANGELOG.md`.

### Manual triggers and recovery

- **Re-run a release**: the `Release` workflow accepts `workflow_dispatch`, so a maintainer can re-run it from the Actions tab. If the tag already exists, `bump-and-tag` will fail; in that case re-run only the `publish-pypi` / `publish-crates` / `publish-npm` jobs from the original run.
- **Skip a release intentionally**: use a non-releasing prefix (`chore:`, `docs:`, `refactor:`, `test:`, `ci:`, `build:`, `style:`) for the PR title. The workflow will run, compute `bump=none`, and exit cleanly without creating a tag.
- **First-time setup**: a PyPI Trusted Publisher entry for `mf4-rs` pointing at `dmagyar-0/mf4-rs` workflow `release.yml`, environment `pypi`; a `CARGO_REGISTRY_TOKEN` secret; an npm Trusted Publisher entry on the `mf4-rs` npm package (package **Settings → Trusted Publisher**: GitHub Actions, organization `dmagyar-0`, repository `mf4-rs`, workflow `release.yml`); and — if branch protection blocks `GITHUB_TOKEN` pushes to `main` — a `RELEASE_PAT` secret with `contents:write`. All are complete for this repo. Note that npm's Trusted Publisher settings live on the package page, so the package must exist first: bootstrap it once by publishing manually from a logged-in machine (`cd js && npm run build:wasm && npm run build && npm publish --access public`), then configure the Trusted Publisher — every subsequent release publishes tokenlessly from CI.
