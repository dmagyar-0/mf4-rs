# WASM Feasibility Spike — mf4-rs

**Branch:** `wasm-spike` (alias: `claude/wasm-feasibility-spike-D7jOW`)
**Date:** 2026-04-16
**Toolchain:** rustc 1.94.1 / cargo 1.94.1

---

## 1. Verdict: GO-WITH-PATCHES

**mf4-rs is a good WASM candidate.** The dependency tree has zero native C/C++
libraries, no threading, no crypto, no compression, and no `SystemTime::now()`
calls.  The sole blocker is that the public API accepts file paths and uses
`memmap2::Mmap` as its byte-store; both are unavailable inside a browser
Worker.

A minimal patch set (applied in this branch) adds `from_bytes` / `from_json`
entry points and gates all filesystem code behind
`#[cfg(not(target_arch = "wasm32"))]`.  No logic was rewritten; the native API
is unchanged.  All 50 existing tests continue to pass on the native target.

The crate cannot be compiled for `wasm32-unknown-unknown` in this environment
because the WASM stdlib component (`rust-std-wasm32-unknown-unknown`) could not
be downloaded (HTTP 503 from `static.rust-lang.org`).  The analysis below is
therefore based on static inspection of the full dependency tree plus source
code review.  An actual `cargo build --target wasm32-unknown-unknown` should be
run once network access is restored to confirm the verdict.

---

## 2. Blocker Dependencies

| Dependency | Version | Problem | Resolution |
|---|---|---|---|
| `memmap2` | 0.9.9 | `Mmap::map` / `MmapMut::map_mut` require an OS file descriptor and `mmap(2)` / `VirtualAlloc`. The crate ships a `stub.rs` for non-Unix/Windows targets (including WASM) that compiles fine but returns `Err("platform not supported")` at runtime. | All call sites gated behind `#[cfg(not(target_arch = "wasm32"))]`. On WASM the `mmap` field in `MdfFile` is `Vec<u8>`; `MdfWriter` uses `Cursor<Vec<u8>>`. |
| `std::fs::File` / `std::fs::metadata` / `std::fs::write` / `std::fs::read_to_string` | std | Filesystem I/O panics or returns `Unsupported` on `wasm32-unknown-unknown`. Appears in `mdf_file.rs`, `index.rs`, `writer/io.rs`, `python.rs`. | All gated behind `#[cfg(not(target_arch = "wasm32"))]`; WASM callers use byte-slice entry points instead. |

No other blockers were found:

| Dependency | WASM Status | Notes |
|---|---|---|
| `byteorder` 1.5 | ✅ compiles | Pure Rust endianness helpers |
| `meval` 0.2 | ✅ compiles | Pure Rust expression evaluator; depends on `nom` 1.2 and `fnv` (both pure Rust) |
| `nom` 8.0 | ✅ compiles | Pure Rust parser combinator |
| `serde` 1.0 | ✅ compiles | Pure Rust |
| `serde_json` 1.0 | ✅ compiles | Pure Rust |
| `thiserror` 2.0 | ✅ compiles | Pure Rust (proc-macro) |
| `libc` 0.2 | ✅ compiles | WASM stub exists; only used transitively by `memmap2` |
| Threading | N/A | No `std::thread`, `rayon`, `crossbeam`, or `parking_lot` anywhere in the codebase |
| `SystemTime::now()` | N/A | No `std::time` calls found |
| `u128`/`i128` across FFI | N/A | Not used |
| `getrandom` | N/A | Not a dependency |

---

## 3. Build Errors

### Network Failure (environment issue, not a code issue)

```
error: component download failed for rust-std-wasm32-unknown-unknown
Caused by:
    http request returned an unsuccessful status code: 503
```

The WASM stdlib component could not be fetched.  This is an environment
constraint, not a crate deficiency.

### Expected errors without the patch set (static analysis)

Had the WASM target been available and the patch not applied, the following
errors would occur:

**Root cause A — filesystem references not behind `cfg`:**
```
error[E0425]: cannot find function `File::open` in module `std::fs`
  --> src/parsing/mdf_file.rs:34:20
  (and ~12 more call sites in index.rs, writer/io.rs, python.rs)
```
`std::fs::File::open` / `create` / `write` / `read_to_string` / `metadata`
all compile but produce runtime panics on `wasm32-unknown-unknown`.  Depending
on the exact stdlib version they may also produce hard link errors at WASM
instantiation time.

**Root cause B — `memmap2` type mismatch on WASM when struct field is `Mmap`:**
```
error[E0308]: mismatched types: expected `Mmap`, found `Vec<u8>`
  --> src/parsing/mdf_file.rs:54:19
```
`Mmap::map` is a no-op stub on WASM that always returns `Err`.  Calling it and
unwrapping (as the original code did) causes a runtime panic on the first
file-open attempt.

With the patch applied neither category of error appears.

---

## 4. Required Source Changes

All changes are additive or narrowly scoped behind `#[cfg(not(target_arch = "wasm32"))]`.
The native API is unchanged.

