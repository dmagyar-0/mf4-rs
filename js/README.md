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

## Prerequisites

- Node >= 18 (tested on Node 22).
- [`wasm-pack`](https://rustwasm.github.io/wasm-pack/) on `PATH` to build the
  wasm bindings from the Rust crate.
- The Rust crate must be compiled with the `wasm` feature (already the case
  via the `build:wasm` script below).

## Install / build

```bash
cd js
npm install

# Build the wasm bindings into js/pkg-node/ (run from js/, cds into the repo root)
npm run build:wasm

# Compile the TypeScript wrapper into js/dist/
npm run build

# Run the test suite
npm test
```

`pkg-node/` and `dist/` are build artifacts and are gitignored — run
`build:wasm` and `build` after cloning, before importing the package.

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

### Note on `signalByteRanges` vs. what the convenience methods fetch

`signalByteRanges(name, group)` returns the merged byte ranges for a
channel **and its group's master** — the ranges you'd want for a plain
two-channel (master + data) group. In testing against groups with additional
channels declared after the requested one, that merged range does not
always cover the tail bytes of later records for those trailing channels,
and `valuesFromFragments`/`readFromFragments` require the *entire* data
section to be present in the supplied fragments (they decode whole blocks,
not just the requested column). To stay correct for arbitrary group shapes,
`MdfIndex.values`/`read` fetch the union of `byteRanges` for **every**
channel in the owning group, not just `signalByteRanges`. For a plain
two-channel group this union is identical to `signalByteRanges`, so there is
no extra cost in the common case; for wider groups it means fetching that
group's full data section. `byteRanges` and `signalByteRanges` are still
exposed directly as power-user APIs if you want to build a different fetch
strategy (e.g. you know your group is master+single-channel and want the
smallest possible request).

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

This package's `pkg-node/` wasm build is generated with
`wasm-pack build --target nodejs`, which produces a CommonJS module that
loads the `.wasm` file via `fs.readFileSync` — it only runs in Node.

For browsers, build a separate `web` target instead:

```bash
wasm-pack build --target web --out-dir js/pkg-web -- --features wasm
```

That produces an ES module with an `init()` function that `fetch`es the
`.wasm` binary, suitable for bundlers or `<script type="module">`. The
TypeScript wrapper in `src/` is written against the same `Mdf` / `MdfIndex`
/ `MdfWriter` shape either build exposes, and the fetch-based `RangeSource`
(`FetchRangeSource`) has no Node-specific dependencies, so the wrapper
classes work unmodified in a browser once pointed at a `web`-target build —
**only `wasm-module.ts`'s `require("../../pkg-node/mf4_rs.js")` load path
would need to change** to the `web`-target's `init()` call for a bundler
build. This browser path is documented but not covered by this package's
test suite, which only exercises the `nodejs` target — verify it yourself
before relying on it in production.

## API surface

- `Mdf` — in-memory reader: `fromBytes`, `fromFile` (Node), `fromUrl`,
  `groups()`, `channelNames()`, `values()`, `read()`, `startTimeNs()`,
  `dispose()`.
- `MdfIndex` — self-contained index + lazy fragment reads: `fromBytes`,
  `fromJson`, `fromFile` (Node), `toJson()`, `validate()`, `groups()`,
  `channelNames()`, `fileSize()`, `byteRanges()`, `byteRangesForRecords()`,
  `signalByteRanges()`, `valuesFromFragments()`, `readFromFragments()`,
  `values()` (lazy, over a `RangeSource`), `read()` (lazy), `dispose()`.
- `MdfWriter` — in-memory writer: `initMdfFile`, `setStartTime`,
  `addChannelGroup`, `addChannel`, `addTimeChannel`, `addFloatChannel`,
  `addFloat32Channel`, `addIntChannel`, `addSintChannel`, `setTimeChannel`,
  `startDataBlock`, `writeRecord`, `finishDataBlock`, `finalize()`,
  `dispose()`.
- `RangeSource` implementations: `FetchRangeSource`, `FileRangeSource`,
  `BytesRangeSource`.
- Types: `Signal`, `GroupInfo`, `IndexGroupInfo`, `ChannelInfo`,
  `ByteRange`, `ChannelValue`.

See the inline TSDoc comments in `src/` for full parameter documentation.
