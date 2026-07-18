/**
 * Sources of raw bytes for lazy, partial reads via `MdfIndex`.
 *
 * `MdfIndex.values`/`read` call `signalByteRanges` to find out which byte
 * spans of the source MDF file are needed, then use a `RangeSource` to fetch
 * exactly those spans (in parallel) before handing them to the wasm decoder.
 */

import type { ByteRange } from "./types";

/** Minimal fetch signature compatible with the global `fetch`. */
export type FetchLike = (
  input: string,
  init?: {
    headers?: Record<string, string>;
    signal?: AbortSignal;
  },
) => Promise<{
  ok: boolean;
  status: number;
  headers: { get(name: string): string | null };
  arrayBuffer(): Promise<ArrayBuffer>;
}>;

/**
 * Abstraction over "fetch N bytes at offset O" for an MDF data source.
 *
 * Implementations: `FetchRangeSource` (HTTP), `FileRangeSource` (Node
 * filesystem), `BytesRangeSource` (in-memory buffer).
 */
export interface RangeSource {
  /** Total size of the underlying data in bytes, if cheaply known. */
  size?(): Promise<number>;
  /** Read `length` bytes starting at `offset`. */
  read(offset: number, length: number): Promise<Uint8Array>;
}

/**
 * Fetch multiple byte ranges from a `RangeSource` concurrently, preserving
 * order. Shared by `MdfIndex.values`/`read`.
 */
export async function readAllRanges(
  source: RangeSource,
  ranges: ByteRange[],
): Promise<Uint8Array[]> {
  return Promise.all(ranges.map(([offset, length]) => source.read(offset, length)));
}

/**
 * `RangeSource` backed by HTTP `Range: bytes=start-end` requests via
 * `fetch`.
 *
 * Servers that ignore the `Range` header and return the full body with
 * `200 OK` are handled transparently by slicing the response client-side.
 */
export class FetchRangeSource implements RangeSource {
  private readonly url: string;
  private readonly fetchImpl: FetchLike;
  private cachedSize: number | undefined;
  /** Full body cached after a `200` (Range-ignoring) response, served locally. */
  private cachedBody: Uint8Array | undefined;

  constructor(url: string, fetchImpl?: FetchLike) {
    this.url = url;
    const impl = fetchImpl ?? (globalThis as { fetch?: FetchLike }).fetch;
    if (!impl) {
      throw new Error(
        "No fetch implementation available. Pass one explicitly: " +
          "new FetchRangeSource(url, fetchImpl)",
      );
    }
    this.fetchImpl = impl;
  }

  async size(): Promise<number> {
    if (this.cachedSize !== undefined) {
      return this.cachedSize;
    }
    const res = await this.fetchImpl(this.url, { headers: { Range: "bytes=0-0" } });
    if (!res.ok) {
      throw new Error(`FetchRangeSource: HEAD-equivalent request failed with status ${res.status}`);
    }
    if (res.status === 206) {
      const contentRange = res.headers.get("content-range");
      if (contentRange) {
        const total = contentRange.split("/")[1];
        if (total && total !== "*") {
          this.cachedSize = Number(total);
          return this.cachedSize;
        }
      }
    }
    // Server ignored Range and sent the full body; cache it for later reads.
    const buf = new Uint8Array(await res.arrayBuffer());
    this.cachedBody = buf;
    this.cachedSize = buf.byteLength;
    return this.cachedSize;
  }

