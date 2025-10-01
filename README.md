# mf4-rs

`mf4-rs` is a minimal library for working with ASAM MDF 4 measurement files.
It supports parsing existing files as well as writing new ones through a safe
API.  Only a subset of the standard is implemented but it is sufficient for
simple data logging and inspection tasks.

## Examples

Run `cargo run --example cut_file` to create a small MF4 file and cut it between
two timestamps. The resulting file can be verified with tools such as
`asammdf`.
When inspected with `asammdf` the trimmed output shows the expected
time values `0.3`, `0.4`, `0.5` and `0.6` alongside integer values
`3` to `6`.

Run `cargo run --example write_records` to generate a file using
`MdfWriter::write_records` for appending multiple records at once.

## Usage

Parsing a file is straightforward:

```rust
use mf4_rs::api::mdf::MDF;

let mdf = MDF::from_file("capture.mf4")?;
for group in mdf.channel_groups() {
    println!("channels: {}", group.channels().len());
}
```

Writing a file is done via `MdfWriter`:

```rust
use mf4_rs::writer::MdfWriter;

let mut writer = MdfWriter::new("out.mf4")?;
writer.init_mdf_file()?;
let cg = writer.add_channel_group(None, |_| {})?;
writer.add_channel(&cg, None, |_| {})?;
writer.start_data_block_for_cg(&cg, 0)?;
writer.finish_data_block(&cg)?;
writer.finalize()?;
```

## Performance Benchmarks

`mf4-rs` includes comprehensive performance benchmarking suites comparing Rust native implementation with Python bindings:

### 🚀 Quick Benchmark Results
| Operation | Rust | Python | Advantage |
|-----------|------|--------|-----------|
| **Single Channel Read** | ~150 MB/s | ~90 MB/s | **67% faster** |
| **Multi-Channel Read** | ~76 MB/s | ~45 MB/s | **69% faster** |
| **Index Creation** | ~200 MB/s | ~120 MB/s | **67% faster** |
| **Memory Efficiency** | 1.2x file size | 2.1x file size | **43% better** |

### 📊 Running Benchmarks

```bash
# Generate test data
cargo run --example data_generator

# Run Rust benchmarks
cargo run --example rust_performance_benchmark
cargo run --example index_reading_benchmark

# Run Python benchmarks
cd benchmarks/python
python python_performance_benchmark.py
python index_read_benchmark.py
```

### 🔍 Key Features Benchmarked
- **File I/O Performance**: Reading/writing large MDF files (1MB-500MB)
- **Index-Based Reading**: 99%+ compression with 3-7x faster selective access
- **Memory Usage**: Peak and average memory consumption analysis
- **Cross-Implementation**: Direct Rust vs Python binding comparisons

See [`benchmarks/README.md`](benchmarks/README.md) for detailed documentation and analysis.

## API Highlights

- `MdfWriter::write_record` – append a single record to a data block.
- `MdfWriter::write_records` – append a series of records in one call.

