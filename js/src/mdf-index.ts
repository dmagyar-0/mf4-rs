import { getWasmModule, type WasmMdfIndex } from "./wasm-module";
import {
  readAllRanges,
  FetchRangeSource,
  type RangeSource,
  type FetchLike,
} from "./range-source";
import type { ByteRange, IndexGroupInfo, Signal } from "./types";

/** Initial prefix fetched before the build loop — metadata usually starts at
 * the front of the file, so seeding this often completes the build in one or
 * two round-trips. */
const BUILD_SEED_BYTES = 256 * 1024;
/** Minimum bytes fetched per build round-trip; larger reduces round-trips at
 * the cost of possibly fetching a little unused metadata. */
const BUILD_LOOKAHEAD_BYTES = 64 * 1024;

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
    const wasm = getWasmModule();
    return new MdfIndex(wasm.MdfIndex.fromBytes(data));
  }

  /** Reload a previously serialised index from its JSON string. */
  static fromJson(json: string): MdfIndex {
    const wasm = getWasmModule();
    return new MdfIndex(wasm.MdfIndex.fromJson(json));
  }

  /** Build a fresh index by parsing an MDF file from disk. Node only. */
  static async fromFile(path: string): Promise<MdfIndex> {
    const fsp = await import("node:fs/promises");
    const buf = await fsp.readFile(path);
    return MdfIndex.fromBytes(new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength));
  }

  /**
   * Build a fresh index over any `RangeSource`, fetching **only** the file's
   * metadata blocks — never the bulk sample data.
   *
   * The metadata walk runs incrementally in wasm: it reports which byte ranges
   * it still needs, this loop fetches them (with look-ahead to keep the number
   * of requests low), and the two repeat until the index is built. For a
   * typical file this transfers a few KB regardless of the file's size. Use it
   * with `FetchRangeSource` (HTTP), `FileRangeSource` (Node), or any custom
   * source. `source.size()` must be implemented.
   *
   * Channel conversions are **not** fetched during the build — only their
   * locations are recorded. `values`/`read` fetch and resolve a channel's
   * conversion lazily on first read, so building the index never pays for
   * conversion blocks you never read.
   */
  static async fromRangeSource(source: RangeSource): Promise<MdfIndex> {
    const wasm = getWasmModule();
    if (!source.size) {
      throw new Error("MdfIndex.fromRangeSource requires a RangeSource with a size() method.");
    }
    const size = await source.size();

    const ranges: ByteRange[] = [];
    const fragments: Uint8Array[] = [];

    const fetchInto = async (offset: number, length: number): Promise<void> => {
      const clamped = Math.min(length, size - offset);
      if (offset < 0 || offset >= size || clamped <= 0) {
        throw new Error(
          `MdfIndex.fromRangeSource: build requested bytes [${offset}, ${offset + length}) ` +
            `outside the file (size ${size}); the source may be truncated or not an MDF file.`,
        );
      }
      const bytes = await source.read(offset, clamped);
      ranges.push([offset, bytes.length]);
      fragments.push(bytes);
    };

    // Seed with a prefix so files whose metadata sits at the front finish in
    // one round-trip.
    if (size > 0) {
      await fetchInto(0, Math.min(BUILD_SEED_BYTES, size));
    }

    // Bounded loop: each round-trip advances the first missing offset, so this
    // terminates; the cap is a safety net against a malformed source.
    const maxRounds = 100_000;
    for (let round = 0; round < maxRounds; round++) {
      const step = wasm.MdfIndex.buildIndexStep(size, ranges, fragments);
      if (step.done) {
        return MdfIndex.fromJson(step.json as string);
      }
      const [offset, length] = step.needed as [number, number];
      await fetchInto(offset, Math.max(length, BUILD_LOOKAHEAD_BYTES));
    }
    throw new Error(
      "MdfIndex.fromRangeSource: index build did not converge; the source may not be a valid MDF file.",
    );
  }

  /**
   * Build a fresh index from an HTTP(S) URL, fetching only metadata.
   *
   * Convenience wrapper over `fromRangeSource` with a `FetchRangeSource`, so
   * only the file's metadata blocks are downloaded (a few KB) rather than the
   * whole file — as long as the server honours `Range` requests. Servers that
   * ignore `Range` fall back to a single full download. Once built, read with
   * `values`/`read` over a `RangeSource` for lazy, partial sample reads.
   */
  static async fromUrl(url: string, fetchImpl?: FetchLike): Promise<MdfIndex> {
    return MdfIndex.fromRangeSource(new FetchRangeSource(url, fetchImpl));
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
   * (block bytes minus the 24-byte header); for a VLSD channel the spans of
   * its `##SD` fragment chain are included too. This is exactly the range set
   * the fragment readers need: `valuesFromFragments`/`readFromFragments`
   * decode whole data sections (all channels interleave per record), so fetch
   * these and pass the fetched fragments straight through — the requested
   * channel and its master are both covered for any record layout. This is
   * also what `values`/`read` fetch under the hood.
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
    // Indexes built over the network resolve conversions lazily; fetch the
    // channel's conversion blocks (if any) so the decoder can apply them.
    await this.#fetchConversionRanges(name, group, false, source, ranges, fragments);
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
    // A signal read decodes the group master too, so resolve the master's
    // conversion as well as the channel's (withMaster = true).
    await this.#fetchConversionRanges(name, group, true, source, ranges, fragments);
    return this.readFromFragments(name, group, ranges, fragments);
  }

  /**
   * Fetch the conversion-block byte ranges a lazy (network-built) index needs
   * to apply conversions, appending them to `ranges`/`fragments` in place.
   *
   * A no-op for self-contained indexes (built from bytes) and channels with no
   * conversion — `conversionRangesStep` reports `done` immediately. Otherwise
   * it drives the same gap-fetch loop as the index build, fetching each
   * reported range from `source` until every conversion block for the read is
   * present.
   */
  async #fetchConversionRanges(
    name: string,
    group: string | null | undefined,
    withMaster: boolean,
    source: RangeSource,
    ranges: ByteRange[],
    fragments: Uint8Array[],
  ): Promise<void> {
    const size = this.fileSize();
    // Conversion chains are short; a small look-ahead keeps this to one or two
    // round-trips even when a block and its text refs sit adjacently.
    const CONVERSION_LOOKAHEAD_BYTES = 8 * 1024;
    const maxRounds = 10_000;
    for (let round = 0; round < maxRounds; round++) {
      const step = this.inner.conversionRangesStep(name, group, withMaster, ranges, fragments);
      if (step.done) return;
      const [offset, length] = step.needed as [number, number];
      const want = Math.min(Math.max(length, CONVERSION_LOOKAHEAD_BYTES), size - offset);
      const bytes = await source.read(offset, want);
      ranges.push([offset, bytes.length]);
      fragments.push(bytes);
    }
    throw new Error(
      "MdfIndex: conversion resolution did not converge; the index or source may be inconsistent.",
    );
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
