# mf4-rs (TypeScript / npm)

Ergonomic TypeScript wrapper over the `mf4-rs` WebAssembly bindings for
reading and writing ASAM MDF 4 (Measurement Data Format) files — the format
used by CANedge, Vector, ETAS, and most automotive/industrial data loggers.

This package sits on top of a generated `wasm-bindgen` module (`pkg-node/`)
and adds:

- Typed classes and interfaces instead of `any`.
- Node convenience constructors (`Mdf.fromFile`, `MdfIndex.fromFile`).
- A pluggable `RangeSource` abstraction (`FetchRangeSource`,
  `FileRangeSource`, `BytesRangeSource`) for lazy, partial reads of large or
  remote files via `MdfIndex`.

## Status

Built and tested against Node 22 on the `nodejs` wasm-pack target. The
`RangeSource` classes are plain TypeScript with no Node-only APIs except
`FileRangeSource` (which uses `fs/promises`) — see **Browser usage** below
for what that means in practice.

## Install

```bash
npm install mf4-rs
```

The published package ships the compiled TypeScript wrapper (`dist/src/` for
Node, `dist-web/` for browsers) and **both** prebuilt wasm modules — the
synchronous `nodejs` target (`pkg-node/`, used by `import "mf4-rs"`) and the
async `web` target (`pkg-web/`, used by `import "mf4-rs/web"`) — so no Rust
toolchain is needed. See [Browser usage](#browser-usage) for the `mf4-rs/web`
entry. The package version tracks the repository's release version: every
release tagged by `.github/workflows/release.yml` publishes the matching npm
version alongside the PyPI wheels and the crates.io crate.

## Building from source

For development on the bindings themselves:

### Prerequisites

- Node >= 18 (tested on Node 22).
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/) on `PATH` to build the
  wasm bindings from the Rust crate.
- The Rust crate must be compiled with the `wasm` feature (already the case
  via the `build:wasm` script below).

### Build

```bash
cd js
npm install

# Build the wasm bindings (run from js/, cds into the repo root):
npm run build:wasm       # nodejs target -> js/pkg-node/
npm run build:wasm:web   # web target    -> js/pkg-web/

# Compile the TypeScript wrapper:
npm run build            # Node (CommonJS) -> js/dist/
npm run build:web        # browser (ESM)   -> js/dist-web/

# Run the test suite
npm test
```

In a git checkout, `pkg-node/`, `pkg-web/`, `dist/`, and `dist-web/` are
build artifacts and are gitignored — run the `build:wasm*` and `build*`
scripts after cloning, before importing the package. The `prepublishOnly`
hook runs all four builds plus the tests, so a release always ships both
targets in sync.

## Node quickstart

### Read a file

```ts
import { Mdf } from "mf4-rs";

const mdf = await Mdf.fromFile("./measurement.mf4");

console.log(mdf.channelNames());
for (const group of mdf.groups()) {
  console.log(group.name, group.recordCount, group.channels.map((c) => c.name));
}

// Plain numeric array (fast path, conversions applied, invalid -> NaN)
const speed = mdf.values("VehicleSpeed");

// Values paired with the group's time axis
const signal = mdf.read("VehicleSpeed");
console.log(signal.timestamps, signal.values);

mdf.dispose();
```

### Write a file

```ts
import { MdfWriter } from "mf4-rs";
import { writeFile } from "node:fs/promises";

const writer = new MdfWriter();
writer.initMdfFile();
writer.setStartTime(BigInt(Date.now()) * 1_000_000n); // ns since epoch

const group = writer.addChannelGroup("Group1");
const time = writer.addTimeChannel(group, "Time");
writer.addFloatChannel(group, "VehicleSpeed");
writer.setTimeChannel(time);

writer.startDataBlock(group);
for (let i = 0; i < 1000; i++) {
  writer.writeRecord(group, [i * 0.01, Math.sin(i * 0.1) * 100]);
}
writer.finishDataBlock(group);

const bytes = writer.finalize();
await writeFile("./out.mf4", bytes);
writer.dispose();
```

## Remote reads with `MdfIndex`

`MdfIndex` is a self-contained, JSON-serialisable index: build it once from
an MDF file's bytes, ship the (small) JSON index to a client, and let the
client fetch only the byte ranges it needs over HTTP — no full-file
download required.

```ts
import { MdfIndex, FetchRangeSource } from "mf4-rs";

// --- Server / build step (has the full file) ---
const index = MdfIndex.fromBytes(fileBytes);
const indexJson = index.toJson(); // ship this to the client
index.dispose();

// --- Client (only has a URL and the JSON index) ---
const clientIndex = MdfIndex.fromJson(indexJson);
const source = new FetchRangeSource("https://example.com/measurement.mf4");

const speed = await clientIndex.values("VehicleSpeed", source, "Group1");
const signal = await clientIndex.read("VehicleSpeed", source, "Group1");
```

`MdfIndex.values`/`read` compute the byte ranges the requested channel
needs, fetch them from the `RangeSource` (concurrently), and decode
client-side — the wasm module never needs the whole file.

