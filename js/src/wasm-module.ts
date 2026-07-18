/**
 * Loader for the generated wasm-bindgen package.
 *
 * The wasm package lives at `js/pkg-node/` (built by `npm run build:wasm`,
 * i.e. `wasm-pack build --target nodejs --out-dir js/pkg-node`) and is a
 * gitignored build artifact — it does not exist until that command has been
 * run at least once.
 *
 * This module is compiled to `dist/src/wasm-module.js`, two directories
 * below `js/`, so the relative path back up to `js/pkg-node/mf4_rs.js` is
 * `../../pkg-node/mf4_rs.js`.
 */

/* eslint-disable @typescript-eslint/no-var-requires */

export interface WasmMdf {
  free(): void;
  channelNames(): string[];
  groups(): unknown;
  read(name: string, group?: string | null): unknown;
  startTimeNs(): bigint | undefined;
  values(name: string, group?: string | null): Float64Array;
}

export interface WasmMdfConstructor {
  new (data: Uint8Array): WasmMdf;
  fromBytes(data: Uint8Array): WasmMdf;
}

export interface WasmMdfIndex {
  free(): void;
  byteRanges(name: string, group?: string | null): unknown;
  byteRangesForRecords(
    name: string,
    start_record: bigint,
    record_count: bigint,
    group?: string | null,
  ): unknown;
  channelNames(): string[];
  fileSize(): number;
  groups(): unknown;
  readFromFragments(
    name: string,
    group: string | null | undefined,
    ranges: unknown,
    fragments: unknown,
  ): unknown;
  signalByteRanges(name: string, group?: string | null): unknown;
  toJson(): string;
  validate(): void;
  valuesFromFragments(
    name: string,
    group: string | null | undefined,
    ranges: unknown,
    fragments: unknown,
  ): Float64Array;
}

export interface WasmMdfIndexConstructor {
  fromBytes(data: Uint8Array): WasmMdfIndex;
  fromJson(json: string): WasmMdfIndex;
}

export interface WasmMdfWriter {
  free(): void;
  addChannel(group_id: string, name: string, data_type: string): string;
  addChannelGroup(name?: string | null): string;
  addFloat32Channel(group_id: string, name: string): string;
  addFloatChannel(group_id: string, name: string): string;
  addIntChannel(group_id: string, name: string): string;
  addSintChannel(group_id: string, name: string): string;
  addStringChannel(group_id: string, name: string): string;
  addTimeChannel(group_id: string, name: string): string;
  finalize(): Uint8Array;
  finishDataBlock(group_id: string): void;
  initMdfFile(): void;
  setStartTime(abs_time_ns: bigint): void;
  setTimeChannel(channel_id: string): void;
  startDataBlock(group_id: string): void;
  writeRecord(group_id: string, values: unknown): void;
}

export interface WasmMdfWriterConstructor {
  new (): WasmMdfWriter;
}

export interface WasmModule {
  Mdf: WasmMdfConstructor;
  MdfIndex: WasmMdfIndexConstructor;
  MdfWriter: WasmMdfWriterConstructor;
}

let cached: WasmModule | undefined;

/**
 * Load (and cache) the generated `pkg-node` wasm-bindgen module.
 *
 * Throws a friendly error if `pkg-node` hasn't been built yet.
 */
export function loadWasmModule(): WasmModule {
  if (cached) {
    return cached;
  }
  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    cached = require("../../pkg-node/mf4_rs.js") as WasmModule;
    return cached;
  } catch (err) {
    throw new Error(
      "Failed to load the mf4-rs wasm module from js/pkg-node. " +
        "Build it first with `npm run build:wasm` (requires wasm-pack). " +
        `Original error: ${(err as Error).message}`,
    );
  }
}
