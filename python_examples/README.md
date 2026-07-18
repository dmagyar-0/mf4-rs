# MF4-RS Python Bindings Examples

This directory contains Python examples demonstrating how to use the mf4-rs Python bindings for working with ASAM MDF 4 measurement files.

## Installation

Wheels for Linux, macOS, and Windows are published to PyPI on every release:

```bash
pip install mf4-rs
```

### Building from source (development)

For working against a local checkout you need a Rust toolchain
([rustup.rs](https://rustup.rs/)), Python 3.8+, and
[maturin](https://github.com/PyO3/maturin):

```bash
pip install maturin

# Build in development mode (creates a .pyd/.so that Python can import)
maturin develop --release

# Or build a wheel for distribution
maturin build --release
pip install target/wheels/mf4_rs-*.whl
```

**Note**: The `pyo3` feature is automatically enabled by maturin via `pyproject.toml`, so you don't need to specify `--features pyo3`.

## API at a Glance

The Python API is **name-based** — channels and groups are addressed by name, with an optional `group` argument to disambiguate duplicate channel names. There are no positional `(group_index, channel_index)` arguments.

- `mf4_rs.Mdf(path)` — reader. `groups`, `group(name)`, `channel(name)`, `channel_names`, `read(name, group=None)` → `pandas.Series`, `values(name, group=None)` → numpy `float64` array, `mdf[name]` (same as `read`), `file_layout()`.
- `mf4_rs.MdfWriter(path)` — writer. `init_mdf_file()`, `add_channel_group(name)`, `add_time_channel()`, `add_float_channel()`, `add_int_channel()`, `add_string_channel()`, `start_data_block()`, `write_record()` / `write_columns_f64()`, `finish_data_block()`, `finalize()`.
- `mf4_rs.MdfIndex` — lightweight JSON index for lazy reads. `from_file(path)`, `from_url(url)`, `save(path)`, `load(path)`, settable `source` property (file path or `http(s)://` URL), lazy `read()` / `values()`, `byte_ranges()`, `conversion_info()`.

`read()` returns a `pandas.Series` indexed by the group's master (time) channel converted to a `DatetimeIndex` using the file's start time. `values()` is pandas-free and returns a plain numpy `float64` array with invalid samples as `NaN`.

## Examples

### 1. read_file.py - Reading MDF Files

```python
import mf4_rs

mdf = mf4_rs.Mdf("example.mf4")

# Inspect structure
for group in mdf.groups:
    print(group.name, [c.name for c in group.channels])

print(mdf.channel_names)

# Read a channel as a pandas Series with DatetimeIndex
series = mdf.read("Temperature")
print(series.describe())

# Or as a plain numpy float64 array (no pandas required)
values = mdf.values("Temperature")

# Disambiguate duplicate channel names by group
engine_temp = mdf.read("Temperature", group="Engine")

# Dict-style access (same as read)
series = mdf["Temperature"]
```

### 2. write_file.py - Creating MDF Files

```python
import mf4_rs

writer = mf4_rs.MdfWriter("output.mf4")
writer.init_mdf_file()

group_id = writer.add_channel_group("Test Group")

# Convenience methods handle data types, bit counts, and channel linking.
# add_time_channel automatically marks the channel as the group master.
time_ch = writer.add_time_channel(group_id, "Time")
temp_ch = writer.add_float_channel(group_id, "Temperature")
speed_ch = writer.add_int_channel(group_id, "Speed")

writer.start_data_block(group_id)
for i in range(100):
    writer.write_record(group_id, [
        mf4_rs.create_float_value(i * 0.01),   # Time
        mf4_rs.create_float_value(25.5),       # Temperature
        mf4_rs.create_uint_value(60),          # Speed
    ])
writer.finish_data_block(group_id)
writer.finalize()
```

For bulk numeric data, `write_columns_f64(group_id, columns)` writes whole
columns in one call and is much faster than a `write_record` loop.

### 3. index_operations.py - MDF File Indexing

The index is a small, self-contained JSON document holding all metadata
(including fully resolved conversions) needed to read channel data with
targeted byte-range I/O — no full file parse or download required.

```python
import mf4_rs

# Create an index (reads metadata only, never sample data)
index = mf4_rs.MdfIndex.from_file("data.mf4")
index.save("data_index.json")

# Later: load the index and attach a data source. The source is not
# serialized with the index, so set it after loading.
index = mf4_rs.MdfIndex.load("data_index.json")
index.source = "data.mf4"                          # local file...
# index.source = "https://cdn.example.com/data.mf4"  # ...or HTTP URL

# Lazy reads: only the needed byte ranges are fetched
series = index.read("Temperature")      # pandas Series
values = index.values("Temperature")    # numpy float64 array

# Indexes can also be built straight from a URL
index = mf4_rs.MdfIndex.from_url("https://cdn.example.com/data.mf4")

# Power users: raw byte ranges (e.g. for custom HTTP Range requests)
ranges = index.byte_ranges("Temperature")
first_10 = index.byte_ranges_for_records("Temperature", 0, 10)

# Inspect the (pre-resolved) conversion attached to a channel
info = index.conversion_info("Temperature")
```

### 4. pandas_example.py - Pandas Integration

`read()` returns a pandas Series whose index is a `DatetimeIndex` built from
the MDF file's absolute start time plus the group's relative master-channel
times, enabling full time-series functionality:

```python
import mf4_rs
import pandas as pd

mdf = mf4_rs.Mdf("data.mf4")

temp = mdf.read("Temperature")
speed = mdf.read("Speed")

print(temp.index)          # DatetimeIndex with absolute timestamps
print(temp.describe())

temp_1s = temp.resample("1s").mean()      # resample to 1-second intervals
window = temp.loc["2024-01-15 10:30:00":"2024-01-15 10:30:10"]

df = pd.DataFrame({"Temperature": temp, "Speed": speed})
print(df.corr())
```

### 5. visualize_layout.py - File Layout Inspection

`Mdf.file_layout()` (also `FileLayout.from_file(path)`) walks every block in
the file and reports block types, addresses, sizes, links, and gaps — handy
for debugging writers or comparing against files produced by other tools.

```python
import mf4_rs

layout = mf4_rs.Mdf("data.mf4").file_layout()
print(layout.to_tree())        # or to_text() / to_json()
```

## Error Handling

All operations can raise `mf4_rs.MdfException` for MDF-specific errors:

```python
try:
    mdf = mf4_rs.Mdf("nonexistent.mf4")
except mf4_rs.MdfException as e:
    print(f"MDF Error: {e}")
```

## Performance Tips

- Use `values()` instead of `read()` when you don't need timestamps — it skips pandas entirely and is the fastest read path.
- Use `write_columns_f64()` / `write_columns()` for bulk writes instead of per-record `write_record()` calls.
- Use the indexing system for repeated or remote access: index creation is a one-time metadata scan, after which reads fetch only the byte ranges they need.
- `Mdf.read` / `Mdf.values` release the GIL while decoding.
- The underlying Rust library uses memory-mapped files, so large files are handled without loading them into memory.

## Indexing Use Cases

- **HTTP/remote file access**: conversions are pre-resolved into the index, so no extra requests are needed during reads
- **Cloud storage optimization**: precise byte ranges minimize bandwidth
- **Fast channel browsing** without opening or downloading entire files
- **Selective data extraction** for specific channels or record ranges
- **Microservice architectures** where index creation and data access are separated

## Compatibility

These bindings expose the core functionality of the Rust mf4-rs library:
- Reading/writing MDF 4.1 compliant files
- All 12 MDF conversion types with chained resolution
- VLSD (variable-length) string/byte channels, including through the index
- Memory-efficient handling of large files
- Fast indexing system for metadata and selective data access

Type stubs (`.pyi`) ship with the wheel, so IDEs show full signatures and
docstrings on hover.
