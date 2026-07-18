import { loadWasmModule, type WasmMdf } from "./wasm-module";
import type { GroupInfo, Signal } from "./types";

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
    const wasm = loadWasmModule();
    return new Mdf(wasm.Mdf.fromBytes(data));
  }

  /** Read an MDF 4 file from disk and parse it. Node only. */
  static async fromFile(path: string): Promise<Mdf> {
    const fsp = await import("node:fs/promises");
    const buf = await fsp.readFile(path);
    return Mdf.fromBytes(new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength));
  }

  /** Download an MDF 4 file in full over HTTP(S) and parse it. */
  static async fromUrl(url: string, fetchImpl?: typeof fetch): Promise<Mdf> {
    const impl = fetchImpl ?? globalThis.fetch;
    if (!impl) {
      throw new Error("No fetch implementation available; pass one explicitly to fromUrl().");
    }
    const res = await impl(url);
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
   */
  values(name: string, group?: string | null): Float64Array {
    return this.inner.values(name, group);
  }

  /** Read a channel as a `Signal` (values paired with its group's time axis). */
  read(name: string, group?: string | null): Signal {
    return this.inner.read(name, group) as Signal;
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
