import { loadWasmModule, type WasmMdfIndex } from "./wasm-module";
import { readAllRanges, type RangeSource } from "./range-source";
import { mergeByteRanges } from "./byte-ranges";
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
   * Byte ranges `[[offset, length], ...]` occupied by a channel.
   *
   * Power-user API: each span coalesces one data-block fragment and, because
   * records interleave all channels, includes neighbouring channels' bytes.
   * It does **not** reliably cover the channel's master/time axis, nor
   * necessarily whole data sections — prefer `signalByteRanges` for reads.
   */
  byteRanges(name: string, group?: string | null): ByteRange[] {
    return this.inner.byteRanges(name, group) as ByteRange[];
  }

  /** Byte ranges covering a record window `[startRecord, startRecord+recordCount)`. */
  byteRangesForRecords(
    name: string,
    startRecord: bigint,
    recordCount: bigint,
    group?: string | null,
  ): ByteRange[] {
    return this.inner.byteRangesForRecords(name, startRecord, recordCount, group) as ByteRange[];
  }

  /**
   * Byte ranges covering both a channel and its group master, merged.
   *
   * This is the range set to fetch for `values`/`read` (and for
   * `valuesFromFragments`/`readFromFragments` directly) — it also covers
   * each full data section for the typical master-first record layout.
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
   * Computes the byte ranges to fetch, reads them from `source`
   * concurrently, then decodes via `valuesFromFragments`. See
   * `fullDataSectionRanges` for why this fetches more than
   * `signalByteRanges` alone when the group has channels beyond the
   * master and the requested one.
   */
  async values(name: string, source: RangeSource, group?: string | null): Promise<Float64Array> {
    const ranges = this.fullDataSectionRanges(name, group);
    const fragments = await readAllRanges(source, ranges);
    return this.valuesFromFragments(name, group, ranges, fragments);
  }

  /**
   * Lazily decode a channel to a `Signal`, fetching only the bytes it needs
   * from `source`.
   *
   * Computes the byte ranges to fetch, reads them from `source`
   * concurrently, then decodes via `readFromFragments`. See
   * `fullDataSectionRanges` for why this fetches more than
   * `signalByteRanges` alone when the group has channels beyond the
   * master and the requested one.
   */
  async read(name: string, source: RangeSource, group?: string | null): Promise<Signal> {
    const ranges = this.fullDataSectionRanges(name, group);
    const fragments = await readAllRanges(source, ranges);
    return this.readFromFragments(name, group, ranges, fragments);
  }

  /**
   * Byte ranges that reliably cover the *entire* data section(s) backing a
   * channel's group, merged and coalesced.
   *
   * `valuesFromFragments`/`readFromFragments` decode by reading whole data
   * blocks, not just the columns for the requested channel — in testing,
   * `signalByteRanges` (master + requested channel only) undershoots
   * whenever the group has additional channels declared after the
   * requested one, because their trailing bytes in the final record are
   * never included in the merged master+channel span. Unioning every
   * channel's `byteRanges` in the owning group reliably covers the full
   * data section (and degrades to exactly `signalByteRanges` for a plain
   * two-channel master+data group, so there is no extra cost in the common
   * case).
   */
  private fullDataSectionRanges(name: string, group?: string | null): ByteRange[] {
    const groups = this.groups();
    const owning =
      groups.find((g) => (group ? g.name === group : g.channelNames.includes(name))) ??
      groups.find((g) => g.channelNames.includes(name));
    const channelNames = owning ? owning.channelNames : [name];
    const resolvedGroup = group ?? owning?.name ?? undefined;

    const all: ByteRange[] = [];
    for (const channelName of channelNames) {
      all.push(...this.byteRanges(channelName, resolvedGroup));
    }
    return mergeByteRanges(all);
  }

  /** Free the underlying wasm memory. Safe to call multiple times. */
  dispose(): void {
    this.inner.free();
  }
}
