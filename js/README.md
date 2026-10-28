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

### Building the index from a URL (metadata-only)

You don't need a server-side build step: `MdfIndex.fromUrl` (and the generic
`MdfIndex.fromRangeSource`) build the index by fetching **only the file's
metadata blocks** — a few KB regardless of file size — never the bulk
samples. The metadata walk runs incrementally in wasm, reporting the byte
ranges it still needs while the wrapper fetches them (with look-ahead to keep
round-trips low).

```ts
import { MdfIndex, FetchRangeSource } from "mf4-rs";

const url = "https://example.com/measurement.mf4";

// Builds the index by fetching only metadata (needs a Range-capable server;
// servers that ignore Range fall back to a single full download):
const index = await MdfIndex.fromUrl(url);

// The URL stays bound to the index, so reads need no source argument:
const speed = await index.read("VehicleSpeed");

// Or build from any RangeSource (Node file, in-memory bytes, custom):
// const index = await MdfIndex.fromRangeSource(new FileRangeSource(path));
```

This mirrors the native Rust `MdfIndex::from_url`. Prefer the ship-the-JSON
pattern above when many clients read the same file (build the index once);
use `fromUrl` when a client wants to open an arbitrary remote file directly.

### Metadata, time ranges and partial reads

```ts
index.startTimeNs();             // bigint ns since the Unix epoch, or undefined
index.groups();                  // [{ name, comment, recordCount, masterChannel,
                                 //    channels: [{ name, unit, comment, dataType,
                                 //    bitCount, isMaster, isVlsd, hasConversion }] }]
index.channel("VehicleSpeed");   // one channel's metadata

// Start/end time of a channel's group (reads just two records):
const { start, end } = (await index.timeRange("VehicleSpeed"))!;

// Read only part of a signal. Time bounds are inclusive, in master units
// (seconds since startTimeNs for a time master); the window is located by
// searching the master, so only a few single records plus the selected
// window are fetched — not the whole time channel.
const slice = await index.read("VehicleSpeed", { startTime: 10, endTime: 20 });
const head = await index.values("VehicleSpeed", { startRecord: 0, recordCount: 1000 });

// Pass `group` (and optionally `source`) in the same options object:
await index.read("Speed", { group: "Engine", startTime: 10 });
```

`Mdf.read` / `Mdf.values` accept the same window options (sliced in memory),
and `Mdf.timeRange(name)` returns a group's start/end time.

Conversions are applied by `values`/`read`; `conversionInfo(name)` shows
which one (`null` when the channel has none):

```ts
await index.conversionInfo("EngineTemp");
// { type: "Linear", values: [-40, 0.5], precision: 0, flags: 0 }
await index.conversionInfo("Gear");
// { type: "ValueToText", values: [0, 1, 2],
//   texts: { "0": "N", "1": "D", "2": "R", "3": "?" }, ... }
```

`texts` / nested `refs` are keyed by position in the block's reference list
(for `ValueToText`, `values[i]` ↔ `texts[i]`, and index `values.length` is
the default); `formula` holds an `Algebraic` expression. An index built over
the network fetches the conversion block on demand; `Mdf.conversionInfo` is
synchronous.

Timestamps stay `number` seconds **relative to the measurement start** —
that is how MDF stores them, and a float64 keeps sub-nanosecond precision
for any realistic recording length. Absolute epoch nanoseconds do not fit a
`number` (they exceed 2^53), so convert with `toAbsoluteNs` when you need
them:

```ts
import { toAbsoluteNs } from "mf4-rs";
const absNs = toAbsoluteNs(slice.timestamps, index.startTimeNs()!); // BigInt64Array
```

For the master channel itself `read()` returns empty `timestamps`: its
`values` *are* the time axis. Virtual masters (`cn_type` 3, no stored bytes)
read as the record index through their conversion.

### `signalByteRanges` vs. `byteRanges`

`signalByteRanges(name, group)` returns the **full data-block ranges** for
the group that owns the channel — one span per data-block fragment covering
the whole block, including its 24-byte header (verified before decoding). These are exactly
the bytes the fragment decoders need: `valuesFromFragments` /
`readFromFragments` decode whole data sections (every channel is interleaved
per record), so fetch `signalByteRanges` and pass the fetched fragments
straight through — the requested channel and its master are both covered for
any record layout. This is what `MdfIndex.values`/`read` fetch under the
hood. Fragments stored back to back are bridged into a single span, and for a
column-oriented (MDF 4.2 remote-master) group the shared time group's spans are
included as well — pass `withMaster = false` (third argument) to leave them
out when only the values are needed, as `MdfIndex.values` does.

`byteRanges(name, group)` is a lower-level power-user API: it returns the
spans that a *single* channel's bytes fall in. Because records interleave all
channels, those spans still include neighbouring channels' bytes and do **not**
on their own guarantee full-data-section coverage — so they are not suitable
to feed directly to the fragment decoders. Use `byteRanges` only when you are
building a custom fetch strategy and understand the record layout;
otherwise prefer `signalByteRanges`.

### Column-oriented files