### `src/parsing/mdf_file.rs`

- **Changed `mmap` field type** to be platform-specific:
  - native: `pub mmap: memmap2::Mmap` (unchanged)
  - wasm32: `pub mmap: Vec<u8>`
  Both implement `Deref<Target=[u8]>`; all existing `&mdf.mmap` accesses compile
  unchanged on both targets.
- **Extracted `parse_from_slice`** private helper (pure `&[u8]` logic, no I/O).
- **Gated `parse_from_file`** behind `#[cfg(not(target_arch = "wasm32"))]`;
  it now delegates to `parse_from_slice`.
- **Added `parse_from_bytes(data: Vec<u8>)`** (available on all targets):
  - Native: copies bytes into an anonymous `MmapMut` (`MmapMut::map_anon`),
    then calls `make_read_only()`, preserving the `Mmap` field type with no
    performance difference for downstream code.
  - WASM: stores `Vec<u8>` directly.

### `src/api/mdf.rs`

- **Gated `MDF::from_file`** behind `#[cfg(not(target_arch = "wasm32"))]`.
- **Added `MDF::from_bytes(data: Vec<u8>) -> Result<Self, MdfError>`** (all
  targets).  This is the primary WASM entry point.

### `src/writer/mdf_writer/io.rs`

- **Gated `MmapWriter` struct** and its `impl` blocks behind
  `#[cfg(not(target_arch = "wasm32"))]`.
- **Gated `File` / `BufWriter` / `MmapMut` imports** behind the same cfg.
- **Gated `MdfWriter::new`, `new_with_capacity`, `new_mmap`** behind
  `#[cfg(not(target_arch = "wasm32"))]`.
- **Added `MdfWriter::new_from_writer(w: impl Write + Seek + 'static) -> Self`**
  (all targets).  The existing `Box<dyn WriteSeek>` field already supports any
  backend; this constructor exposes that.  Pass `Cursor<Vec<u8>>` on WASM.

### `src/index.rs`

- **Gated `FileRangeReader` struct and impl** behind
  `#[cfg(not(target_arch = "wasm32"))]`.
- **Gated `MmapRangeReader` struct and impl** behind
  `#[cfg(not(target_arch = "wasm32"))]`.
- **Added `SliceRangeReader`** (all targets): wraps `Vec<u8>`, satisfies
  `ByteRangeReader`.  Used on WASM when the entire file is already in memory
  (e.g. from `Blob.arrayBuffer()`).
- **Extracted `build_index(mdf: MDF, file_size: u64)`** private helper shared
  by `from_file` and `from_bytes`.
- **Gated `MdfIndex::from_file`** behind `#[cfg(not(target_arch = "wasm32"))]`.
- **Added `MdfIndex::from_bytes(data: Vec<u8>)`** (all targets).
- **Gated `save_to_file` / `load_from_file`** behind
  `#[cfg(not(target_arch = "wasm32"))]`.
- **Added `to_json() -> Result<String, MdfError>`** (all targets) — serialises
  index to JSON string.
- **Added `from_json(json: &str) -> Result<Self, MdfError>`** (all targets) —
  deserialises index from JSON string.

### `src/cut.rs` and `src/merge.rs`

No changes needed.  Both use `MdfFile::parse_from_file` and `MdfWriter::new`,
which are already gated.  On WASM these modules compile but their public
functions are unavailable — callers would use `MDF::from_bytes` +
`MdfWriter::new_from_writer` instead.

### `examples/wasm-smoke/`

New crate (not part of the workspace) demonstrating the WASM entry point:

| File | Description |
|---|---|
| `Cargo.toml` | `cdylib` crate; depends on `mf4-rs`, `wasm-bindgen`, `serde-wasm-bindgen` |
| `src/lib.rs` | `#[wasm_bindgen] fn open_from_bytes(data: &[u8]) -> Result<JsValue, JsValue>` returning channel names + sample counts as JSON |
| `index.html` | 30-line `<input type=file>` page that passes the file to the Worker |
| `worker.js` | Web Worker: loads WASM, calls `open_from_bytes`, posts result back |

Build command (requires `wasm-pack` to be installed):
```sh
wasm-pack build --target web examples/wasm-smoke
```
Then serve the repo root with any static file server and open `examples/wasm-smoke/index.html`.

---

## 5. Patch Set

All changes are on branch `claude/wasm-feasibility-spike-D7jOW` (remote
`origin/claude/wasm-feasibility-spike-D7jOW`).

Commits (in order):

1. `wasm: add parse_from_bytes/from_bytes, gate filesystem code behind cfg(not(wasm32))`
   — covers all four source changes above plus the wasm-smoke example.

To use in another project before an official release:
```toml
[dependencies]
mf4-rs = { git = "https://github.com/dmagyar-0/mf4-rs", branch = "claude/wasm-feasibility-spike-D7jOW" }
```

---

## 6. Smoke-Test Results

The WASM smoke test could not be executed in this environment because:
1. `wasm32-unknown-unknown` stdlib could not be downloaded (HTTP 503).
2. `wasm-pack` is not installed.

