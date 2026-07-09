# mf4-rs Deep Codebase Review & asammdf Cross-Validation

**Date:** 2026-07-09 · **Version reviewed:** v2.0.0 (`ddbad6f`) · **Reference:** asammdf 8.8.22, Python 3.11, numpy/pandas current

> **FIX STATUS (2026-07-09, this branch):** All confirmed bugs below have been fixed on this
> branch except where noted. Summary of the fixes:
>
> - **A1/A2** fixed — `values_as_f64` / `Mdf.values()` / `signal()` now apply conversions and
>   invalidation (NaN), matching `MdfIndex` and asammdf; both paths pick the *first* master.
> - **A3** fixed — every index read/byte-range path now errors clearly on VLSD channels
>   (offset-based SD reads remain unimplemented — error instead of garbage).
> - **A4/A5/A6** mitigated by loud refusal — unsorted data groups and non-record-aligned DL
>   fragments now return clear errors on all read paths (parser + index) instead of silently
>   mis-decoding. Full record-ID demux / fragment-spanning support remains future work.
> - **A7** fixed — the writer hard-errors on unencodable channels (fixed-length strings, BE,
>   CanOpen, complex) and on value/encoder type mismatches; the Python API gained
>   `add_sint_channel` and `add_string_channel` (VLSD), both verified readable by asammdf.
> - **A8** fixed — merge compares **and preserves** conversions/units/comments/sources and
>   `sync_type`; rejects invalidation-bit files; header start time taken from the first file.
>   Time axes are still concatenated verbatim (documented).
> - **A9** fixed — cut refuses multi-CG data groups, preserves `record_id`, uses scale-aware
>   inclusive bounds, no longer aborts on non-monotonic masters, and cloned channels keep
>   their exact byte offsets (`add_channel_preserving_offsets`).
> - **A10–A12, D1, D3, D4, D6** fixed — bit_offset/f16 rejected with errors, invalidation
>   bytes included in record framing, `dl_equal_length` is now the data-section length,
>   `write_record_u64` splits at 4 MB, no empty first fragments, VLSD channels forced to
>   bit_count 64 with no sentinel link on disk.
> - **B1–B8** fixed (B5: `X1` alias supported; eval errors still fall back to raw). JSON
>   indexes survive ±inf/NaN conversion values losslessly.
> - **C1–C8** fixed — malformed/truncated files return `MdfError` (no panics), cycle detection
>   in all block walks, hostile index JSON validated, `write_columns_f64` UB removed,
>   HTTP range reads verify 206/length, caching reader handles EOF, errors use `Display`.
> - **D2** fixed (`##DV` parses), **D5** partially fixed (header stamped with current time;
>   `##FH` block still not written), **D7** fixed.
> - **F** fixed — Python `read()` uses a numpy fast path + Rust-built `datetime64[ns]` index:
>   ~0.15 s for 4×1M samples (was ~1.35 s); GIL released during decodes.
> - New regression suites: `tests/fix_conversions.rs`, `tests/fix_parser.rs`,
>   `tests/fix_index.rs`, `tests/fix_writer.rs`, `tests/fix_api.rs` (83 new tests).
>   Cross-validation: 30/30 pass; asammdf interop: 15/15 pass.
>
> Still open (features, not bugs): ##DZ compression, ##FH block, unsorted-file decoding,
> offset-based VLSD reads in the index, channel composition (##CA), virtual channels,
> float16 decode, `hd_tz_offset` in the pandas DatetimeIndex.

## Methodology

1. **Static deep review** of all ~5,000 lines of Rust across five subsystems (parser/decoder, writer, index, conversions, cut/merge/Python bindings), with link offsets re-derived byte-by-byte from the MDF 4.1 block layouts and semantics cross-checked against asammdf's `v4_blocks.py` source. Several writer findings were verified at runtime with probe programs.
2. **Empirical cross-validation** (`review/xval.py`, 30 checks): asammdf-written files (MDF 4.10/4.11/4.20, all integer/float types with boundary values, VLSD strings, byte arrays, linear/algebraic/value-to-text/range-to-text conversions, invalidation bits, ##DZ compression) read back through *three* mf4-rs paths (direct `Mdf`, `MdfIndex.from_file`, and index→JSON→reload); mf4-rs-written files (record loop, `write_columns*`) read back by asammdf; `cut`/`merge` outputs validated by asammdf. The repo's own 15-test interop suite and the full `cargo test` suite pass.
3. **Performance benchmark** (`review/bench.py`): 1M records × 4 f64 channels + time, vs asammdf.
4. **Real-world sample files:** direct downloads were attempted from CSS Electronics (CANedge samples) and python-can's test data, but this environment's network proxy blocks GitHub outside the session repo and CSS's file server returned 403. As a proxy for real-world data, files were generated with asammdf itself (the dominant open-source MDF writer) across versions 4.10–4.20 and feature configurations. Findings marked "third-party-file risk" below are traced from code against the spec, not reproduced with a vendor file.

**Result summary: 23/30 cross-validation checks pass.** The 7 failures, plus static review, yield the findings below. `cargo test` (76 tests) and the interop suite (15 tests) all pass — none of the bugs below are covered by existing tests.

---

## A. Confirmed bugs — silent data corruption

These produce wrong values *without any error*.

### A1. `Mdf.values()` (direct) skips conversions and invalidation bits; `MdfIndex.values()` applies them
`src/python.rs:684` → `Channel::values_as_f64` (`src/api/channel.rs:179`). **Empirically confirmed:** for a channel with a linear conversion, `Mdf(path).values("Lin")` returns raw `[0,1,2,…]` while `MdfIndex.from_file(path).values("Lin")` and asammdf return `[0.5, 3.5, 6.5,…]`. The docstring claims "Invalid / non-numeric samples are NaN" — invalidation bits are also never checked on this path. The same raw-vs-physical divergence applies to `signal()`/timestamps (A2).

### A2. Master/time axis ignores the master channel's conversion (direct API only)
`src/api/channel_group.rs:102` builds the time axis with `values_as_f64` (no conversion). A master stored as integer ticks with a `0.001·x` linear conversion (common) yields timestamps 1000× off via `MDF::signal()`, while `MdfIndex.read()` applies the conversion — index and direct reads of the *same file* disagree. Also: direct picks the *last* `channel_type==2` channel as master, index picks the *first*.

### A3. Index reads of VLSD channels return garbage on the file/slice paths
`src/index.rs:1635,1678,1717` have no VLSD guard (only `read_channel_values` at `:994` errors). The index's channel block is rebuilt with `data: 0` (`index.rs:107-142`), so the decoder decodes the 8-byte SD **offset** in the record as if it were the payload. **Empirically confirmed:** `MdfIndex.from_file(f).read("Text")` on an asammdf VLSD string channel returns `['', 'D', '<Invalid UTF8>', …]` while direct read returns the correct strings. URL-sourced reads error; file-sourced reads silently fabricate data.

### A4. VLSD record↔value pairing ignores the stored offsets
`src/parsing/raw_channel.rs:37-135` enumerates the ##SD stream sequentially and assumes entry *i* belongs to record *i*, never reading the u64 offset in the fixed record. Files that de-duplicate repeated strings or write out-of-order entries (CANape does) get values shifted against the time axis. Cut inherits this via lockstep pairing (`src/cut.rs:386-432`) and *bakes the wrong pairing into the output file*. Third-party-file risk: works for mf4-rs/asammdf-written files, wrong for offset-using writers.

### A5. Records spanning ##DL fragment boundaries are silently mis-framed
`src/parsing/raw_channel.rs:152-161`, `src/api/channel.rs:128-215`, `src/index.rs:1022,1051,1551`: each fragment is independently truncated to whole records. The spec allows fragments split at arbitrary byte positions (that's what `dl_offset`/equal-length fields describe); asammdf concatenates fragments before framing. A file split mid-record decodes garbage from that point on, with a wrong sample count and no error. A cheap defense — `(size-24) % record_size == 0` and `Σ records == cycle_count` — is absent. Third-party-file risk.

### A6. Unsorted data groups (multiple CGs per DG, `record_id_len > 0`) decode garbage
Record IDs are parsed (`channel_group_block.rs:52`) but never compared during iteration (`raw_channel.rs:140-157`, `index.rs:1051`, `cut.rs:409`). Interleaved records of different sizes are chunked by one CG's record size → both groups return plausible-looking wrong values. This should at minimum be a hard error. CLAUDE.md pitfall #9 claims this "must be handled"; it is not. This is the standard layout for CAN bus logs (CANedge etc.). Third-party-file risk, high real-world exposure.

### A7. Writer silently writes zeros for unsupported/mismatched values
`src/writer/mdf_writer/data.rs:159-173` maps `String*` (fixed-length), all `*BE` types, and CanOpen types to `ChannelEncoder::Skip`; `encode()`'s `_ => {}` (`data.rs:64`) swallows type mismatches (e.g. `SignedInteger` value into a `UInt` channel). **Empirically confirmed twice:** (a) a string channel created through the Python `add_channel(..., create_data_type_string_utf8())` produces a file where every value is `""` — the Python API has *no* way to create a working (VLSD) string channel, despite exposing `create_string_value`; (b) `create_int_value(-1000)` written into an `add_int_channel` (which is documented unsigned) silently stores 0. These must be hard errors.

### A8. `merge_files` drops conversions, units, invalidation bits, and master sync flags
`src/merge.rs` — empirically confirmed side effects:
- Layout identity ignores conversions/units (`:9-33`); output channels are written with **no** ##CC/unit at all (`:167-188`) → merged files return raw instead of physical values, and files with *different* scalings merge silently.
- `sync_type` is never copied (`:183`) → merged masters have `channel_type=2, sync_type=0`; `cut_mdf_by_time` on a merged file finds no master and copies everything; asammdf won't detect the master.
- Time axes are concatenated verbatim, not offset (`:151-158`) — **empirically confirmed:** merged time runs 0→50s then 0→50s again (non-monotonic).
- Invalidation bits dropped (`GroupMeta` lacks `invalidation_bytes_nr`; decode path ignores validity) → invalid samples become "valid".
- BE-typed / fixed-string channels in either input are zeroed via A7.

### A9. Cut: multi-CG topology broken, offsets rewritten, boundary epsilon wrong
- `src/cut.rs:235-243` + `init.rs:94-102`: for a source DG with 2+ CGs the output contains one DG with both CGs chained *and* an orphan DG holding the second CG's data — structurally corrupt.
- `cut.rs:243`: output `##CG.record_id` left 0 while copied records keep the source ID bytes.
- `init.rs:234-238` (triggered from `cut.rs:305`): any cloned channel with a legitimate `byte_offset == 0` (second bit-field in byte 0, or channel list not in byte order) gets its offset silently rewritten → decodes wrong bytes.
- `cut.rs:454-460`: upper bound uses absolute `f64::EPSILON` (meaningless at t≫2s) and the first record past `end_time` aborts *all* remaining blocks (assumes monotonic time, undocumented).

### A10. Writer: `bit_offset` ignored; sub-byte channels written wrong (runtime-verified)
`data.rs:41-66,152-154`: encoders never shift by `cn_bit_offset`, although the record is sized for it and the CN block declares it. A `bit_count=4, bit_offset=4` channel written with value `0xA` reads back 0 — even by mf4-rs itself. Two channels sharing a byte overwrite each other.

### A11. Writer: multi-CG groups and record IDs unwritable (runtime-verified)
Record IDs are never stamped into records (declared `record_id=7` → on-disk byte 0 → asammdf drops all records); a second `start_data_block` on the same DG overwrites the single `dg_data` link, orphaning the first DT.

### A12. Writer: declared invalidation bytes break record framing
`data.rs:129-147`: `record_size` excludes `invalidation_bytes_nr` and `cg_inval_bytes@100` is never patched on the normal path — a user setting `cg.invalidation_bytes_nr = N` in the closure gets a file whose declared stride ≠ written stride: every reader misframes all records after the first. Only the `_raw` path handles it.

---

## B. Confirmed bugs — wrong values in conversions

(Semantics cross-checked against asammdf's `v4_blocks.py`.)

- **B1. Rational denominator guard falsifies results** — `linear.rs:44-47`: `den.abs() > f64::EPSILON` treats any tiny denominator as zero and *returns the raw value*. `y=1/x` at `x=1e-20` returns `1e-20` instead of `1e20`. Should just divide (IEEE inf/NaN on true zero), as asammdf does.
- **B2. ValueToText/RangeToText with NIL default returns `Unknown`/NaN instead of the raw value** — `text.rs:47-53,113-119`. asammdf (and CANape) pass the raw value through. Empirically visible as `nan` vs asammdf's raw/empty output.
- **B3. Single-pair lookup tables rejected** — `table_lookup.rs:10`: a spec-valid one-pair table (`cc_val=[k,v]`) silently passes raw through instead of returning `v`.
- **B4. BitfieldText: direct ##TX refs dropped; panic on named sub-conversions via index** — `bitfield.rs:24,57` skip TX-only bit groups (asammdf emits them); `bitfield.rs:28-29` calls `read_string_block(&[], addr)` on every index read of a bitfield with named sub-conversions → **panic** (see C1).
- **B5. Algebraic: `X1` alias unsupported; eval errors silently return raw** — `linear.rs:59-64`. Real-world files (MDF3-converted) use `X1`; asammdf rewrites it. A failed/missing formula silently passes raw values through.
- **B6. Index JSON with non-finite `cc_val` cannot be reloaded** — `serde_json` writes `±inf`/NaN as `null` and fails to deserialize into `f64`. Range tables commonly use ±inf catch-all bounds, and BitfieldText masks stored as f64 bit patterns (e.g. `0xFFFF_FFFF_FFFF_FFFF`) are NaN patterns. `save()` succeeds, `load()` errors — breaking the index system's core contract for such files.
- **B7. `cn_flags` bit 0 ("all invalid") ignored when the group has no invalidation bytes** — `api/channel.rs:126` and `index.rs:1093` gate all validity checks on `invalidation_bytes_nr > 0`; per spec bit 0 applies regardless. `values()`/`values_f64` return garbage as valid; `read()` paths disagree with `values()` paths.
- **B8. TextToValue resolved path iterates a `HashMap`** — `text.rs:164-180`: duplicate key texts → nondeterministic result; unresolved keys silently fall to default.

## C. Confirmed bugs — panics on malformed input (crash, not `MdfError`)

The parser trusts every file-provided offset. In Python these surface as `pyo3_runtime.PanicException`, escaping the documented "catch `MdfException`" contract (`python.rs:1940`).

- **C1. `read_string_block`** (`common.rs:237`): unchecked `&mmap[offset..offset+24]` — reachable from every channel-name/unit lookup on truncated files, and from conversion application with empty `file_data` (index reads; see B4).
- **C2. `SourceBlock::from_bytes`** (`source_block.rs:45-71`): links read before any length check; data-section check off by one (`+2` vs index `+2`).
- **C3. `DataListBlock::from_bytes`** (`data_list_block.rs:43`): `links_nr==0` → `0usize - 1` underflow.
- **C4. File-structure walks:** `mdf_file.rs:80-94` (files <168 bytes; DG/CG addresses), `channel_group_block.rs:151`, `raw_data_group.rs:40-57`, `raw_channel.rs:79-94`, `block_layout.rs:341,380,787,829` — all unchecked mmap slicing.
- **C5. No cycle detection in DG/CG/CN/##DL linked-list walks** — a back-pointing `next` link loops forever with unbounded allocation (conversions have cycle detection; the structural walk does not).
- **C6. Hostile/stale index JSON:** `db.size < 24` underflow, `record_size == 0` division, `bit_count == 0` shift underflow, unchecked `Vec::with_capacity(cycles_nr)` (`index.rs:1022+`, `api/channel.rs:77`, `decoder.rs:347`).
- **C7. VLSD numeric payload shorter than `bit_count/8`** → `byteorder` panic (`decoder.rs:282-309`).
- **C8. `write_columns_f64` unsound pointer cast** — `data.rs:766-769`: `&mut [f64]` constructed from a `Vec<u8>` pointer with aligned writes; UB per Rust rules (works only because allocators 8-align large blocks). Reachable from Python.

## D. Confirmed spec violations / interop hazards (writer)

- **D1. `dl_equal_length` off by 24** (runtime-verified): `data.rs:940-950` stores the full block length including the 24-byte header; spec (and asammdf) use the data-section length. Readers that random-access via `offset / dl_equal_length` (Vector tooling) land 24·k bytes off. One-line fix.
- **D2. `##DV` blocks can never parse** — `raw_data_group.rs:43-48` matches `"##DV"` but `DataBlock::ID = "##DT"` makes `from_bytes` reject it: the arm always errors. MDF 4.2 column-storage files fail despite explicit intent to support them.
- **D3. FloatLE with `bit_count` ∉ {32,64}** gets the 8-byte encoder → buffer panic or neighbor clobbering (half-floats are spec-valid; reader side also lacks float16 → silent `None`).
- **D4. `write_record_u64` never auto-splits at 4 MB** (`data.rs:425-443`), unlike every other write path; mixing paths produces unequal fragments under an equal-length DL.
- **D5. Missing `##FH`** (spec requires ≥1) and HD `abs_time` defaulting to `1970-01-01T02:00` (`header_block.rs:161` — undocumented 2-hour magic constant, CLAUDE.md says "epoch").
- **D6. VLSD writer footguns:** the `ch.data = 1` sentinel is written to disk and only patched in `finish_data_block` (early error → invalid link); VLSD with default `bit_count` (8) reserves 1 byte but the encoder writes 8.
- **D7. HTTP range reads never verify 206/`Content-Range`** (`index.rs:588-620`): a server that ignores `Range` returns 200 + full body → the first `length` bytes are silently used as if they were the requested range. Short bodies are also accepted silently. `CachingRangeReader` over-reads past EOF, erroring with strict inner readers (`index.rs:399-431`) — the documented WASM pattern fails whenever metadata lands in the last partial chunk.

## E. Functional differences vs asammdf (not necessarily bugs)

| Behavior | mf4-rs | asammdf 8.8 |
|---|---|---|
| Unmatched value in V2T/R2T, NIL default | `None`/NaN (B2) | raw value / `b''` |
| Overlapping RangeToText boundary (x=3 in [0,3]+[3,7]) | `'low'` (first match — spec-conformant) | no match → default (searchsorted artifact) |
| `values()` output | **raw** (A1) | physical |
| Interior-NUL strings | keeps bytes after first NUL | truncates at first NUL |
| UTF-16 odd byte count | whole sample → `None` | truncates trailing byte |
| Timezone | `read()` DatetimeIndex is tz-naive UTC; `hd_tz_offset` ignored | tz handling per file header |
| `byte_ranges()` | one coalesced span per fragment, *includes other channels' interleaved bytes* (fine for HTTP round-trips, wasteful for wide groups; undocumented) | n/a |
| ##DZ compression | unsupported, clean error (verified) | full support |
| Unsorted (multi-CG) files | silently wrong (A6) | supported |
| Errors in Python | `format!("{:?}")` debug dumps + `PanicException` leaks | exceptions with messages |

Verified identical: all integer/float boundary values (u8–u64, i8–i64 incl. extremes, f32/f64 incl. NaN/±inf), MDF 4.10/4.11/4.20 reads, direct VLSD string/bytearray reads, invalidation bits (`read()` path), linear/algebraic conversions via index (incl. JSON round-trip), timestamps/DatetimeIndex vs file start time, `write_columns*` output read by asammdf, cut boundaries (inclusive both ends) and cut's conversion preservation, byte-range math across DL fragments.

## F. Performance (1M records × 4 f64 + time; release build)

| Operation | mf4-rs | asammdf | ratio |
|---|---|---|---|
| Write, bulk (`write_columns_f64` / `append+save`) | **0.068 s** | 0.488 s | **7× faster** |
| Write, record-at-a-time loop (per 100k) | 0.119 s (1.19 µs/rec) | n/a | ~2.4× slower than asammdf-bulk at 1M |
| Read 4 channels, numpy (`values()` / `.samples`) | **0.021 s** | 0.11 s | **5× faster** |
| Read 4 channels, `read()` → pandas Series | **1.35 s** | 0.11 s | **13× slower** |
| Single channel from cold start (JSON index + read vs open + get) | **0.006 s** | 0.037 s | 6× faster |
| Index build / JSON load | 0.2 ms / 0.1 ms (3 kB JSON) | n/a | — |
| File size (uncompressed) | 40.0 MB | 40.0 MB (27.1 MB with ##DZ) | = |

### Why `read()` is slow, and the fix
`signal_to_series` (`python.rs:440-485`) converts **every sample to a boxed `PyObject`** and builds the Series from a Python list; separately, `pd.to_timedelta(float_seconds) + start` costs ~250 ms per 1M samples. Measured decomposition: values 8 ms + index/Series 251 ms + object boxing ≈ 90 ms ⇒ current ~340 ms/channel.
**Fix:** (1) for numeric channels, pass the same numpy f64 array `values()` uses (NaN for invalid) instead of a PyObject list; (2) compute `start_ns + (t*1e9) as i64` in Rust and hand numpy a `datetime64[ns]` array — measured at **7 ms** vs 251 ms. Together `read()` drops from ~1.35 s to ~0.05–0.1 s for 4 channels — faster than asammdf, matching `values()`.
Other wins: release the GIL in `PyMDF.read/values` (index paths already do; direct paths hold it for the full decode); batch `create_float_value` overhead away by documenting `write_columns*` as the primary write API (already 7× faster than asammdf — the CLAUDE.md perf table predates these APIs and undersells the bindings).

## G. Suggested fix priority

1. **A1/A2** — make `values()`/`signal()` apply conversions + invalidation (or rename/document loudly); align master-channel selection between direct and index paths. Cheap, user-facing wrong numbers today.
2. **A3** — guard the three unguarded index VLSD paths (error like the other two) until offset-based SD reads are implemented.
3. **A7** — turn `ChannelEncoder::Skip`/type mismatch into hard errors; expose VLSD + signed-int channel creation in the Python writer (today: silent empty strings / zeros).
4. **D1** (one line), **D2** (one line), **B1** (one line), **B3**, **B2**.
5. **A8/A9** — merge/cut: compare + preserve conversions/units/sync_type/invalidation; error on multi-CG inputs instead of corrupting.
6. **C1–C8** — bounds-check the mmap-slicing entry points (a fuzzer would find these in minutes); fix the `write_columns_f64` UB with `write_unaligned`/`copy_from_slice`; convert Rust errors with `{}` not `{:?}`.
7. **A5/A6** — either implement fragment-spanning records + record-ID demux, or detect and refuse loudly.
8. **F** — the `read()` fast path (numpy values + datetime64[ns] index, GIL release).
9. **B6** — serialize `cc_val` NaN/inf-safely (e.g. bit patterns) so range/bitfield indexes survive JSON round-trips.
10. Test gap: not a single existing test asserts a *converted value* (linear math, table boundaries, defaults) — every conversion bug above passes the current suite. Add value-level conversion tests and a malformed-file corpus.