`toColumnOriented` rewrites any (uncompressed) MDF 4 file as a
**column-oriented MDF 4.20** file built for exactly this access pattern: one
channel group per signal with its data blocks back to back, timestamps shared
through the MDF 4.2 remote-master link, and all metadata at the front. An index
built with `MdfIndex.fromUrl` then needs a single prefix fetch, and reading a
signal costs two range requests (values + shared time channel; one when they
are adjacent) that transfer only that signal's bytes.

```ts
import { toColumnOriented, toColumnOrientedFile, toColumnOrientedStream } from "mf4-rs";

// In memory: { bytes: Uint8Array, report }
const { bytes, report } = toColumnOriented(fileBytes, { blockSize: 4 << 20 });

// Node: stream file to file with bounded memory (sync fs I/O under the hood)
await toColumnOrientedFile("recording.mf4", "recording.col.mf4");

// Any synchronous random-access source / positional sink (e.g. an OPFS
// FileSystemSyncAccessHandle in a browser worker)
toColumnOrientedStream(
  { size, read: (offset, length) => readBytes(offset, length) },
  (offset, chunk) => writeBytes(offset, chunk),
);
```

Options (all optional): `blockSize` (default 4 MiB), `memoryBudget` (default
64 MiB), `readChunkSize` (default 4 MiB), `shareIdenticalTime` (default
`true`). The returned `ColumnReport` lists the groups written, the
`metadataSize` prefix length, and any `skippedChannels`.

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
const signal = await index.read("Temperature");

// Any RangeSource works the same way, e.g. a user-picked File:
const file = input.files![0];
const local = await MdfIndex.fromRangeSource({
  size: async () => file.size,
  read: async (offset, length) =>
    new Uint8Array(await file.slice(offset, offset + length).arrayBuffer()),
});
```

`init()` optionally accepts a URL / `Response` / `BufferSource` /
`WebAssembly.Module` to control where `mf4_rs_bg.wasm` is loaded from (handy
behind a CDN or a strict CSP); omit it to load the `.wasm` next to the
module. The `web` entry omits the Node-only helpers (`FileRangeSource`,
`toColumnOrientedFile`; `Mdf.fromFile` / `MdfIndex.fromFile` throw there) and
contains no `node:` imports, so bundlers such as webpack 5 need no polyfills or
fallbacks. Use `FetchRangeSource`, `BytesRangeSource` or your own
`RangeSource` instead. `FetchRangeSource` works in pages and workers (the
global `fetch` is always invoked with the global receiver). The `mf4-rs/web` build is
compiled as native ES modules (with explicit file extensions), so it works
both through bundlers (Vite, webpack, Next, …) and in native
`<script type="module">` / Deno.

Both entry points are covered by the test suite (`npm test` exercises the
Node classes plus a headless run of the `mf4-rs/web` build).

## API surface

- `Mdf` — in-memory reader: `fromBytes`, `fromFile` (Node), `fromUrl`,
  `groups()`, `channelNames()`, `values()`, `read()` (both take a group name
  or `ReadOptions` window), `timeRange()`, `conversionInfo()`,
  `startTimeNs()`, `dispose()`.
- `MdfIndex` — self-contained index + lazy reads: `fromBytes`,
  `fromJson(json, source?)`, `fromFile` (Node), `fromRangeSource` / `fromUrl`
  (metadata-only, over a `RangeSource` / HTTP), `source` / `setSource()`,
  `toJson()`, `validate()`, `groups()`, `channel()`, `channelNames()`,
  `startTimeNs()`, `fileSize()`, `byteRanges()`, `byteRangesForRecords()`,
  `signalByteRanges()`, `valuesFromFragments()`, `readFromFragments()`,
  `values()` / `read()` (lazy; `(name, source?, group?)` or
  `(name, ReadOptions)`), `timeRange()`, `conversionInfo()`, `dispose()`.
- `MdfWriter` — in-memory writer: `initMdfFile`, `setStartTime`,
  `addChannelGroup`, `addChannel`, `addTimeChannel`, `addFloatChannel`,
  `addFloat32Channel`, `addIntChannel`, `addSintChannel`, `addStringChannel`,
  `setTimeChannel`, `addLinearConversion`, `addValueToTextConversion`, `startDataBlock`, `writeRecord`, `finishDataBlock`,
  `finalize()`, `dispose()`. Numeric channels accept `number` or `bigint`
  record values; string channels (`addStringChannel`) accept `string`.
- Column-oriented conversion: `toColumnOriented` (in memory),
  `toColumnOrientedStream` (synchronous source/sink callbacks),
  `toColumnOrientedFile` (Node); `ColumnOptions`, `ColumnReport`.
- `RangeSource` implementations: `FetchRangeSource`, `FileRangeSource`,
  `BytesRangeSource`.
- Helpers: `toAbsoluteNs(timestamps, startTimeNs)`, `mergeByteRanges`,
  `readAllRanges`.
- Types: `Signal`, `GroupInfo`, `IndexGroupInfo`, `ChannelInfo`,
  `IndexChannelInfo`, `ConversionInfo`, `ReadOptions`, `ReadWindow`, `TimeRange`, `ByteRange`,
  `ChannelValue`.
- Entry points: `mf4-rs` (Node, synchronous) and `mf4-rs/web` (browser; adds
  `init()` and re-exports the same classes minus the Node-only
  `FileRangeSource` and `toColumnOrientedFile`).

See the inline TSDoc comments in `src/` for full parameter documentation.
