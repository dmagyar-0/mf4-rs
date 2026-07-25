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
/** Base bytes fetched per build round-trip, and the window the read-ahead
 * heuristic falls back to whenever the walk jumps to a new metadata cluster
 * (see {@link nextLookahead}). Large enough to swallow a typical cluster in one
 * request. */
const BUILD_LOOKAHEAD_BYTES = 64 * 1024;
/** Cap on the per-round fetch window, reached only while the walk is streaming
 * sequentially through one large metadata cluster. */
const BUILD_MAX_LOOKAHEAD_BYTES = 1024 * 1024;
/** Distance between two needed ranges that still gets bridged into a single
 * request. Constant, and deliberately much smaller than the look-ahead cap:
 * bridging pulls **every** intervening byte, so a large bridge on a file whose
 * metadata is interleaved with data blocks would transfer the data blocks too. */
const BUILD_BRIDGE_BYTES = 64 * 1024;

/**
 * Tuning for the incremental index build, for callers who know their file's
 * shape. All sizes are in bytes; omitted fields keep the defaults.
 *
 * The defaults suit files whose metadata sits contiguously at the front. They
 * are wrong in opposite directions for two other shapes, which is why they are
 * exposed:
 *
 * - **Metadata interleaved with data blocks** (a per-message bus logger, or any
 *   writer that emits each group's data before the next group's metadata):
 *   look-ahead spans straddle the data blocks between metadata clusters and
 *   transfer them. Lowering `maxLookaheadBytes` toward a single cluster's size
 *   trades more round-trips for far fewer bytes.
 * - **Very large metadata** (thousands of channels in one group): raising
 *   `seedBytes` and `maxLookaheadBytes` converges in fewer round-trips.
 *
 * There is no universally right answer — it is a bandwidth-versus-latency
 * trade, so measure against your own files.
 */
export interface IndexBuildOptions {
  /** Prefix fetched before the first pass. Default 256 KiB. */
  seedBytes?: number;
  /** Base per-round fetch window, and the size the window resets to whenever
   * the walk jumps to a new metadata cluster. Default 64 KiB. */
  lookaheadBytes?: number;
  /** Ceiling on the per-round fetch window, reached only while the walk streams
   * sequentially through one large cluster. Default 1 MiB. */
  maxLookaheadBytes?: number;
  /** Two needed ranges closer than this are fetched as one request, including
   * the bytes between them. Default 64 KiB. */
  bridgeBytes?: number;
}

interface ResolvedBuildOptions {
  seedBytes: number;
  lookaheadBytes: number;
  maxLookaheadBytes: number;
  bridgeBytes: number;
}

function positiveSize(value: number | undefined, fallback: number, what: string): number {
  if (value === undefined) return fallback;
  if (!Number.isInteger(value) || value <= 0) {
    throw new Error(`MdfIndex build option ${what} must be a positive integer, got ${value}`);
  }
  return value;
}

function resolveBuildOptions(options?: IndexBuildOptions): ResolvedBuildOptions {
  const lookaheadBytes = positiveSize(
    options?.lookaheadBytes,
    BUILD_LOOKAHEAD_BYTES,
    "lookaheadBytes",
  );
  const maxLookaheadBytes = positiveSize(
    options?.maxLookaheadBytes,
    BUILD_MAX_LOOKAHEAD_BYTES,
    "maxLookaheadBytes",
  );
  return {
    seedBytes: positiveSize(options?.seedBytes, BUILD_SEED_BYTES, "seedBytes"),
    lookaheadBytes,
    // A base above the ceiling would silently ignore the ceiling.
    maxLookaheadBytes: Math.max(lookaheadBytes, maxLookaheadBytes),
    bridgeBytes: positiveSize(options?.bridgeBytes, BUILD_BRIDGE_BYTES, "bridgeBytes"),
  };
}

/** Smallest offset in a batch of needed ranges, without spreading a
 * potentially huge array into `Math.min`. */