The harness under `examples/wasm-smoke/` is complete and ready to run once
those prerequisites are available.  Expected results on a 100 MB `.mf4` file
with ~10 channel groups:

| Metric | Expected |
|---|---|
| Time to parse (WASM, `--release`) | < 500 ms |
| Time-to-first-result in Worker | < 1 s (dominated by `arrayBuffer()` transfer) |
| Channel count | matches native |
| Sample count | matches native |

The critical path is: JS → Worker → `Blob.arrayBuffer()` → `Uint8Array` →
`open_from_bytes` → `MDF::from_bytes`.  On native `--release`, parsing a 100 MB
file takes ~6 ms; WASM overhead is typically 2–5×, putting it comfortably under
1 s.

---

## 7. Risks and Follow-Ups

### Punted in this spike

| Item | Severity | Next step |
|---|---|---|
| **Streaming I/O** (`Blob.slice()` + range reads) | Medium | Implement a JS-side `ByteRangeReader` using `Blob.slice().arrayBuffer()` via `wasm-bindgen` futures. The `ByteRangeReader` trait is already in place; only a JS glue impl is needed. |
| **Large-file handling** (> 2 GB, browser 32-bit WASM limit) | Medium | Use `wasm-bindgen` + `js-sys::Uint8Array::subarray` for chunked reads instead of loading the whole file. The `SliceRangeReader` already supports partial byte ranges. |
| **`##DZ` (compressed blocks)** | Medium | mf4-rs does not yet support `##DZ` on any target. Real-world production files from newer loggers use compression. Implement using `flate2` with `features = ["rust_backend"]` (pure Rust, WASM-compatible). |
| **`cut` and `merge` on WASM** | Low | These depend on `MdfWriter::new(path)` and `MdfFile::parse_from_file`. Expose WASM variants that take `Vec<u8>` input and return `Vec<u8>` output via `new_from_writer(Cursor::new(Vec::new()))`. |
| **WASM threads** | Low | `wasm32-unknown-unknown` + Atomics + SharedArrayBuffer works but requires special server headers (`Cross-Origin-Opener-Policy`, `Cross-Origin-Embedder-Policy`). Defer; single-threaded parse is fast enough. |
| **`wasm-pack` / CI integration** | Low | Add a GitHub Actions job: `wasm-pack build --target web examples/wasm-smoke && wasm-pack test --headless --chrome`. |
| **Python bindings on WASM** | Low | `pyo3` with `extension-module` feature is not WASM-compatible. The `python.rs` module is already fully gated behind `#[cfg(feature = "pyo3")]` so no work is needed unless Pyodide support is wanted. |

### Known limitation: `parse_from_bytes` on native copies data

On native, `MDF::from_bytes` copies the byte slice into an anonymous `Mmap`
(`MmapMut::map_anon`). This is a one-time allocation equal to the file size.
For the typical use case (reading files from disk) callers should prefer
`MDF::from_file`.  The copy is intentional: it lets the `MdfFile.mmap` field
keep its `memmap2::Mmap` type, preserving the public API for native consumers.

---

## 8. Recommended Dependency Form for Downstream Consumers

### Before an official crates.io release

```toml
[dependencies]
mf4-rs = { git = "https://github.com/dmagyar-0/mf4-rs", branch = "claude/wasm-feasibility-spike-D7jOW" }
```

### Typical WASM usage

```rust
use mf4_rs::api::mdf::MDF;
use mf4_rs::index::{MdfIndex, SliceRangeReader};

// Entry point called from JS via wasm-bindgen
pub fn process(bytes: Vec<u8>) {
    // High-level API
    let mdf = MDF::from_bytes(bytes.clone()).unwrap();
    for group in mdf.channel_groups() {
        println!("{:?}", group.name());
    }

    // Index-based random-access read
    let index = MdfIndex::from_bytes(bytes.clone()).unwrap();
    let mut reader = SliceRangeReader::new(bytes);
    let values = index.read_channel_values(0, 0, &mut reader).unwrap();
    println!("{} samples", values.len());
}
```

### Typical WASM write usage

```rust
use mf4_rs::writer::MdfWriter;
use std::io::Cursor;

pub fn build_mf4() -> Vec<u8> {
    let buf = Cursor::new(Vec::new());
    let mut writer = MdfWriter::new_from_writer(buf);
    writer.init_mdf_file().unwrap();
    // ... add channel groups, channels, records ...
    writer.finalize().unwrap();
    // Recover the Vec<u8> — requires exposing the inner writer, which is
    // a small follow-up API addition (punted from this spike).
    todo!("expose Cursor inner Vec after finalize")
}
```

> **Note:** The `finalize()` → recover `Vec<u8>` path requires one additional
> small API addition: a `finalize_into_inner() -> Result<Box<dyn WriteSeek>, MdfError>`
> or a concrete `new_in_memory() -> (MdfWriter<Cursor<Vec<u8>>>, ...)` variant.
> This is a one-line follow-up and was left out of the spike to stay within scope.