### `signalByteRanges` vs. `byteRanges`

`signalByteRanges(name, group)` returns the **full data-section ranges** for
the group that owns the channel — one span per data-block fragment covering
the entire section (block bytes minus the 24-byte header). These are exactly
the bytes the fragment decoders need: `valuesFromFragments` /
`readFromFragments` decode whole data sections (every channel is interleaved
per record), so fetch `signalByteRanges` and pass the fetched fragments
straight through — the requested channel and its master are both covered for
any record layout. This is what `MdfIndex.values`/`read` fetch under the
hood.

`byteRanges(name, group)` is a lower-level power-user API: it returns the
spans that a *single* channel's bytes fall in. Because records interleave all
channels, those spans still include neighbouring channels' bytes and do **not**
on their own guarantee full-data-section coverage — so they are not suitable
to feed directly to the fragment decoders. Use `byteRanges` only when you are
building a custom fetch strategy and understand the record layout;
otherwise prefer `signalByteRanges`.

### Custom range sources

```ts
import { BytesRangeSource, FileRangeSource } from "mf4-rs";

// Already have the bytes in memory
const fromBytes = new BytesRangeSource(fileBytes);

// Local file, Node only
const fromFile = new FileRangeSource("./measurement.mf4");
```

Implement the `RangeSource` interface (`size?()`, `read(offset, length)`) to
plug in S3, IndexedDB, or any other byte-addressable backend.

## Browser usage

The package ships **two** wasm builds. The default `.` entry (`import
"mf4-rs"`) uses the synchronous `nodejs`-target build and runs only in Node.
For browsers, import the separate `mf4-rs/web` entry, which uses the
`web`-target build (`pkg-web/`) — an ES module whose wasm is instantiated
asynchronously.

Because browser wasm instantiation cannot be synchronous, call and `await`
`init()` **once** before constructing any class; everything else is
identical to the Node API:

```ts
import { init, Mdf, MdfIndex, FetchRangeSource } from "mf4-rs/web";

// Instantiate the wasm module. Idempotent — safe to await anywhere.
await init();

// Full in-memory read:
const bytes = new Uint8Array(await (await fetch("/data.mf4")).arrayBuffer());
const mdf = Mdf.fromBytes(bytes);
console.log(mdf.channelNames());
mdf.dispose();

// Lazy, range-based remote read (only the needed bytes are fetched):
const index = await MdfIndex.fromUrl("https://example.com/data.mf4");
const signal = await index.read("Temperature", new FetchRangeSource("https://example.com/data.mf4"));
```

`init()` optionally accepts a URL / `Response` / `BufferSource` /
`WebAssembly.Module` to control where `mf4_rs_bg.wasm` is loaded from (handy
behind a CDN or a strict CSP); omit it to load the `.wasm` next to the
module. The `web` entry omits `FileRangeSource` (it needs Node's `fs`); use
`FetchRangeSource` or `BytesRangeSource` instead. The `mf4-rs/web` build is
compiled as native ES modules (with explicit file extensions), so it works
both through bundlers (Vite, webpack, Next, …) and in native
`<script type="module">` / Deno.

Both entry points are covered by the test suite (`npm test` exercises the
Node classes plus a headless run of the `mf4-rs/web` build).

## API surface

- `Mdf` — in-memory reader: `fromBytes`, `fromFile` (Node), `fromUrl`,
  `groups()`, `channelNames()`, `values()`, `read()`, `startTimeNs()`,
  `dispose()`.
- `MdfIndex` — self-contained index + lazy fragment reads: `fromBytes`,
  `fromJson`, `fromFile` (Node), `fromUrl` (downloads in full), `toJson()`, `validate()`, `groups()`,
  `channelNames()`, `fileSize()`, `byteRanges()`, `byteRangesForRecords()`,
  `signalByteRanges()`, `valuesFromFragments()`, `readFromFragments()`,
  `values()` (lazy, over a `RangeSource`), `read()` (lazy), `dispose()`.
- `MdfWriter` — in-memory writer: `initMdfFile`, `setStartTime`,
  `addChannelGroup`, `addChannel`, `addTimeChannel`, `addFloatChannel`,
  `addFloat32Channel`, `addIntChannel`, `addSintChannel`, `addStringChannel`,
  `setTimeChannel`, `startDataBlock`, `writeRecord`, `finishDataBlock`,
  `finalize()`, `dispose()`. Numeric channels accept `number` or `bigint`
  record values; string channels (`addStringChannel`) accept `string`.
- `RangeSource` implementations: `FetchRangeSource`, `FileRangeSource`,
  `BytesRangeSource`.
- Types: `Signal`, `GroupInfo`, `IndexGroupInfo`, `ChannelInfo`,
  `ByteRange`, `ChannelValue`.
- Entry points: `mf4-rs` (Node, synchronous) and `mf4-rs/web` (browser; adds
  `init()` and re-exports the same classes minus the Node-only
  `FileRangeSource`).

See the inline TSDoc comments in `src/` for full parameter documentation.