function minOffset(needed: [number, number][]): number {
  let min = Infinity;
  for (const [offset] of needed) {
    if (offset < min) min = offset;
  }
  return min;
}

/**
 * Next fetch window, using a read-ahead heuristic rather than the round number.
 *
 * Growing purely with the round count is wrong on a file whose metadata is
 * interleaved with data blocks: the window ratchets up to its ceiling within a
 * few rounds and then stays there for every remaining round, so each small
 * metadata cluster is fetched with a multi-megabyte request that drags in the
 * data blocks around it.
 *
 * Instead, grow only while the walk is reading *sequentially* — the next thing
 * it needs continues on from what was just fetched, i.e. we are streaming
 * through one large metadata cluster and want bigger reads. When the walk
 * instead jumps somewhere far away (the next channel group's metadata, past a
 * data block), reset to the base window: the new cluster is probably small, and
 * a large window here is pure over-fetch.
 */
function nextLookahead(
  needed: [number, number][],
  previousFetchEnd: number,
  current: number,
  opts: ResolvedBuildOptions,
): number {
  const sequential = minOffset(needed) <= previousFetchEnd + opts.bridgeBytes;
  if (!sequential) return opts.lookaheadBytes;
  return Math.min(current * 2, opts.maxLookaheadBytes);
}

/**
 * Coalesce the ranges a build/conversion pass reported into a few fetch spans.
 *
 * The ranges arrive sorted and non-overlapping from wasm; here we bridge spans
 * separated by a gap no larger than `bridge` (grabbing the intervening bytes in
 * one request) and extend each span by `lookahead` so the next structural block
 * is usually already covered. Bridging turns the many tiny name/unit gaps of a
 * scattered-metadata file into a few large reads, while genuinely far-apart
 * gaps (e.g. data-block headers megabytes apart) stay separate — keeping the
 * transferred bytes small for the common contiguous-metadata case.
 */
