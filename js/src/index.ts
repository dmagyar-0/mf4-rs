/**
 * mf4-rs — ergonomic TypeScript wrapper over the mf4-rs WebAssembly bindings
 * for reading and writing ASAM MDF 4 (Measurement Data Format) files.
 *
 * Import from this module rather than `pkg-node` directly to get typed
 * signatures, Node file/URL convenience constructors, and lazy
 * `MdfIndex` reads over pluggable `RangeSource`s (HTTP, filesystem, memory).
 */

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

export { loadWasmModule } from "./wasm-module";
