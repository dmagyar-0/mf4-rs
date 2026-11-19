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
 * Implementations: `FetchRangeSource` (HTTP), `BytesRangeSource` (in-memory
 * buffer) and, from the Node entry point only, `FileRangeSource` (filesystem).
 */
export interface RangeSource {
  /** Total size of the underlying data in bytes, if cheaply known. */
  size?(): Promise<number>;
  /** Read `length` bytes starting at `offset`. */
  read(offset: number, length: number): Promise<Uint8Array>;
  /**
   * Total size of the underlying data if it is already known **without** a
   * further request (e.g. learned from a previous response), else
   * `undefined`. `MdfIndex` uses it to refuse a source that does not match
   * the index (a replaced file) with a clear error.
   */
  knownSize?(): number | undefined;
}

/**
 * Total size from a `Content-Range: bytes A-B/<total>` or `bytes *\/<total>`
 * header, if it states one.
 */
function contentRangeTotal(header: string | null): number | undefined {
  const total = header?.split("/")[1]?.trim();
  if (!total || total === "*") return undefined;
  const n = Number(total);
  return Number.isSafeInteger(n) ? n : undefined;
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

/** Default {@link FetchRangeSourceOptions.maxFullDownloadBytes}: 64 MiB. */
const DEFAULT_MAX_FULL_DOWNLOAD_BYTES = 64 * 1024 * 1024;

/** Options for {@link FetchRangeSource}. */
export interface FetchRangeSourceOptions {
  /**
   * Largest file to download whole when the server does not honour range
   * requests (or, in a browser, hides `Content-Range` from the size probe).
   * Default 64 MiB; pass `Infinity` to always allow it.
   */
  maxFullDownloadBytes?: number;
}

/**
 * `RangeSource` backed by HTTP `Range: bytes=start-end` requests via
 * `fetch`.
 *
 * Servers that ignore the `Range` header and return the full body with
 * `200 OK` are handled by downloading the body once and slicing it
 * client-side — but only up to {@link FetchRangeSourceOptions.maxFullDownloadBytes}
 * (64 MiB by default): a larger file is refused with an explanatory error
 * rather than silently pulled into memory whole.
 */
export class FetchRangeSource implements RangeSource {
  private readonly url: string;
  private readonly fetchImpl: FetchLike;
  private readonly maxFullDownloadBytes: number;
  private cachedSize: number | undefined;
  /** Full body cached after a `200` (Range-ignoring) response, served locally. */
  private cachedBody: Uint8Array | undefined;

  constructor(url: string, fetchImpl?: FetchLike, options: FetchRangeSourceOptions = {}) {
    this.url = url;
    this.maxFullDownloadBytes = options.maxFullDownloadBytes ?? DEFAULT_MAX_FULL_DOWNLOAD_BYTES;
    const impl = fetchImpl ?? (globalThis as { fetch?: FetchLike }).fetch;
    if (!impl) {
      throw new Error(
        "No fetch implementation available. Pass one explicitly: " +
          "new FetchRangeSource(url, fetchImpl)",
      );
    }
    // Never call the platform fetch as a method of this object: browsers
    // (and workers) throw "Illegal invocation" when `fetch` runs with a
    // receiver other than the global scope.
    this.fetchImpl = fetchImpl ? (input, init) => impl(input, init) : (input, init) => impl.call(globalThis, input, init);
  }

  knownSize(): number | undefined {
    return this.cachedSize;
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
      // 206 with an unknown total: the body is a 1-byte partial and must not
      // be cached as the full file. Fall back to a plain GET. (In a browser
      // this usually means the server's CORS policy does not expose
      // `Content-Range`: add `Access-Control-Expose-Headers: Content-Range`.)
      const full = await this.fetchImpl(this.url);
      if (!full.ok) {
        throw new Error(`FetchRangeSource: full GET for size failed with status ${full.status}`);
      }
      this.checkFullDownload(full, "the response did not expose its total size (Content-Range)");
      const body = new Uint8Array(await full.arrayBuffer());
      this.cachedBody = body;
      this.cachedSize = body.byteLength;
      return this.cachedSize;
    }
    // Server ignored Range and sent the full body; cache it for later reads.
    this.checkFullDownload(res, "the server ignored the Range header");
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
    // Every ranged response states the resource's total size; remember it
    // (it costs no request) so a mismatch with an index can be reported.
    const total = contentRangeTotal(res.headers.get("content-range"));
    if (total !== undefined) {
      this.cachedSize = total;
    }
    if (res.status === 416) {
      throw new Error(
        `FetchRangeSource: range [${offset}, ${offset + length}) is past the end of the resource` +
          (total !== undefined ? ` (${total} bytes)` : ""),
      );
    }
    if (!res.ok) {
      throw new Error(`FetchRangeSource: request failed with status ${res.status}`);
    }
    if (res.status !== 206) {
      this.checkFullDownload(res, "the server ignored the Range header");
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

  /**
   * Refuse to download a whole resource (a response that is not the
   * requested range) larger than `maxFullDownloadBytes`, judged by its
   * `Content-Length` before the body is read.
   */
  private checkFullDownload(
    res: { headers: { get(name: string): string | null } },
    reason: string,
  ): void {
    const declared = Number(res.headers.get("content-length") ?? NaN);
    if (Number.isFinite(declared) && declared > this.maxFullDownloadBytes) {
      throw new Error(
        `FetchRangeSource: ${reason}, so reading it would download the whole ${declared}-byte ` +
          `file (limit ${this.maxFullDownloadBytes} bytes). Serve it with HTTP range support ` +
          `(and, in a browser, \`Access-Control-Expose-Headers: Content-Range\`), or raise ` +
          `\`maxFullDownloadBytes\` to allow the full download.`,
      );
    }
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

/** `RangeSource` backed by an in-memory buffer (e.g. an already-downloaded file). */
export class BytesRangeSource implements RangeSource {
  private readonly bytes: Uint8Array;

  constructor(bytes: Uint8Array) {
    this.bytes = bytes;
  }

  async size(): Promise<number> {
    return this.bytes.byteLength;
  }

  knownSize(): number | undefined {
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