function coalesceNeeded(
  needed: [number, number][],
  bridge: number,
  lookahead: number,
  size: number,
): [number, number][] {
  const sorted = [...needed].sort((a, b) => a[0] - b[0]);
  const spans: [number, number][] = [];
  for (const [offset, length] of sorted) {
    const end = offset + length;
    const last = spans[spans.length - 1];
    if (last && offset <= last[0] + last[1] + bridge) {
      last[1] = Math.max(last[1], end - last[0]);
    } else {
      spans.push([offset, length]);
    }
  }
  return spans.map(([offset, length]): [number, number] => [
    offset,
    Math.min(Math.max(length, lookahead), size - offset),
  ]);
}

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
   * The metadata walk runs incrementally in wasm, driven by a stateful
   * `IndexBuilder`: each pass reports the byte ranges it still needs — a
   * *batch* covering every independent gap discovered so far (a scattered
   * file's metadata across many channel groups converges in a handful of
   * passes, not one per group) — this loop fetches all of them **concurrently**
   * and pushes the results into the builder, and the two repeat until the
   * index is built. Fetched bytes are copied into wasm memory once, via
   * `push`, and stay there across passes (no re-marshalling every round). For
   * a typical file this transfers a few KB regardless of the file's size. Use
   * it with `FetchRangeSource` (HTTP), `FileRangeSource` (Node), or any custom
   * source. `source.size()` must be implemented.
   *
   * Channel conversions are **not** fetched during the build — only their
   * locations are recorded. `values`/`read` fetch and resolve a channel's
   * conversion lazily on first read, so building the index never pays for
   * conversion blocks you never read.
   *
   * How much gets transferred depends on how the file interleaves metadata with
   * data, and the fetch-window defaults can be overridden per call — see
   * {@link IndexBuildOptions}. On a file whose metadata is interleaved with
   * data blocks, lowering `maxLookaheadBytes` toward one metadata cluster's
   * size trades extra round-trips for a large reduction in bytes.
   */
  static async fromRangeSource(
    source: RangeSource,
    options?: IndexBuildOptions,
  ): Promise<MdfIndex> {
    const wasm = getWasmModule();
    if (!source.size) {
      throw new Error("MdfIndex.fromRangeSource requires a RangeSource with a size() method.");
    }
    const opts = resolveBuildOptions(options);
    const size = await source.size();

    const builder = new wasm.IndexBuilder(size);
    try {
      const fetchInto = async (offset: number, length: number): Promise<void> => {
        const clamped = Math.min(length, size - offset);
        if (offset < 0 || offset >= size || clamped <= 0) {
          throw new Error(
            `MdfIndex.fromRangeSource: build requested bytes [${offset}, ${offset + length}) ` +
              `outside the file (size ${size}); the source may be truncated or not an MDF file.`,
          );
        }
        const bytes = await source.read(offset, clamped);
        builder.push(offset, bytes);
      };

      // Seed with a prefix so files whose metadata sits at the front finish in
      // one round-trip.
      let previousFetchEnd = 0;
      if (size > 0) {
        const seed = Math.min(opts.seedBytes, size);
        await fetchInto(0, seed);
        previousFetchEnd = seed;
      }
      let lookahead = opts.lookaheadBytes;

      // Bounded loop: each round fetches every range the walk still needs
      // (concurrently, with a geometrically growing look-ahead), so a file
      // with N metadata blocks converges in a handful of rounds rather than
      // one per block. The cap is a safety net against a malformed source.
      const maxRounds = 10_000;
      for (let round = 0; round < maxRounds; round++) {
        const step = builder.step();
        if (step.done) {
          return MdfIndex.fromJson(step.json as string);
        }
        // Bridge and look-ahead are separate knobs: bridging pulls every
        // intervening byte, so it stays small and constant, while the
        // per-span look-ahead grows only while the walk reads sequentially.
        const needed = step.needed as [number, number][];
        lookahead = nextLookahead(needed, previousFetchEnd, lookahead, opts);
        const spans = coalesceNeeded(needed, opts.bridgeBytes, lookahead, size);
        for (const [offset, length] of spans) {
          previousFetchEnd = Math.max(previousFetchEnd, offset + length);
        }
        await Promise.all(spans.map(([offset, length]) => fetchInto(offset, length)));
      }
      throw new Error(
        "MdfIndex.fromRangeSource: index build did not converge; the source may not be a valid MDF file.",
      );
    } finally {
      // The builder holds wasm-side fragment memory; release it on both the
      // success and error paths.
      builder.free();
    }
  }

  /**
   * Build a fresh index from an HTTP(S) URL, fetching only metadata.
   *
   * Convenience wrapper over `fromRangeSource` with a `FetchRangeSource`, so
   * only the file's metadata blocks are downloaded (a few KB) rather than the
   * whole file — as long as the server honours `Range` requests. Servers that
   * ignore `Range` fall back to a single full download. Each round of the
   * build fetches every byte range the walk still needs concurrently (see
   * `fromRangeSource`), so a file with many channel groups converges in a
   * handful of round-trips rather than one request per group. Once built,
   * read with `values`/`read` over a `RangeSource` for lazy, partial sample
   * reads.
   *
   * Pass `options` to tune the fetch windows for your file's shape — see
   * {@link IndexBuildOptions}.
   */
  static async fromUrl(
    url: string,
    fetchImpl?: FetchLike,
    options?: IndexBuildOptions,
  ): Promise<MdfIndex> {
    return MdfIndex.fromRangeSource(new FetchRangeSource(url, fetchImpl), options);
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
      const spans = coalesceNeeded(
        step.needed as [number, number][],
        CONVERSION_LOOKAHEAD_BYTES,
        CONVERSION_LOOKAHEAD_BYTES,
        size,
      );
      const fetched = await Promise.all(
        spans.map(async ([offset, length]): Promise<[ByteRange, Uint8Array]> => {
          const bytes = await source.read(offset, length);
          return [[offset, bytes.length], bytes];
        }),
      );
      for (const [range, bytes] of fetched) {
        ranges.push(range);
        fragments.push(bytes);
      }
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
