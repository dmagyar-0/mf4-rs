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

export { Mdf } from "./mdf";
export { MdfIndex } from "./mdf-index";
export { MdfWriter, type RecordValue } from "./mdf-writer";

export {
  FetchRangeSource,
  FileRangeSource,
  BytesRangeSource,
  readAllRanges,
  type RangeSource,
  type FetchLike,
} from "./range-source";

export { mergeByteRanges } from "./byte-ranges";

export type {
  ChannelValue,
  Signal,
  ChannelInfo,
  GroupInfo,
  IndexGroupInfo,
  ByteRange,
} from "./types";

export { loadWasmModule } from "./wasm-module-node";
