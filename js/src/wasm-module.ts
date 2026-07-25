/**
 * Shared handle to the generated wasm-bindgen package.
 *
 * The wasm code is identical across targets; only the JS glue differs. Two
 * builds are shipped:
 *
 * - `js/pkg-node/` — `wasm-pack build --target nodejs` (synchronous, CommonJS),
 *   loaded lazily on Node via `wasm-module-node.ts` (the `.` entry point).
 * - `js/pkg-web/` — `wasm-pack build --target web` (async ESM), loaded in the
 *   browser via `web.ts` after `await init()` (the `mf4-rs/web` entry point).
 *
 * Both are gitignored build artifacts — they do not exist until
 * `npm run build:wasm` / `npm run build:wasm:web` has been run at least once.
 *
 * This module holds no target-specific loading logic so that bundling the web
 * entry never pulls in the Node `require` of `pkg-node`. The Node entry
 * registers a lazy loader via `registerWasmLoader`; the web entry sets the
 * resolved module directly via `setWasmModule`.
 */

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
  conversionRangesStep(
    name: string,
    group: string | null | undefined,
    with_master: boolean,
    ranges: unknown,
    fragments: unknown,
  ): WasmBuildStep;
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
  /**
   * Thin compatibility shim: re-marshals the whole `ranges`/`fragments`
   * arrays from JS on every call (`O(F)` copy per round). Prefer
   * `IndexBuilder`, which copies each fetched fragment into wasm memory
   * exactly once and keeps them across `step()` calls.
   */
  buildIndexStep(
    file_size: number,
    ranges: unknown,
    fragments: unknown,
  ): WasmBuildStep;
}

/**
 * Stateful, non-quadratic driver for an incremental range-fetched index
 * build (see `WasmMdfIndexConstructor.buildIndexStep` for the shim this
 * replaces). Fragments pushed via `push` are copied into wasm memory once and
 * kept in a sorted store across `step()` calls.
 */
export interface WasmIndexBuilder {
  free(): void;
  push(offset: number, bytes: Uint8Array): void;
  step(): WasmBuildStep;
}

export interface WasmIndexBuilderConstructor {
  new (file_size: number): WasmIndexBuilder;
}

/** One step of the incremental `buildIndexStep` index build. */
export interface WasmBuildStep {
  done: boolean;
  /** Finished index as JSON when `done` is true. */
  json?: string;
  /** `[[offset, length], ...]` of every range still needed when `done` is
   * false — the walk gathers them all in one pass so the driver can fetch them
   * as a batch. */
  needed?: [number, number][];
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
  IndexBuilder: WasmIndexBuilderConstructor;
}

let cached: WasmModule | undefined;
let loader: (() => WasmModule) | undefined;

/**
 * Provide an already-resolved wasm module (used by the browser `web.ts`
 * entry after `await init()` has instantiated it).
 */
export function setWasmModule(module: WasmModule): void {
  cached = module;
}

/**
 * Register a lazy, synchronous loader for the wasm module (used by the Node
 * `wasm-module-node.ts` entry, which `require`s the `pkg-node` build on first
 * use). Kept out of this shared module so the web bundle never references it.
 */
export function registerWasmLoader(fn: () => WasmModule): void {
  loader = fn;
}

/**
 * Return the resolved wasm module, invoking the registered lazy loader on
 * first use if one is present.
 *
 * Throws a friendly error if no module has been provided yet — on Node that
 * means `pkg-node` failed to build; in the browser it means `init()` from
 * `mf4-rs/web` has not been awaited.
 */
export function getWasmModule(): WasmModule {
  if (cached) {
    return cached;
  }
  if (loader) {
    cached = loader();
    return cached;
  }
  throw new Error(
    "mf4-rs wasm module is not initialised. On Node, import from 'mf4-rs'; " +
      "in the browser, `await init()` from 'mf4-rs/web' before using the API.",
  );
}
