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

The Python API is the same as the Rust and TypeScript ones — it is specified
in [`docs/API.md`](../docs/API.md) and checked by `tests/test_api_contract.py`.
Everything is addressed by **name**, with an optional `group` to disambiguate
duplicate channel names.

- `mf4_rs.Mdf` — whole-file reader. Open with `Mdf.from_file(path)`,
  `from_bytes(data)` or `from_url(url)`.
- `mf4_rs.MdfIndex` — lazy / remote reader with **the same reading API**. Open
  with `from_file`, `from_bytes`, `from_url` (fetches only metadata),
  `from_json` or `load`; `to_json()` / `save(path)`; `source` /
  `set_source(path_or_url)`.
- Both readers: `groups`, `channel_names`, `start_time_ns` (properties),
  `group(name)`, `channel(name, group=None)`, `read(...)` → `Signal`,
  `values(...)` → numpy `float64`, `time_range(name, group=None)`,
  `conversion(name, group=None)`, and `obj[name]` as shorthand for `read`.
- `read` / `values` take `group=`, and either `start_time=` / `end_time=` or
  `start_record=` / `record_count=` to read only part of a channel.
- `mf4_rs.MdfWriter(path)` — writer: `init_mdf_file()`, `add_channel_group()`,
  `add_time_channel()` / `add_float_channel()` / `add_int_channel()` / …,
  `add_linear_conversion()`, `start_data_block()`, `write_record()` /
  `write_columns()`, `finish_data_block()`, `finalize()`.
- File tools: `to_column_oriented`, `cut_mdf_by_time`, `cut_mdf_by_utc`,
  `merge_files`, `FileLayout`.

A `Signal` has `name`, `unit`, `timestamps` and `values` (numpy arrays);
`signal.to_series()` gives a `pandas.Series` with a `DatetimeIndex` built from
the file's start time.

## Examples

Run `write_file.py` first — it creates the `example.mf4` the others read.

| Script | Shows |
|---|---|
| `write_file.py` | Channel groups, typed channels, conversions, `write_record` and the bulk `write_columns`. |
| `read_file.py` | Metadata, `read` → `Signal`, `values` → numpy, group and window options, `time_range`, `conversion`, error handling. |
| `index_operations.py` | Building, saving and loading an `MdfIndex`, attaching a source, lazy and windowed reads, byte ranges. |
| `pandas_example.py` | `Signal.to_series()`, resampling, building a `DataFrame` from a group. |
| `visualize_layout.py` | `FileLayout`: block types, addresses, sizes, links and gaps. |

```python
import mf4_rs

mdf = mf4_rs.Mdf.from_file("example.mf4")
for group in mdf.groups:
    print(group.name, group.record_count, group.channel_names)

speed = mdf.read("Speed")                              # Signal
speed.to_series().resample("1s").mean()                # pandas
numbers = mdf.values("Speed", start_time=1.0, end_time=2.0)   # numpy

index = mf4_rs.MdfIndex.from_url("https://cdn.example.com/data.mf4")
speed = index.read("Speed", group="Vehicle")           # range requests happen now
```

## Error Handling

Every error is an `mf4_rs.MdfException`. Reading a channel that does not
exist raises its subclass `mf4_rs.ChannelNotFoundError`; the metadata lookups
`group()` / `channel()` return `None` instead.

```python
try:
    mdf = mf4_rs.Mdf.from_file("nonexistent.mf4")
except mf4_rs.MdfException as e:
    print(f"MDF Error: {e}")
```

## Performance Tips

- Use `values()` instead of `read()` when you don't need timestamps — it is the fastest read path.
- Pass a time or record window to read only part of a channel; through an `MdfIndex` only those records are fetched.
- Use `write_columns()` for bulk writes instead of per-record `write_record()` calls.
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
