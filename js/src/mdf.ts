import { getWasmModule, type WasmMdf } from "./wasm-module";
import { readFileNodeOnly } from "./platform";
import {
  checkWindow,
  hasWindow,
  normaliseOptions,
  rangeOf,
  sliceSignal,
  timeAxisOf,
  toFloat64,
} from "./signal-util";
import type { GroupInfo, ReadOptions, Signal, TimeRange } from "./types";

/**
 * Read-only handle to an MDF 4 file parsed from an in-memory byte buffer.
 *
 * Thin, typed wrapper over the generated wasm `Mdf` class. Navigate by
 * group/channel **name**; samples are decoded lazily by `values`/`read`.
 */
export class Mdf {
  private readonly inner: WasmMdf;

  private constructor(inner: WasmMdf) {
    this.inner = inner;
  }

  /** Parse an MDF 4 file from an owned byte buffer. */
  static fromBytes(data: Uint8Array): Mdf {
    const wasm = getWasmModule();
    return new Mdf(wasm.Mdf.fromBytes(data));
  }

  /** Read an MDF 4 file from disk and parse it. Node only. */
  static async fromFile(path: string): Promise<Mdf> {
    return Mdf.fromBytes(await readFileNodeOnly(path, "Mdf.fromFile"));
  }

  /** Download an MDF 4 file in full over HTTP(S) and parse it. */
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

  /** Names of every named channel across all groups (duplicates kept). */
  channelNames(): string[] {
    return this.inner.channelNames();
  }

  /** Metadata for every channel group, each carrying its `channels`. */
  groups(): GroupInfo[] {
    return this.inner.groups() as GroupInfo[];
  }

  /**
   * Read a numeric channel as a plain `Float64Array` (no timestamps).
   * Conversions are applied; invalid/non-numeric samples become `NaN`.
   *
   * The second argument is a group name or a {@link ReadOptions} object
   * (`{ group, startTime, endTime }` or `{ group, startRecord, recordCount }`)
   * to read only part of the channel.
   */
  values(name: string, groupOrOptions?: string | null | ReadOptions): Float64Array {
    const opts = normaliseOptions(groupOrOptions);
    if (!hasWindow(opts)) {
      return this.inner.values(name, opts.group);
    }
    return toFloat64(this.read(name, opts).values);
  }

  /**
   * Read a channel as a `Signal` (values paired with its group's time axis).
   * For the master channel itself `timestamps` is empty: its `values` are
   * the time axis.
   *
   * The second argument is a group name or a {@link ReadOptions} window.
   */
  read(name: string, groupOrOptions?: string | null | ReadOptions): Signal {
    const opts = normaliseOptions(groupOrOptions);
    const signal = this.inner.read(name, opts.group) as Signal;
    if (!hasWindow(opts)) {
      return signal;
    }
    return sliceSignal(signal, checkWindow(opts), timeAxisOf(signal));
  }

  /**
   * First and last master (time) value of the group holding `name`, or
   * `undefined` for an empty group. Throws if the group has no master.
   */
  timeRange(name: string, group?: string | null): TimeRange | undefined {
    const signal = this.read(name, group);
    if (signal.values.length === 0) {
      return undefined;
    }
    if (signal.timestamps.length === 0 && !this.#isMaster(name, group)) {
      throw new Error(`Channel '${name}' has no master (time) channel.`);
    }
    return rangeOf(timeAxisOf(signal));
  }

  #isMaster(name: string, group?: string | null): boolean {
    return this.groups().some(
      (g) =>
        (group == null || g.name === group) &&
        g.channels.some((c) => c.name === name && c.isMaster),
    );
  }

  /**
   * Measurement start time in nanoseconds since the Unix epoch, or
   * `undefined` if the file header does not record one.
   */
  startTimeNs(): bigint | undefined {
    return this.inner.startTimeNs();
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