  async read(offset: number, length: number): Promise<Uint8Array> {
    if (length <= 0) {
      return new Uint8Array(0);
    }
    // A previous request revealed the server ignores Range; serve from cache.
    if (this.cachedBody !== undefined) {
      return this.sliceCached(offset, length);
    }

    const end = offset + length - 1;
    const res = await this.fetchImpl(this.url, {
      headers: { Range: `bytes=${offset}-${end}` },
    });
    if (!res.ok) {
      throw new Error(`FetchRangeSource: request failed with status ${res.status}`);
    }
    const buf = new Uint8Array(await res.arrayBuffer());

    if (res.status === 206) {
      // Server honoured the Range request — validate what it returned.
      const contentRange = res.headers.get("content-range");
      if (contentRange) {
        const match = /^bytes\s+(\d+)-\d+\/(?:\d+|\*)$/.exec(contentRange.trim());
        if (match && Number(match[1]) !== offset) {
          throw new Error(
            `FetchRangeSource: server returned range starting at ${match[1]}, expected ${offset}`,
          );
        }
      }
      if (buf.byteLength !== length) {
        throw new Error(
          `FetchRangeSource: 206 response body is ${buf.byteLength} bytes but ${length} were ` +
            `requested at offset ${offset}`,
        );
      }
      return buf;
    }

    // Server returned 200 with the full body (ignored Range) — cache and slice.
    this.cachedBody = buf;
    if (this.cachedSize === undefined) {
      this.cachedSize = buf.byteLength;
    }
    return this.sliceCached(offset, length);
  }

  private sliceCached(offset: number, length: number): Uint8Array {
    const body = this.cachedBody!;
    if (offset + length > body.byteLength) {
      throw new RangeError(
        `FetchRangeSource: requested [${offset}, ${offset + length}) exceeds cached body ` +
          `length ${body.byteLength}`,
      );
    }
    return body.subarray(offset, offset + length);
  }
}

/**
 * `RangeSource` backed by a local file, read via `fs/promises`.
 *
 * Node-only.
 */
export class FileRangeSource implements RangeSource {
  private readonly path: string;
  private handlePromise: Promise<import("node:fs/promises").FileHandle> | undefined;

  constructor(path: string) {
    this.path = path;
  }

  private async handle(): Promise<import("node:fs/promises").FileHandle> {
    if (!this.handlePromise) {
      const fsp = await import("node:fs/promises");
      this.handlePromise = fsp.open(this.path, "r");
    }
    return this.handlePromise;
  }

  async size(): Promise<number> {
    const fh = await this.handle();
    const stat = await fh.stat();
    return stat.size;
  }

  async read(offset: number, length: number): Promise<Uint8Array> {
    if (length <= 0) {
      return new Uint8Array(0);
    }
    const fh = await this.handle();
    const buffer = Buffer.alloc(length);
    // A single fh.read may return fewer bytes than requested; loop until the
    // whole span is read, treating a 0-byte read as EOF.
    let total = 0;
    while (total < length) {
      const { bytesRead } = await fh.read(buffer, total, length - total, offset + total);
      if (bytesRead === 0) {
        throw new Error(
          `FileRangeSource: short read at offset ${offset + total} — wanted ${length - total} ` +
            `more byte(s) but hit EOF (requested [${offset}, ${offset + length}))`,
        );
      }
      total += bytesRead;
    }
    return new Uint8Array(buffer.buffer, buffer.byteOffset, length);
  }

  /** Close the underlying file handle, if opened. */
  async close(): Promise<void> {
    if (this.handlePromise) {
      const fh = await this.handlePromise;
      await fh.close();
      this.handlePromise = undefined;
    }
  }
}

/** `RangeSource` backed by an in-memory buffer (e.g. an already-downloaded file). */
export class BytesRangeSource implements RangeSource {
  private readonly bytes: Uint8Array;

  constructor(bytes: Uint8Array) {
    this.bytes = bytes;
  }

  async size(): Promise<number> {
    return this.bytes.byteLength;
  }

  async read(offset: number, length: number): Promise<Uint8Array> {
    if (length <= 0) {
      return new Uint8Array(0);
    }
    if (offset < 0 || offset + length > this.bytes.byteLength) {
      throw new RangeError(
        `BytesRangeSource: requested [${offset}, ${offset + length}) exceeds buffer length ` +
          `${this.bytes.byteLength}`,
      );
    }
    return this.bytes.subarray(offset, offset + length);
  }
}
