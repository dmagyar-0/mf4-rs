import { loadWasmModule, type WasmMdfIndex } from "./wasm-module";
import { readAllRanges, type RangeSource } from "./range-source";
import type { ByteRange, IndexGroupInfo, Signal } from "./types";

/**
 * A self-contained, JSON-serialisable index over an MDF 4 file.
 *
 * Build it from bytes (or reload from JSON), inspect metadata by name,
 * compute byte ranges for partial fetches, then decode values from
 * caller-fetched fragments over any `RangeSource` — the model for reading
 * large or remote MDF files in the browser or over HTTP.
 */
export class MdfIndex {
  private readonly inner: WasmMdfIndex;

  private constructor(inner: WasmMdfIndex) {
    this.inner = inner;
  }

  /**
   * Build a fresh index by parsing an MDF file from an in-memory buffer.
   *
   * All conversions are resolved during construction, so the index is fully
   * self-contained afterwards (and can be serialised with `toJson`).
   */
  static fromBytes(data: Uint8Array): MdfIndex {
    const wasm = loadWasmModule();
    return new MdfIndex(wasm.MdfIndex.fromBytes(data));
  }

  /** Reload a previously serialised index from its JSON string. */
  static fromJson(json: string): MdfIndex {
    const wasm = loadWasmModule();
    return new MdfIndex(wasm.MdfIndex.fromJson(json));
  }

  /** Build a fresh index by parsing an MDF file from disk. Node only. */
  static async fromFile(path: string): Promise<MdfIndex> {
    const fsp = await import("node:fs/promises");
    const buf = await fsp.readFile(path);
    return MdfIndex.fromBytes(new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength));
  }

  /** Serialise the index to a JSON string. */
  toJson(): string {
    return this.inner.toJson();
  }

  /** Validate internal consistency of the index (block layout vs file size). */
  validate(): void {
    this.inner.validate();
  }

  /** Metadata for every channel group. */
  groups(): IndexGroupInfo[] {
    return this.inner.groups() as IndexGroupInfo[];
  }

  /** Names of every named channel across all groups (duplicates kept). */
  channelNames(): string[] {
    return this.inner.channelNames();
  }

  /** Total size of the source MDF file (bytes) when the index was built. */
  fileSize(): number {
    return this.inner.fileSize();
  }

  /**
   * Byte ranges `[[offset, length], ...]` occupied by a single channel.
   *
   * Power-user API: each span coalesces one data-block fragment and, because
   * records interleave all channels, includes neighbouring channels' bytes.
   * It does **not** on its own guarantee full-data-section coverage, so it is
   * not suitable to feed directly to the fragment decoders — prefer
   * `signalByteRanges` for reads.
   */
  byteRanges(name: string, group?: string | null): ByteRange[] {
    return this.inner.byteRanges(name, group) as ByteRange[];
  }

  /**
   * Byte ranges covering a record window `[startRecord, startRecord+recordCount)`.
   *
   * `startRecord` and `recordCount` are plain integers (non-negative).
   */
  byteRangesForRecords(
    name: string,
    startRecord: number,
    recordCount: number,
    group?: string | null,
  ): ByteRange[] {
    if (!Number.isInteger(startRecord) || startRecord < 0) {
      throw new RangeError(`startRecord must be a non-negative integer, got ${startRecord}`);
    }
    if (!Number.isInteger(recordCount) || recordCount < 0) {
      throw new RangeError(`recordCount must be a non-negative integer, got ${recordCount}`);
    }
    return this.inner.byteRangesForRecords(
      name,
      BigInt(startRecord),
      BigInt(recordCount),
      group,
    ) as ByteRange[];
  }

  /**
   * Full data-section byte ranges for the group owning `name`, merged.
   *
   * Returns one span per data-block fragment covering the entire data section
   * (block bytes minus the 24-byte header). This is exactly the range set the
   * fragment readers need: `valuesFromFragments`/`readFromFragments` decode
   * whole data sections (all channels interleave per record), so fetch these
   * and pass the fetched fragments straight through — the requested channel
   * and its master are both covered for any record layout. This is also what
   * `values`/`read` fetch under the hood.
   */
  signalByteRanges(name: string, group?: string | null): ByteRange[] {
    return this.inner.signalByteRanges(name, group) as ByteRange[];
  }

  /**
   * Decode a numeric channel to a `Float64Array` from caller-fetched
   * fragments (invalid/non-numeric samples become `NaN`).
   *
   * `ranges`/`fragments` must be a matching pair previously obtained from
   * `byteRanges`/`signalByteRanges` and fetched bytes for each span.
   */
  valuesFromFragments(
    name: string,
    group: string | null | undefined,
    ranges: ByteRange[],
    fragments: Uint8Array[],
  ): Float64Array {
    return this.inner.valuesFromFragments(name, group, ranges, fragments);
  }

  /**
   * Decode a channel to a `Signal` from caller-fetched fragments.
   *
   * `ranges`/`fragments` must be a matching pair previously obtained from
   * `signalByteRanges` and fetched bytes for each span.
   */
  readFromFragments(
    name: string,
    group: string | null | undefined,
    ranges: ByteRange[],
    fragments: Uint8Array[],
  ): Signal {
    return this.inner.readFromFragments(name, group, ranges, fragments) as Signal;
  }

  /**
   * Lazily decode a numeric channel to a `Float64Array`, fetching only the
   * bytes it needs from `source`.
   *
   * Fetches the group's full data-section ranges (`signalByteRanges`) from
   * `source` concurrently, then decodes via `valuesFromFragments`. The
   * fragment decoders read whole data sections (records interleave every
   * channel), so `signalByteRanges` is exactly the right range set for any
   * group shape.
   */
  async values(name: string, source: RangeSource, group?: string | null): Promise<Float64Array> {
    const ranges = this.signalByteRanges(name, group);
    const fragments = await readAllRanges(source, ranges);
    return this.valuesFromFragments(name, group, ranges, fragments);
  }

  /**
   * Lazily decode a channel to a `Signal`, fetching only the bytes it needs
   * from `source`.
   *
   * Fetches the group's full data-section ranges (`signalByteRanges`) from
   * `source` concurrently, then decodes via `readFromFragments`.
   */
  async read(name: string, source: RangeSource, group?: string | null): Promise<Signal> {
    const ranges = this.signalByteRanges(name, group);
    const fragments = await readAllRanges(source, ranges);
    return this.readFromFragments(name, group, ranges, fragments);
  }

  /** Free the underlying wasm memory. Safe to call multiple times. */
  dispose(): void {
    this.inner.free();
  }

  /** `using`/`Symbol.dispose` support: frees the underlying wasm memory. */
  [Symbol.dispose](): void {
    this.dispose();
  }
}
