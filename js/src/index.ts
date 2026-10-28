/**
 * mf4-rs — ergonomic TypeScript wrapper over the mf4-rs WebAssembly bindings
 * for reading and writing ASAM MDF 4 (Measurement Data Format) files.
 *
 * Import from this module rather than `pkg-node` directly to get typed
 * signatures, Node file/URL convenience constructors, and lazy
 * `MdfIndex` reads over pluggable `RangeSource`s (HTTP, filesystem, memory).
 *
 * This is the **Node** entry point: it uses the synchronous `pkg-node` wasm
 * build, so no `init()` step is needed. For the browser, import from
 * `mf4-rs/web` and `await init()` once before using the API.
 */

// Registers the lazy Node loader for the wasm module as an import side effect.
import "./wasm-module-node";
// Registers the fs-backed reader behind Mdf.fromFile / MdfIndex.fromFile.
import "./node-platform";

export { Mdf } from "./mdf";
export { MdfIndex, type IndexBuildOptions } from "./mdf-index";
export { MdfWriter, type RecordValue } from "./mdf-writer";
export {
  toColumnOriented,
  toColumnOrientedStream,
  type ColumnOptions,
  type ColumnReport,
  type SyncByteSource,
  type SyncByteSink,
} from "./column";
export { toColumnOrientedFile } from "./column-file";

export {
  FetchRangeSource,
  BytesRangeSource,
  readAllRanges,
  type RangeSource,
  type FetchLike,
} from "./range-source";
export { FileRangeSource } from "./file-range-source";

export { mergeByteRanges } from "./byte-ranges";
export { toAbsoluteNs } from "./signal-util";

export type {
  ChannelValue,
  Signal,
  ChannelInfo,
  GroupInfo,
  IndexGroupInfo,
  IndexChannelInfo,
  ReadOptions,
  ReadWindow,
  TimeRange,
  ByteRange,
} from "./types";

export { loadWasmModule } from "./wasm-module-node";
