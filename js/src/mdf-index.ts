import { getWasmModule, type WasmMdfIndex } from "./wasm-module";
import { readFileNodeOnly, writeFileNodeOnly } from "./platform";
import {
  BytesRangeSource,
  FetchRangeSource,
  type RangeSource,
  type FetchLike,
  type FetchRangeSourceOptions,
} from "./range-source";
import { ChannelNotFoundError, guard } from "./errors";
import { hasWindow, wasmWindow } from "./signal-util";
import { mergeByteRanges } from "./byte-ranges";
import type {
  ByteRange,
  ChannelInfo,
  ConversionInfo,
  GroupInfo,
  ReadOptions,
  Signal,
  TimeRange,
} from "./types";

/** Ranges longer than this (8 MiB) are fetched as several concurrent
 * requests, as the native reader does: one connection to an object store is
 * far slower than several. */
const FETCH_CHUNK_BYTES = 8 * 1024 * 1024;
/** Most requests one fetch round keeps in flight. */
const MAX_CONCURRENT_FETCHES = 16;
/** `readMany` of whole channels fetches the whole batch at once only while it
 * needs at most this many bytes (256 MiB); a larger batch is read channel by
 * channel so its memory stays bounded. */
const READ_MANY_BATCH_BYTES = 256 * 1024 * 1024;

/**
 * Read `[offset, offset + length)` and insist on getting all of it: a short
 * read would otherwise look like a gap that never closes.
 */
