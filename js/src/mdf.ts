/**
 * Ergonomic reader over the wasm `Mdf` class.
 *
 * `Mdf` and `MdfIndex` share one reading API — the cross-language contract
 * documented in `docs/API.md`: `groups` / `group` / `channel` /
 * `channelNames` / `startTimeNs` for metadata, `read` / `values` for data,
 * `timeRange` / `conversion` for inspection. `Mdf` holds the whole file in
 * memory and is synchronous; `MdfIndex` reads lazily and is asynchronous.
 */

import { guard } from "./errors";
import { getWasmModule, type WasmMdf } from "./wasm-module";
import { readFileNodeOnly } from "./platform";
import { windowOf } from "./signal-util";
import type {
  ChannelInfo,
  ConversionInfo,
  GroupInfo,
  ReadOptions,
  Signal,
  TimeRange,
} from "./types";

export class Mdf {
  private readonly inner: WasmMdf;

  private constructor(inner: WasmMdf) {
    this.inner = inner;
  }

  /** Read an MDF 4 file from disk and parse it. Node only. */
  static async fromFile(path: string): Promise<Mdf> {
    return Mdf.fromBytes(await readFileNodeOnly(path, "Mdf.fromFile"));
  }

  /** Parse an MDF 4 file from a byte buffer. */
  static fromBytes(data: Uint8Array): Mdf {
    const wasm = getWasmModule();
    return new Mdf(wasm.Mdf.fromBytes(data));
  }

  /**
   * Download an MDF 4 file in full over HTTP(S) and parse it. To read a
   * remote file *without* downloading it, use `MdfIndex.fromUrl`.
   */
  static async fromUrl(url: string, fetchImpl?: typeof fetch): Promise<Mdf> {
    const impl = fetchImpl ?? globalThis.fetch;
    if (!impl) {
      throw new Error("No fetch implementation available; pass one explicitly to fromUrl().");
    }
    // Called as a plain function: a browser `fetch` invoked with another
    // receiver throws "Illegal invocation".
    const res = await impl.call(globalThis, url);
    if (!res.ok) {
      throw new Error(`Mdf.fromUrl: request failed with status ${res.status}`);
    }
    const buf = new Uint8Array(await res.arrayBuffer());
    return Mdf.fromBytes(buf);
  }

  /** Metadata of every channel group, in file order. */
  groups(): GroupInfo[] {
    return this.inner.groups() as GroupInfo[];
  }

  /** Names of every named channel across all groups (duplicates kept). */
  channelNames(): string[] {
    return this.inner.channelNames();
  }

  /**
   * Measurement start time in nanoseconds since the Unix epoch, or
   * `undefined` if the file header does not record one.
   */
  startTimeNs(): bigint | undefined {
    return this.inner.startTimeNs();
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
   * Read a channel as a `Signal`: its values (conversions applied) paired
   * with its group's time axis. For the master channel itself `timestamps`
   * is empty: its `values` are the time axis.
   *
   * `options` selects a group and/or a window (`{ group, startTime, endTime }`
   * or `{ group, startRecord, recordCount }`). Throws
   * `ChannelNotFoundError` if no such channel exists.
   */
  read(name: string, options: ReadOptions = {}): Signal {
    const window = windowOf(options);
    return guard(() => this.inner.read(name, options.group, window) as Signal);
  }

  /**
   * Read a numeric channel as a plain `Float64Array` (no timestamps).
   * Conversions are applied; invalid and non-numeric samples are `NaN`.
   * `options` as for {@link read}.
   */
  values(name: string, options: ReadOptions = {}): Float64Array {
    const window = windowOf(options);
    return guard(() => this.inner.values(name, options.group, window));
  }

  /**
   * First and last master (time) value of the group holding `name`, or
   * `undefined` for an empty group. Throws if the group has no master.
   */
  timeRange(name: string, group?: string | null): TimeRange | undefined {
    return guard(() => this.inner.timeRange(name, group) as TimeRange | undefined);
  }

  /**
   * The conversion `read` / `values` apply to channel `name`, fully
   * resolved, or `null` when the channel has none.
   */
  conversion(name: string, group?: string | null): ConversionInfo | null {
    return guard(() => this.inner.conversion(name, group) as ConversionInfo | null);
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
