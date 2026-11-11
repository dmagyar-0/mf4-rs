# mf4-rs API

One API, three languages. The Rust crate, the Python package (`mf4_rs`) and
the TypeScript package (`mf4-rs`) expose the same classes, methods, types and
behaviour. This page is the specification; [`api-contract.json`](api-contract.json)
is the same contract in machine-readable form, and a test in each language
fails when a binding and the contract disagree (see [Keeping it in sync](#keeping-it-in-sync)).

## The rules

1. **Same names.** Names are written `snake_case` in Rust and Python and
   `camelCase` in TypeScript (`time_range` / `timeRange`). Nothing else differs.
2. **Everything is addressed by name** — channel names, optionally narrowed by
   a group name. There are no numeric group/channel indices.
3. **Two readers, one reading API.** `Mdf` reads a whole file; `MdfIndex` reads
   lazily from a file or URL. Every reading method exists on both with the same
   arguments and the same results.
4. **`read` returns a `Signal`** (values + timestamps); **`values` returns plain
   `float64` numbers** (invalid or non-numeric samples are `NaN`).
5. **One error rule.** Metadata lookups (`group`, `channel`) return *nothing*
   for an unknown name. Data operations (`read`, `values`, `time_range`,
   `conversion`, `byte_ranges`) fail with a *channel not found* error.

The only differences between languages are the ones the language forces:

| | Rust | Python | TypeScript |
|---|---|---|---|
| Zero-argument accessors (`groups`, `channel_names`, `start_time_ns`, `file_size`, `source`) | method | property | method |
| Optional arguments | passed explicitly: `None`, `Some("group")`, or `&ReadOptions` | keyword arguments | a `ReadOptions` object |
| "nothing" | `None` | `None` | `undefined` (`null` for `conversion`) |
| Arrays | `Vec<f64>` | `numpy.ndarray` | `Float64Array` |
| `MdfIndex` reads | blocking | blocking (GIL released) | `async` |
| Errors | `MdfError` | `MdfException` | `Error` |
| Channel not found | `MdfError::ChannelNotFound` | `ChannelNotFoundError` | `ChannelNotFoundError` |

## At a glance

**Rust**

```rust
use mf4_rs::{Mdf, MdfIndex, ReadOptions};

let mdf = Mdf::from_file("recording.mf4")?;
for group in mdf.groups()? {
    println!("{:?}: {} records, {:?}", group.name, group.record_count, group.channel_names);
}
let speed = mdf.read("Speed", None)?;                            // Signal
let rpm = mdf.values("RPM", &ReadOptions::new().group("Engine"))?;
let part = mdf.read("Speed", &ReadOptions::new().time(Some(1.0), Some(2.5)))?;

let index = MdfIndex::from_url("https://host/recording.mf4")?;   // metadata only
let speed = index.read("Speed", None)?;                          // range requests happen now
```

**Python**

```python
import mf4_rs

mdf = mf4_rs.Mdf.from_file("recording.mf4")
for group in mdf.groups:
    print(group.name, group.record_count, group.channel_names)
speed = mdf.read("Speed")                         # Signal
rpm = mdf.values("RPM", group="Engine")           # numpy float64
part = mdf.read("Speed", start_time=1.0, end_time=2.5)
speed.to_series()                                 # pandas Series, datetime index

index = mf4_rs.MdfIndex.from_url("https://host/recording.mf4")   # metadata only
speed = index.read("Speed")                       # range requests happen now
```

**TypeScript**

```ts
import { Mdf, MdfIndex } from "mf4-rs";

const mdf = await Mdf.fromFile("recording.mf4");
for (const group of mdf.groups()) {
  console.log(group.name, group.recordCount, group.channelNames);
}
const speed = mdf.read("Speed");                           // Signal
const rpm = mdf.values("RPM", { group: "Engine" });        // Float64Array
const part = mdf.read("Speed", { startTime: 1.0, endTime: 2.5 });

const index = await MdfIndex.fromUrl("https://host/recording.mf4");  // metadata only
const lazySpeed = await index.read("Speed");               // range requests happen now
```

## Reading: `Mdf` and `MdfIndex`

| | `Mdf` | `MdfIndex` |
|---|---|---|
| Holds | the whole file (memory-mapped / in memory) | metadata only; a JSON-serialisable index |
| Reads from | itself | its `source` — a file path or URL — fetching only the bytes a call needs |
| Use it for | local files | remote files, huge files, repeated access through a saved index |

### Opening

| Contract | `Mdf` | `MdfIndex` | Notes |
|---|---|---|---|
| `from_file(path)` | ✓ | ✓ | TypeScript: Node only. For `MdfIndex` the file becomes the source. |
| `from_bytes(data)` | ✓ | ✓ | Python and Rust `MdfIndex`: no source is attached; TypeScript binds the bytes as the source. |
| `from_url(url)` | ✓ downloads the whole file | ✓ fetches **only metadata**; the URL becomes the source | Rust needs the `http` feature. |
| `from_json(json)` | | ✓ | Reload a serialised index. The source is not part of the JSON. |
| `load(path)` | | ✓ | `from_json` from a file. TypeScript: Node only. |

### Metadata

| Contract | Returns | |
|---|---|---|
| `groups` | list of `GroupInfo` | Every channel group, in file order. |
| `channel_names` | list of names | Every named channel across all groups (duplicates kept). |
| `start_time_ns` | integer or nothing | Measurement start, nanoseconds since the Unix epoch. |
| `group(name)` | `GroupInfo` or nothing | First group with that name. |
| `channel(name, group?)` | `ChannelInfo` or nothing | First channel with that name; `group` restricts the search. |

### Data

| Contract | Returns | |
|---|---|---|
| `read(name, options?)` | `Signal` | Values with conversions applied, paired with the group's time axis. |
| `values(name, options?)` | `float64` array | Just the numbers — no timestamps. Invalid and non-numeric samples are `NaN`. |
| `time_range(name, group?)` | `TimeRange` or nothing | First and last master (time) value of the channel's group; nothing for an empty group. On `MdfIndex` this reads two records. |
| `conversion(name, group?)` | `ConversionInfo` or nothing | The conversion `read` / `values` apply, fully resolved. |

`options` is a `ReadOptions`:

| Field | |
|---|---|
| `group` | Look only in the group(s) with this name. A column-oriented file has one group per signal, all named after the source group — every group with the name is searched. |
| `start_time`, `end_time` | Inclusive time window in the master channel's unit (seconds relative to the measurement start). Either bound may be omitted. |
| `start_record`, `record_count` | Record window `[start_record, start_record + record_count)`. Reaching past the last record is an error. |

Pass a time window **or** a record window, not both. On `MdfIndex` a window
fetches only the selected records; a time window is located by searching the
master channel with a handful of single-record reads. (Exception:
variable-length channels — text and byte arrays — are fetched whole and then
cut to the window.)

How `options` is passed:

```rust
mdf.read("Speed", None)?;                                              // Rust: None = no options
mdf.read("Speed", &ReadOptions::new().group("Engine").records(0, 100))?;
```
```python
mdf.read("Speed", group="Engine", start_record=0, record_count=100)    # Python: keywords
mdf["Speed"], mdf["Speed", "Engine"]                                   # shorthand for read
```
```ts
mdf.read("Speed", { group: "Engine", startRecord: 0, recordCount: 100 });  // TypeScript: object
```

### `MdfIndex` only

| Contract | |
|---|---|
| `to_json()` / `save(path)` | Serialise the index (`save` is Node-only in TypeScript). |
| `validate()` | Check a loaded index for internal consistency. |
| `file_size` | Size in bytes of the indexed file. |
| `source` / `set_source(source)` | The file path or URL reads fetch from. Not serialised — re-attach it after `load` / `from_json`. |
| `byte_ranges(name, group?)` | `(offset, length)` spans holding a channel's samples, for issuing range requests yourself. Refused for variable-length channels. |
| `byte_ranges_for_records(name, start_record, record_count, group?)` | The same for a record window. |

The source is a string in Rust and Python (`http(s)://` URLs use range
requests, anything else is a file path). In TypeScript it is a `RangeSource`
object (`FetchRangeSource`, `FileRangeSource`, `BytesRangeSource`, or your own).

### `Mdf` only

| Contract | |
|---|---|
| `file_layout()` | A `FileLayout` describing every block in the file. Rust and Python. |

## Types

All four are plain data with identical fields in every language.

**`Signal`** — what `read` returns.

| Field | |
|---|---|
| `name` | Channel name. |
| `unit` | Physical unit, if any. |
| `timestamps` | Master-channel values as `float64`. Empty when the group has no master, or when the channel *is* the master (its `values` are the time axis). |
| `values` | One value per record, conversions applied; an invalid sample is `None` / `NaN` / `null`. |

`values` holds numbers, text or bytes: `Vec<Option<DecodedValue>>` in Rust, a
`ChannelValue[]` in TypeScript, and in Python a `float64` array for numeric
channels or an `object` array of `str` / `bytes` / `None` otherwise. Python
adds `Signal.to_series()` (a `pandas.Series` with a `DatetimeIndex` when the
file records a start time); Rust adds `Signal::values_f64()`.

**`GroupInfo`** — `name`, `comment`, `record_count`, `channel_names`,
`master_channel` (name of the time channel, local or the remote master of a
column-oriented group), `channels` (list of `ChannelInfo`).

**`ChannelInfo`** — `name`, `unit`, `comment`, `data_type`, `bit_count`,
`is_master`, `is_vlsd`, `has_conversion`.

`data_type` is a string, the same in every language: `FloatLE`, `FloatBE`, `UnsignedIntegerLE`,
`UnsignedIntegerBE`, `SignedIntegerLE`, `SignedIntegerBE`, `StringUtf8`,
`StringLatin1`, `StringUtf16LE`, `StringUtf16BE`, `ByteArray` (plus, when
reading, `MimeSample`, `MimeStream`, `CanOpenDate`, `CanOpenTime`, `ComplexLE`,
`ComplexBE`, `Unknown`). `MdfWriter.add_channel` takes the same names (the
writable subset is listed under [Writing](#writing-mdfwriter)). In Rust
`ChannelInfo.data_type` is this string and the writer takes the `DataType`
enum whose variants carry the same names.

**`ConversionInfo`** — `type` (`Identity`, `Linear`, `Rational`, `Algebraic`,
`TableLookupInterp`, `TableLookupNoInterp`, `RangeLookup`, `ValueToText`,
`RangeToText`, `TextToValue`, `TextToText`, `BitfieldText`), `values` (the
block's numeric parameters — `[offset, factor]` for `Linear`), `precision`,
`flags`, and when present `physical_range` (`[min, max]`), `formula`, `texts`
and `refs` (keyed by reference index; for `ValueToText`, `values[i]` maps to
`texts[i]` and index `len(values)` is the default) and `default`. A struct in
Rust (field `conversion_type`), a `dict` in Python, an object in TypeScript
(whose `texts` / `refs` keys are strings, as all JS object keys are).

**`TimeRange`** — `start`, `end`. A struct in Rust, a `(start, end)` tuple in
Python, an object in TypeScript.

## Writing: `MdfWriter`

Rust and Python write to a file: `MdfWriter::new(path)` / `MdfWriter(path)`,
and `finalize()` closes it. TypeScript runs in wasm without a filesystem:
`new MdfWriter()` writes to memory and `finalize()` returns the file as a
`Uint8Array`. Everything in between is identical.

| Step | Contract | |
|---|---|---|
| 1 | `init_mdf_file()` | Write the file header. Stamps the current time (not in TypeScript). |
| | `set_start_time(ns)` | Set the measurement start, nanoseconds since the Unix epoch. |
| 2 | `add_channel_group(name?)` → id | |
| | `set_channel_group_comment(group, comment)` | |
| 3 | `add_time_channel(group, name)` → id | 64-bit float master channel. Add it first. |
| | `add_float_channel` / `add_float32_channel` | 64 / 32-bit float. |
| | `add_int_channel` / `add_sint_channel` | 64-bit **unsigned** / **signed** integer. |
| | `add_string_channel` | Variable-length UTF-8 text. |
| | `add_channel(group, name, data_type)` | A channel by data-type name: `FloatLE`, `UnsignedIntegerLE`, `SignedIntegerLE` (each **32 bits** wide), a text type or `ByteArray` (variable-length channels; text in the type's own encoding). Other types, including the big-endian ones, can be read but not written and are refused. |
| | `set_time_channel(channel)` | Mark an existing channel as the master. |
| | `add_linear_conversion(channel, offset, factor)` | `physical = offset + factor * raw`. |
| | `add_value_to_text_conversion(channel, mapping, default_text)` | `mapping` is a list of `(raw integer, text)` pairs. |
| 4 | `start_data_block(group)` | After all channels and conversions of the group. |
| | `write_record(group, values)` | One value per channel, in the order the channels were added. |
| | `write_columns(group, columns)` | Many records at once: one typed array per channel. The fast path. |
| | `finish_data_block(group)` | |
| 5 | `finalize()` | |

Record values are plain values in Python and TypeScript (`float` / `int` /
`str` / `bytes`; `number` / `bigint` / `string` / `Uint8Array`) and
`DecodedValue`s in Rust. Python and TypeScript accept and refuse exactly the
same values (the rules live once, in the Rust core):

- float channels take any number;
- integer channels take integers, and numbers with an integral value — never
  a fraction, `NaN`, a negative into an unsigned channel, or a value that does
  not fit the channel's width;
- text channels take text only, `ByteArray` channels bytes only;
- booleans are refused.

A refused value is an error. Integers are never silently truncated, on any
write path (`write_record` and `write_columns` alike); a float written to a
32-bit float channel is rounded to `float32` as usual. Columns are `numpy` arrays (`float64`, `float32`,
`uint64`, `int64`), typed arrays (`Float64Array`, `Float32Array`,
`BigUint64Array`, `BigInt64Array`) or `ColumnData` slices.

```python
w = mf4_rs.MdfWriter("out.mf4")
w.init_mdf_file()
g = w.add_channel_group("Engine")
w.add_time_channel(g, "Time")
speed = w.add_float_channel(g, "Speed")
w.add_linear_conversion(speed, 0.0, 3.6)            # m/s -> km/h
w.start_data_block(g)
w.write_record(g, [0.0, 12.5])
w.write_columns(g, [numpy.arange(1.0, 4.0), numpy.ones(3)])
w.finish_data_block(g)
w.finalize()
```

## File tools

| Contract | Rust | Python | TypeScript | |
|---|---|---|---|---|
| `to_column_oriented(input, output, options)` → `ColumnReport` | ✓ | ✓ | ✓ | Rewrite a file as column-oriented MDF 4.20 — one group per signal, metadata at the front, each signal's samples contiguous — so a remote `MdfIndex` reads one signal with one or two range requests. Streaming, bounded memory. |
| `cut_mdf_by_time(input, output, start, end)` | ✓ | ✓ | | Keep the records whose time lies in `[start, end]` seconds. |
| `cut_mdf_by_utc(input, output, start, end)` | ✓ | ✓ | | The same with absolute UTC bounds (nanoseconds in Rust; `datetime`, ISO string or seconds in Python). |
| `merge_files(output, first, second)` | ✓ | ✓ | | Concatenate two files; groups with identical layouts are joined. |
| `FileLayout` | ✓ | ✓ | | Block-level layout inspector (`from_file`, `to_text`, `to_tree`, `to_json`). |

`to_column_oriented` options are `block_size`, `memory_budget` and
`share_identical_time` (merge byte-identical time axes); the report has
`source_groups`, `time_groups`, `signal_groups`, `shared_time_groups`,
`data_blocks`, `metadata_size`, `file_size` and `skipped_channels`. TypeScript
takes and returns bytes (`toColumnOriented(bytes, options)` →
`{ bytes, report }`), with `toColumnOrientedFile` (Node) and
`toColumnOrientedStream` for files too large to hold in memory.

The cut, merge and layout tools are path-based and not part of the TypeScript
package.

## Language extensions

Each language may add what only it can offer. These are listed under `extra`
in `api-contract.json`, so they are deliberate rather than drift.

**Rust**

- Block access underneath the contract: `Mdf::channel_groups()` → `ChannelGroup` → `Channel` (`decoded_values()`, `values()`, `conversion()`, `block()`), `Mdf::find_channel`, the public `MdfIndex.channel_groups` field, `MdfIndex::indexed_channel` and `conversion_block`.
- Custom byte sources: implement `ByteRangeReader`, build with `MdfIndex::from_range_reader`, read through `index.open(reader)` → `MdfReader`, which has the same `read` / `values` / `time_range` methods.
- Low-level writer building blocks: `add_channel_with`, `add_channel_group_with`, `start_data_block_with`, `write_records*`, `write_raw_record`, `write_block`, …

**Python**

- `obj[name]` and `obj[name, group]` — shorthand for `read`.
- `Signal.to_series()` — pandas integration.

**TypeScript**

- `dispose()` on every class (wasm memory is not garbage-collected); `using` is supported.
- `ReadOptions.source` — read one call from a different `RangeSource`.
- `MdfIndex.fromRangeSource(source)` plus the fragment API (`signalByteRanges`, `valuesFromFragments`, `readFromFragments`) for callers that fetch bytes themselves.
- `mf4-rs/web` — the browser entry point; `await init()` once before use.

## Keeping it in sync

Two things are checked, by one test file per language:

- **Names.** [`api-contract.json`](api-contract.json) lists every class,
  method, accessor, type field, function and export; each test compares it
  with what its binding really exposes.
- **Behaviour.** Each language builds the same file with its own `MdfWriter`,
  answers the calls listed in
  [`tests/contract/scenario.json`](../tests/contract/scenario.json) with both
  `Mdf` and `MdfIndex` — whole and windowed reads, groups without a master,
  empty groups, text encodings, error cases — and must reproduce
  [`tests/contract/golden.json`](../tests/contract/golden.json) exactly. The
  same file lists which values `write_record` must accept and refuse.

| Language | Test | Run |
|---|---|---|
| Rust | `tests/api_contract.rs` | `cargo test --test api_contract` |
| Python | `tests/test_api_contract.py` | `python tests/test_api_contract.py` |
| TypeScript | `js/test/contract.test.ts` | `cd js && npm test` |

To change the API:

1. Edit `api-contract.json` and this page.
2. Implement the change in Rust (`src/api/mdf.rs`, `src/index.rs`, `src/writer/mdf_writer/simple.rs`, `src/types.rs`).
3. Mirror it in `src/python.rs` (then regenerate the stubs: `cargo run --features pyo3 --bin stub_gen`) and in `src/wasm.rs` + `js/src/`.
4. Run the three tests. A name that exists in only one language must be listed under that language's `extra`, or the tests fail.
5. If answers change on purpose, regenerate the golden file with `UPDATE_GOLDEN=1 cargo test --test api_contract` — the Python and TypeScript tests must then match it without further changes. New behaviour deserves a new case in `scenario.json`.

The `.github/workflows/tests.yml` workflow runs all three on every pull request.