async function readExact(source: RangeSource, offset: number, length: number): Promise<Uint8Array> {
  const bytes = await source.read(offset, length);
  if (bytes.length < length) {
    throw new Error(
      `MdfIndex: the source returned ${bytes.length} of the ${length} bytes requested at offset ` +
        `${offset}; it may be truncated or not match the index.`,
    );
  }
  return bytes;
}

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

  /**
   * Reload an index from a JSON file written by {@link save}. Node only. The
   * source is not part of the file: pass it here or attach it with `setSource`.
   */
  static async load(path: string, source?: RangeSource): Promise<MdfIndex> {
    const bytes = await readFileNodeOnly(path, "MdfIndex.load");
    return MdfIndex.fromJson(new TextDecoder().decode(bytes), source);
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
   * {@link IndexBuildOptions} — and the source's retries and timeout — see
   * {@link FetchRangeSourceOptions}.
   */
  static async fromUrl(
    url: string,
    fetchImpl?: FetchLike,
    options?: IndexBuildOptions & FetchRangeSourceOptions,
  ): Promise<MdfIndex> {
    return MdfIndex.fromRangeSource(new FetchRangeSource(url, fetchImpl, options), options);
  }

  /**
   * The source `read` / `values` fetch sample bytes from: set by `fromUrl` /
   * `fromRangeSource` / `fromBytes` / `fromFile`, or with `setSource`.
   */
  source(): RangeSource | undefined {
    return this.#source;
  }

  /** Attach (or, with `undefined`, detach) the source for reads. */
  setSource(source: RangeSource | undefined): this {
    this.#source = source;
    return this;
  }

  /** Serialise the index to a JSON string (the source is not included). */
  toJson(): string {
    return this.inner.toJson();
  }

  /** Write the index as JSON to `path`. Node only. */
  async save(path: string): Promise<void> {
    await writeFileNodeOnly(path, new TextEncoder().encode(this.toJson()), "MdfIndex.save");
  }

  /** Validate internal consistency of the index (block layout vs file size). */
  validate(): void {
    this.inner.validate();
  }

  /** Metadata of every channel group, in file order. */
  groups(): GroupInfo[] {
    return this.inner.groups() as GroupInfo[];
  }

  /** Metadata of the first channel group named `name`, or `undefined`. */
  group(name: string): GroupInfo | undefined {
    return this.inner.group(name) as GroupInfo | undefined;
  }

  /**
   * Metadata of the first channel named `name`, or `undefined`. Pass `group`
   * to look only in the group(s) with that name.
   */
  channel(name: string, group?: string | null): ChannelInfo | undefined {
    return this.inner.channel(name, group) as ChannelInfo | undefined;
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
    return guard(() => this.inner.byteRanges(name, group) as ByteRange[]);
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
    return guard(
      () =>
        this.inner.byteRangesForRecords(
          name,
          BigInt(startRecord),
          BigInt(recordCount),
          group,
        ) as ByteRange[],
    );
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
    return guard(() => this.inner.signalByteRanges(name, group, withMaster) as ByteRange[]);
  }

  /**
   * Decode a numeric channel to a `Float64Array` from caller-fetched
   * fragments (invalid/non-numeric samples become `NaN`).
   *
   * `ranges`/`fragments` must be a matching pair: the ranges from
   * `signalByteRanges` (whole data blocks including their 24-byte headers,
   * which the decoder verifies — `byteRanges` spans are not sufficient) and
   * the bytes fetched for each span. `raw` skips the channel's conversion.
   */
  valuesFromFragments(
    name: string,
    group: string | null | undefined,
    ranges: ByteRange[],
    fragments: Uint8Array[],
    raw = false,
  ): Float64Array {
    return guard(() => this.inner.valuesFromFragments(name, group, ranges, fragments, raw));
  }

  /**
   * Decode a channel to a `Signal` from caller-fetched fragments.
   *
   * `ranges`/`fragments` must be a matching pair: the ranges from
   * `signalByteRanges` (whole data blocks including their 24-byte headers,
   * with the master's blocks) and the bytes fetched for each span. `raw`
   * skips the channel's conversion (timestamps stay physical).
   */
  readFromFragments(
    name: string,
    group: string | null | undefined,
    ranges: ByteRange[],
    fragments: Uint8Array[],
    raw = false,
  ): Signal {
    return guard(() => this.inner.readFromFragments(name, group, ranges, fragments, raw) as Signal);
  }

  /**
   * Read a channel as a `Signal` (values paired with the group's time axis),
   * fetching only the bytes it needs from the source. For the master channel
   * itself `timestamps` is empty: its `values` are the time axis. With
   * `options.raw` the values are the stored ones, without the conversion
   * (timestamps stay physical; the conversion block is never fetched).
   *
   * `options` selects a group and/or a window (`{ group, startTime, endTime }`
   * or `{ group, startRecord, recordCount }`). A window fetches only the
   * selected records: a time window is located by searching the master
   * channel (a few single-record reads), not by downloading it. Throws
   * `ChannelNotFoundError` if no such channel exists.
   */
  async read(name: string, options: ReadOptions = {}): Promise<Signal> {
    const window = wasmWindow(options);
    const raw = window.raw === true;
    this.#requireChannel(name, options.group);
    const source = this.#requireSource(options);
    if (hasWindow(options)) {
      const step = await this.#gapLoop(source, (ranges, fragments) =>
        this.inner.readWindowStep(name, options.group, window, ranges, fragments),
      );
      return step.signal as Signal;
    }
    const ranges = this.signalByteRanges(name, options.group);
    const fragments = await this.#fetchAll(source, ranges);
    // A signal read decodes the group master too, so resolve the master's
    // conversion as well as the channel's (withMaster = true; a raw read
    // skips the channel's own).
    await this.#fetchConversionRanges(name, options.group, true, source, ranges, fragments, raw);
    return this.readFromFragments(name, options.group, ranges, fragments, raw);
  }

  /**
   * Read several channels at once, with the same `options` for all of them:
   * one `Signal` per name, in order — like calling {@link read} for each,
   * but every byte range of the batch is fetched concurrently, a time axis
   * shared by several of the channels (every signal of a source group in a
   * column-oriented file) is fetched once, and a time window is searched
   * once per time axis. Ranges over 8 MiB are fetched as concurrent chunks,
   * and a whole-channel batch over 256 MiB is read channel by channel to
   * bound memory. A windowed batch keeps every selected record in memory
   * until it is decoded — for very wide windows over many channels, call
   * {@link read} per channel instead. Throws `ChannelNotFoundError` — before
   * fetching anything — if any name is unknown.
   */
  async readMany(names: string[], options: ReadOptions = {}): Promise<Signal[]> {
    const window = wasmWindow(options);
    const raw = window.raw === true;
    for (const name of names) this.#requireChannel(name, options.group);
    if (names.length === 0) return [];
    const source = this.#requireSource(options);
    if (hasWindow(options)) {
      const step = await this.#gapLoop(source, (r, f) =>
        this.inner.readManyStep(names, options.group, window, r, f),
      );
      return step.signals as Signal[];
    }
    // Whole channels: every data block of the batch (channels and their time
    // axes, each once) and any deferred conversion blocks, fetched
    // concurrently in one go...
    const perName = names.map((name) => this.signalByteRanges(name, options.group));
    const spans = mergeByteRanges(perName.flat());
    const batchBytes = spans.reduce((sum, [, length]) => sum + length, 0);
    if (batchBytes > READ_MANY_BATCH_BYTES) {
      // Too much to hold at once: one channel (and its time axis) at a time.
      const conversions = await this.#fetchBatchConversions(names, options.group, source, raw);
      const signals: Signal[] = [];
      for (const [i, name] of names.entries()) {
        const own = mergeByteRanges(perName[i]!);
        const fetched = await this.#fetchAll(source, own);
        signals.push(
          this.readFromFragments(
            name,
            options.group,
            [...conversions.ranges, ...own],
            [...conversions.fragments, ...fetched],
            raw,
          ),
        );
      }
      return signals;
    }
    const [fetched, conversions] = await Promise.all([
      this.#fetchAll(source, spans),
      this.#fetchBatchConversions(names, options.group, source, raw),
    ]);
    // ...then decoded one channel at a time from zero-copy views of the
    // fetched bytes, so wasm never holds more than one channel's samples.
    // (`mergeByteRanges` only unions, so every range lies inside one span.)
    return names.map((name, i) => {
      const ranges: ByteRange[] = [...conversions.ranges];
      const fragments: Uint8Array[] = [...conversions.fragments];
      for (const [offset, length] of perName[i]!) {
        const at = spans.findIndex(([o, l]) => o <= offset && offset + length <= o + l);
        const start = offset - spans[at]![0];
        ranges.push([offset, length]);
        fragments.push(fetched[at]!.subarray(start, start + length));
      }
      return this.readFromFragments(name, options.group, ranges, fragments, raw);
    });
  }

  /**
   * Read a numeric channel as a plain `Float64Array` (no timestamps),
   * fetching only the bytes it needs. Conversions are applied unless
   * `options.raw`; invalid and non-numeric samples are `NaN`. `options` as
   * for {@link read}.
   */
  async values(name: string, options: ReadOptions = {}): Promise<Float64Array> {
    const window = wasmWindow(options);
    const raw = window.raw === true;
    this.#requireChannel(name, options.group);
    const source = this.#requireSource(options);
    if (hasWindow(options)) {
      const step = await this.#gapLoop(source, (ranges, fragments) =>
        this.inner.valuesWindowStep(name, options.group, window, ranges, fragments),
      );
      return step.values!;
    }
    const ranges = this.signalByteRanges(name, options.group, false);
    const fragments = await this.#fetchAll(source, ranges);
    // Indexes built over the network resolve conversions lazily; fetch the
    // channel's conversion blocks (if any) so the decoder can apply them —
    // none for a raw read.
    await this.#fetchConversionRanges(name, options.group, false, source, ranges, fragments, raw);
    return this.valuesFromFragments(name, options.group, ranges, fragments, raw);
  }

  /**
   * First and last master (time) value of the group holding `name` — the
   * group's start and end time, in seconds relative to `startTimeNs()` for a
   * time master. Reads just two records. `undefined` for an empty group;
   * throws if the group has no master channel.
   */
  async timeRange(name: string, group?: string | null): Promise<TimeRange | undefined> {
    this.#requireChannel(name, group);
    const source = this.#requireSource({});
    const step = await this.#gapLoop(source, (ranges, fragments) =>
      this.inner.timeRangeStep(name, group, ranges, fragments),
    );
    return step.range as TimeRange | undefined;
  }

  /**
   * The conversion `read` / `values` apply to channel `name`, fully
   * resolved, or `null` when the channel has none. Indexes built over the
   * network fetch the conversion block from the source on demand.
   */
  async conversion(name: string, group?: string | null): Promise<ConversionInfo | null> {
    this.#requireChannel(name, group);
    const ranges: ByteRange[] = [];
    const fragments: Uint8Array[] = [];
    // Conversion chains are short; a small look-ahead keeps this to one or
    // two round-trips even when a block and its text refs sit adjacently.
    const lookahead = 8 * 1024;
    for (let round = 0; round < 10_000; round++) {
      const step = this.inner.conversionStep(name, group, ranges, fragments);
      if (step.done) return (step.conversion ?? null) as ConversionInfo | null;
      const source = this.#requireSource({});
      const spans = coalesceNeeded(step.needed!, lookahead, lookahead, this.fileSize());
      const fetched = await this.#fetchAll(source, spans);
      spans.forEach(([offset], i) => {
        ranges.push([offset, fetched[i]!.length]);
        fragments.push(fetched[i]!);
      });
    }
    throw new Error("MdfIndex: conversion resolution did not converge; the index or source may be inconsistent.");
  }

  /** The channel's metadata, or a `ChannelNotFoundError`. */
  #requireChannel(name: string, group: string | null | undefined): ChannelInfo {
    const info = this.channel(name, group);
    if (!info) throw new ChannelNotFoundError(name, group);
    return info;
  }

  #requireSource(options: ReadOptions): RangeSource {
    const source = options.source ?? this.#source;
    if (!source) {
      throw new Error(
        "MdfIndex has no source to read from: build the index with " +
          "fromUrl/fromRangeSource/fromBytes/fromFile, or call setSource().",
      );
    }
    return source;
  }

  /**
   * Drive a wasm step function until every byte its read needs is in: each
   * call either finishes or reports the byte ranges still missing, which are
   * fetched from `source` and handed back on the next call.
   */
  async #gapLoop<T extends { done: boolean; needed?: [number, number][] }>(
    source: RangeSource,
    step: (ranges: ByteRange[], fragments: Uint8Array[]) => T,
    ranges: ByteRange[] = [],
    fragments: Uint8Array[] = [],
  ): Promise<T> {
    const size = this.fileSize();
    const maxRounds = 10_000;
    for (let round = 0; round < maxRounds; round++) {
      const result = guard(() => step(ranges, fragments));
      if (result.done) {
        return result;
      }
      const spans = coalesceNeeded(result.needed!, WINDOW_BRIDGE_BYTES, 0, size);
      const fetched = await this.#fetchAll(source, spans);
      spans.forEach(([offset], i) => {
        ranges.push([offset, fetched[i]!.length]);
        fragments.push(fetched[i]!);
      });
    }
    throw new Error("MdfIndex: read did not converge; the index or source may be inconsistent.");
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
    raw = false,
  ): Promise<void> {
    const size = this.fileSize();
    // Conversion chains are short; a small look-ahead keeps this to one or two
    // round-trips even when a block and its text refs sit adjacently.
    const CONVERSION_LOOKAHEAD_BYTES = 8 * 1024;
    const maxRounds = 10_000;
    for (let round = 0; round < maxRounds; round++) {
      const step = this.inner.conversionRangesStep(name, group, withMaster, ranges, fragments, raw);
      if (step.done) return;
      const spans = coalesceNeeded(
        step.needed as [number, number][],
        CONVERSION_LOOKAHEAD_BYTES,
        CONVERSION_LOOKAHEAD_BYTES,
        size,
      );
      const fetched = await this.#fetchAll(source, spans);
      spans.forEach(([offset], i) => {
        ranges.push([offset, fetched[i]!.length]);
        fragments.push(fetched[i]!);
      });
    }
    throw new Error(
      "MdfIndex: conversion resolution did not converge; the index or source may be inconsistent.",
    );
  }

  /**
   * Fetch the deferred conversion blocks a whole `readMany` batch needs
   * (channels and their time axes, each block once): a no-op for a locally
   * built index. Only conversion fragments are passed to wasm each round,
   * never the batch's data.
   */
  async #fetchBatchConversions(
    names: string[],
    group: string | null | undefined,
    source: RangeSource,
    raw: boolean,
  ): Promise<{ ranges: ByteRange[]; fragments: Uint8Array[] }> {
    const ranges: ByteRange[] = [];
    const fragments: Uint8Array[] = [];
    const size = this.fileSize();
    const CONVERSION_LOOKAHEAD_BYTES = 8 * 1024;
    for (let round = 0; round < 10_000; round++) {
      const step = this.inner.conversionRangesManyStep(names, group, true, ranges, fragments, raw);
      if (step.done) return { ranges, fragments };
      const spans = coalesceNeeded(step.needed!, CONVERSION_LOOKAHEAD_BYTES, CONVERSION_LOOKAHEAD_BYTES, size);
      const fetched = await this.#fetchAll(source, spans);
      spans.forEach(([offset], i) => {
        ranges.push([offset, fetched[i]!.length]);
        fragments.push(fetched[i]!);
      });
    }
    throw new Error(
      "MdfIndex: conversion resolution did not converge; the index or source may be inconsistent.",
    );
  }

  /**
   * Fetch `spans` from `source` (each in full, one `Uint8Array` per span),
   * at most {@link MAX_CONCURRENT_FETCHES} requests at a time, a span longer
   * than {@link FETCH_CHUNK_BYTES} as several concurrent chunk requests.
   * Refuses a source whose size — once it is known without an extra
   * request, e.g. from a `Content-Range` header — differs from the indexed
   * file's: the file behind it was replaced, and its bytes would be decoded
   * at offsets meant for another file.
   */
  async #fetchAll(source: RangeSource, spans: ByteRange[]): Promise<Uint8Array[]> {
    const out: Uint8Array[] = spans.map(([, length]) =>
      length > FETCH_CHUNK_BYTES ? new Uint8Array(length) : new Uint8Array(0),
    );
    const jobs: (() => Promise<void>)[] = [];
    spans.forEach(([offset, length], i) => {
      if (length <= FETCH_CHUNK_BYTES) {
        jobs.push(async () => {
          out[i] = await readExact(source, offset, length);
        });
        return;
      }
      for (let at = 0; at < length; at += FETCH_CHUNK_BYTES) {
        const size = Math.min(FETCH_CHUNK_BYTES, length - at);
        jobs.push(async () => {
          out[i]!.set((await readExact(source, offset + at, size)).subarray(0, size), at);
        });
      }
    });
    try {
      let next = 0;
      let failed = false;
      // After a failure no further request is started.
      const worker = async (): Promise<void> => {
        while (!failed && next < jobs.length) {
          try {
            await jobs[next++]!();
          } catch (e) {
            failed = true;
            throw e;
          }
        }
      };
      await Promise.all(Array.from({ length: Math.min(MAX_CONCURRENT_FETCHES, jobs.length) }, worker));
    } catch (e) {
      // A size mismatch explains the failure better; keep the original as its cause.
      this.#checkSourceSize(source, e);
      throw e;
    }
    this.#checkSourceSize(source);
    return out;
  }

  #checkSourceSize(source: RangeSource, cause?: unknown): void {
    const actual = source.knownSize?.();
    const expected = this.fileSize();
    if (actual !== undefined && expected > 0 && actual !== expected) {
      throw new Error(
        `the source does not match the index: the index was built from a ${expected}-byte ` +
          `file but the source is ${actual} bytes — attach the file the index was built from, ` +
          `or rebuild the index`,
        cause === undefined ? undefined : { cause },
      );
    }
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
