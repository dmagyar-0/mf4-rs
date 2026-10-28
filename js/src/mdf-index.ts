import { getWasmModule, type WasmMdfIndex } from "./wasm-module";
import { readFileNodeOnly } from "./platform";
import {
  readAllRanges,
  BytesRangeSource,
  FetchRangeSource,
  type RangeSource,
  type FetchLike,
} from "./range-source";
import { checkWindow, hasWindow, rangeOf, timeAxisOf, toFloat64 } from "./signal-util";
import type {
  ByteRange,
  IndexChannelInfo,
  IndexGroupInfo,
  ReadOptions,
  Signal,
  TimeRange,
} from "./types";

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
/** Gap bridged into one request by windowed reads. The time search reads
 * single records spread over the file; once its probes close in, they bridge
 * into one request per round. No look-ahead: padding each of the ~32 probes
 * of a round would cost more bytes than it saves rounds. */
const WINDOW_BRIDGE_BYTES = 4 * 1024;

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
  #source: RangeSource | undefined;

  private constructor(inner: WasmMdfIndex, source?: RangeSource) {
    this.inner = inner;
    this.#source = source;
  }

  /**
   * Build a fresh index by parsing an MDF file from an in-memory buffer.
   *
   * All conversions are resolved during construction, so the index is fully
   * self-contained afterwards (and can be serialised with `toJson`). The
   * buffer becomes the index's bound {@link source}, so `values`/`read` work
   * without passing one; call `setSource(undefined)` to release it.
   */
  static fromBytes(data: Uint8Array): MdfIndex {
    const wasm = getWasmModule();
    return new MdfIndex(wasm.MdfIndex.fromBytes(data), new BytesRangeSource(data));
  }

  /**
   * Reload a previously serialised index from its JSON string. The source is
   * not part of the JSON: pass it here or attach it later with `setSource`.
   */
  static fromJson(json: string, source?: RangeSource): MdfIndex {
    const wasm = getWasmModule();
    return new MdfIndex(wasm.MdfIndex.fromJson(json), source);
  }

  /** Build a fresh index by parsing an MDF file from disk. Node only. */
  static async fromFile(path: string): Promise<MdfIndex> {
    return MdfIndex.fromBytes(await readFileNodeOnly(path, "MdfIndex.fromFile"));
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
   *
   * `source` becomes the index's bound {@link source}, so later
   * `values`/`read` calls need no source argument.
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
          return MdfIndex.fromJson(step.json as string, source);
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
   * handful of round-trips rather than one request per group. The URL stays
   * bound to the index, so `values(name)` / `read(name)` read lazily from it.
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

  /**
   * The source `values`/`read` fetch sample bytes from when none is passed:
   * set by `fromUrl` / `fromRangeSource` / `fromBytes` / `fromFile`, or with
   * `setSource`.
   */
  get source(): RangeSource | undefined {
    return this.#source;
  }

  /** Bind (or, with `undefined`, unbind) the default source for reads. */
  setSource(source: RangeSource | undefined): this {
    this.#source = source;
    return this;
  }

  /** Serialise the index to a JSON string (the bound source is not included). */
  toJson(): string {
    return this.inner.toJson();
  }

  /** Validate internal consistency of the index (block layout vs file size). */
  validate(): void {
    this.inner.validate();
  }

  /**
   * Metadata for every channel group: name, comment, record count, master
   * channel and per-channel `{ name, unit, comment, dataType, bitCount,
   * isMaster, isVlsd, hasConversion }`. Start/end times need sample data —
   * see {@link timeRange}.
   */
  groups(): IndexGroupInfo[] {
    return this.inner.groups() as IndexGroupInfo[];
  }

  /**
   * Metadata of one channel (first match, or within `group`), or `undefined`
   * if there is no such channel.
   */
  channel(name: string, group?: string | null): IndexChannelInfo | undefined {
    for (const g of this.groups()) {
      if (group != null && g.name !== group) continue;
      const found = g.channels.find((c) => c.name === name);
      if (found) return found;
    }
    return undefined;
  }

  /**
   * Measurement start time in nanoseconds since the Unix epoch, or
   * `undefined` if the file header does not record one. Master (time)
   * values are seconds relative to this — see `toAbsoluteNs`.
   */
  startTimeNs(): bigint | undefined {
    return this.inner.startTimeNs();
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
   * Byte ranges of every data block of the group owning `name`, merged.
   *
   * Returns one span per data-block fragment covering the whole block
   * (including its 24-byte header, which the decoder verifies); blocks stored
   * back to back are merged into one span. For a VLSD channel the spans of its
   * `##SD` fragment chain are included too, and for a column-oriented (MDF 4.2
   * remote-master) group so are the shared time group's blocks unless
   * `withMaster` is `false` (default `true`; pass `false` when only the values
   * are needed, as `values()` does). This is exactly the range set
   * the fragment readers need: `valuesFromFragments`/`readFromFragments`
   * decode whole data sections (all channels interleave per record), so fetch
   * these and pass the fetched fragments straight through — the requested
   * channel and its master are both covered for any record layout. This is
   * also what `values`/`read` fetch under the hood.
   */
  signalByteRanges(name: string, group?: string | null, withMaster = true): ByteRange[] {
    return this.inner.signalByteRanges(name, group, withMaster) as ByteRange[];
  }

  /**
   * Decode a numeric channel to a `Float64Array` from caller-fetched
   * fragments (invalid/non-numeric samples become `NaN`).
   *
   * `ranges`/`fragments` must be a matching pair: the ranges from
   * `signalByteRanges` (whole data blocks including their 24-byte headers,
   * which the decoder verifies — `byteRanges` spans are not sufficient) and
   * the bytes fetched for each span.
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
   * `ranges`/`fragments` must be a matching pair: the ranges from
   * `signalByteRanges` (whole data blocks including their 24-byte headers,
   * with the master's blocks) and the bytes fetched for each span.
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
   * bytes it needs (invalid/non-numeric samples become `NaN`).
   *
   * The second argument is either a `RangeSource` (the original signature,
   * `values(name, source, group)`) or a {@link ReadOptions} object; without
   * one the index's bound {@link source} is used. A window
   * (`startTime`/`endTime` or `startRecord`/`recordCount`) fetches only the
   * selected records: the time window is located by searching the master
   * channel (a few single-record reads), not by downloading it.
   *
   * Without a window, the group's whole data blocks (`signalByteRanges`,
   * without a remote master's time blocks) are fetched concurrently and
   * decoded via `valuesFromFragments`.
   */
  async values(
    name: string,
    sourceOrOptions?: RangeSource | ReadOptions | null,
    group?: string | null,
  ): Promise<Float64Array> {
    const { source, options } = this.#resolve(sourceOrOptions, group);
    if (hasWindow(options)) {
      return toFloat64((await this.#readWindow(name, options, source)).values);
    }
    const ranges = this.signalByteRanges(name, options.group, false);
    const fragments = await readAllRanges(source, ranges);
    // Indexes built over the network resolve conversions lazily; fetch the
    // channel's conversion blocks (if any) so the decoder can apply them.
    await this.#fetchConversionRanges(name, options.group, false, source, ranges, fragments);
    return this.valuesFromFragments(name, options.group, ranges, fragments);
  }

  /**
   * Lazily decode a channel to a `Signal` (values paired with the group's
   * time axis), fetching only the bytes it needs. For the master channel
   * itself `timestamps` is empty: its `values` are the time axis.
   *
   * Arguments as for {@link values}; a window reads only the selected
   * records of the channel and its master.
   */
  async read(
    name: string,
    sourceOrOptions?: RangeSource | ReadOptions | null,
    group?: string | null,
  ): Promise<Signal> {
    const { source, options } = this.#resolve(sourceOrOptions, group);
    if (hasWindow(options)) {
      return this.#readWindow(name, options, source);
    }
    const ranges = this.signalByteRanges(name, options.group);
    const fragments = await readAllRanges(source, ranges);
    // A signal read decodes the group master too, so resolve the master's
    // conversion as well as the channel's (withMaster = true).
    await this.#fetchConversionRanges(name, options.group, true, source, ranges, fragments);
    return this.readFromFragments(name, options.group, ranges, fragments);
  }

  /**
   * First and last master (time) value of the group holding `name` — the
   * group's start and end time, in seconds relative to `startTimeNs()` for a
   * time master. Reads just two records. `undefined` for an empty group;
   * throws if the group has no master channel.
   */
  async timeRange(
    name: string,
    sourceOrOptions?: RangeSource | Pick<ReadOptions, "group" | "source"> | null,
    group?: string | null,
  ): Promise<TimeRange | undefined> {
    const { source, options } = this.#resolve(sourceOrOptions, group);
    const info = this.channel(name, options.group);
    if (!info) {
      throw new Error(
        options.group == null
          ? `Channel '${name}' not found`
          : `Channel '${name}' not found in group '${options.group}'`,
      );
    }
    const total = this.#recordCount(name, options.group);
    if (total === 0) return undefined;
    const one = (startRecord: number) =>
      this.#windowStep(name, options.group, { startRecord, recordCount: 1 }, source);
    const [first, last] = await Promise.all([one(0), one(total - 1)]);
    if (first.timestamps.length === 0 && !info.isMaster) {
      throw new Error(`Channel '${name}' has no master (time) channel.`);
    }
    const a = rangeOf(timeAxisOf(first));
    const b = rangeOf(timeAxisOf(last));
    return a && b ? { start: a.start, end: b.end } : undefined;
  }

  /** Pick the source and options out of the `values`/`read` arguments. */
  #resolve(
    sourceOrOptions: RangeSource | ReadOptions | null | undefined,
    group: string | null | undefined,
  ): { source: RangeSource; options: ReadOptions } {
    let options: ReadOptions;
    if (sourceOrOptions && typeof (sourceOrOptions as RangeSource).read === "function") {
      options = { source: sourceOrOptions as RangeSource, group };
    } else {
      options = { ...((sourceOrOptions as ReadOptions | null | undefined) ?? {}) };
      if (options.group === undefined) options.group = group;
    }
    const source = options.source ?? this.#source;
    if (!source) {
      throw new Error(
        "MdfIndex has no source to read from: pass a RangeSource (or { source }), " +
          "build the index with fromUrl/fromRangeSource/fromBytes, or call setSource().",
      );
    }
    return { source, options };
  }

  #readWindow(name: string, options: ReadOptions, source: RangeSource): Promise<Signal> {
    return this.#windowStep(name, options.group, checkWindow(options), source);
  }

  /** Drive `readWindowStep` until every byte the windowed read needs is in. */
  async #windowStep(
    name: string,
    group: string | null | undefined,
    window: object,
    source: RangeSource,
  ): Promise<Signal> {
    const size = this.fileSize();
    const ranges: ByteRange[] = [];
    const fragments: Uint8Array[] = [];
    const maxRounds = 10_000;
    for (let round = 0; round < maxRounds; round++) {
      const step = this.inner.readWindowStep(name, group, window, ranges, fragments);
      if (step.done) {
        return step.signal as Signal;
      }
      const spans = coalesceNeeded(
        step.needed as [number, number][],
        WINDOW_BRIDGE_BYTES,
        0,
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
    throw new Error("MdfIndex: windowed read did not converge; the index or source may be inconsistent.");
  }

  /** Record count of the group `name` resolves to (first match / in `group`). */
  #recordCount(name: string, group: string | null | undefined): number {
    for (const g of this.groups()) {
      if (group != null && g.name !== group) continue;
      if (g.channelNames.includes(name)) return g.recordCount;
    }
    return 0;
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
