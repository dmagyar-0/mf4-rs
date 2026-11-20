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

/** A response as `FetchRangeSource` reads it: the `FetchLike` result plus an optional streaming body. */
type FullBodyResponse = Awaited<ReturnType<FetchLike>> & {
  body?: { getReader?(): { read(): Promise<{ done: boolean; value?: Uint8Array }>; cancel(): Promise<void> } } | null;
};

/** Default {@link FetchRangeSourceOptions.maxFullDownloadBytes}: 64 MiB. */
const DEFAULT_MAX_FULL_DOWNLOAD_BYTES = 64 * 1024 * 1024;
/** Default {@link FetchRangeSourceOptions.retries}. */
const DEFAULT_RETRIES = 3;
/** Default {@link FetchRangeSourceOptions.retryDelayMs}. */
const DEFAULT_RETRY_DELAY_MS = 200;
/** Default {@link FetchRangeSourceOptions.timeoutMs}. */
const DEFAULT_TIMEOUT_MS = 60_000;
/** Upper bound on one retry delay, including a server's `Retry-After`. */
const MAX_RETRY_DELAY_MS = 10_000;
/** Statuses that signal a temporary condition: request timeout, throttling
 * (S3 `503 SlowDown`, `429`) and server/gateway errors. */
const TRANSIENT_STATUSES = new Set([408, 429, 500, 502, 503, 504]);

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

/** A `Retry-After: <seconds>` header in milliseconds (the HTTP-date form is ignored). */
function retryAfterMs(header: string | null): number | undefined {
  if (header === null || !/^\s*\d+\s*$/.test(header)) return undefined;
  return Number(header) * 1000;
}

/** Options for {@link FetchRangeSource}. */
export interface FetchRangeSourceOptions {
  /**
   * Largest file to download whole when the server does not honour range
   * requests (or, in a browser, hides `Content-Range` from the size probe).
   * Checked against `Content-Length` before the body is read and, for a
   * streamed body, while reading it. Default 64 MiB; pass `Infinity` to
   * always allow it.
   */
  maxFullDownloadBytes?: number;
  /**
   * Retries of a request that failed transiently — a network error, a
   * timeout, or status `408` / `429` / `500` / `502` / `503` / `504` — with
   * exponential backoff (or the server's `Retry-After`, capped at 10 s).
   * Default 3; `0` disables retrying.
   */
  retries?: number;
  /** Delay before the first retry, doubled for each further one. Default 200 ms. */
  retryDelayMs?: number;
  /**
   * Abort (and retry) a request whose response headers have not arrived
   * after this many milliseconds. Downloading the body is not limited.
   * Default 60 000; `0` disables the timeout.
   */
  timeoutMs?: number;
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
  private readonly retries: number;
  private readonly retryDelayMs: number;
  private readonly timeoutMs: number;
  private cachedSize: number | undefined;
  /** Full body cached after a `200` (Range-ignoring) response, served locally. */
  private cachedBody: Uint8Array | undefined;

  constructor(url: string, fetchImpl?: FetchLike, options: FetchRangeSourceOptions = {}) {
    this.url = url;
    this.maxFullDownloadBytes = options.maxFullDownloadBytes ?? DEFAULT_MAX_FULL_DOWNLOAD_BYTES;
    this.retries = options.retries ?? DEFAULT_RETRIES;
    this.retryDelayMs = options.retryDelayMs ?? DEFAULT_RETRY_DELAY_MS;
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    if (!Number.isInteger(this.retries) || this.retries < 0) {
      throw new RangeError(`FetchRangeSource: retries must be a non-negative integer, got ${this.retries}`);
    }
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

  /**
   * `fetch` the URL, retrying transient failures (see
   * {@link FetchRangeSourceOptions.retries}). A response that is still
   * transient after the last retry is returned as is, for the caller's
   * status handling.
   */
  private async request(headers?: Record<string, string>): Promise<FullBodyResponse> {
    let delay = this.retryDelayMs;
    for (let attempt = 0; ; attempt++) {
      let res: FullBodyResponse;
      try {
        res = await this.fetchOnce(headers);
      } catch (e) {
        if (attempt >= this.retries) throw e;
        await sleep(delay);
        delay *= 2;
        continue;
      }
      if (!TRANSIENT_STATUSES.has(res.status) || attempt >= this.retries) return res;
      await sleep(Math.min(retryAfterMs(res.headers.get("retry-after")) ?? delay, MAX_RETRY_DELAY_MS));
      delay *= 2;
    }
  }

  /** One `fetch`, aborted if its response headers take longer than `timeoutMs`. */
  private async fetchOnce(headers?: Record<string, string>): Promise<FullBodyResponse> {
    const init = headers ? { headers } : undefined;
    if (this.timeoutMs <= 0 || typeof AbortController === "undefined") {
      return this.fetchImpl(this.url, init);
    }
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      return await this.fetchImpl(this.url, { ...init, signal: controller.signal });
    } catch (e) {
      if (controller.signal.aborted) {
        throw new Error(`FetchRangeSource: no response from ${this.url} within ${this.timeoutMs} ms`, { cause: e });
      }
      throw e;
    } finally {
      clearTimeout(timer);
    }
  }

  async size(): Promise<number> {
    if (this.cachedSize !== undefined) {
      return this.cachedSize;
    }
    const res = await this.request({ Range: "bytes=0-0" });
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
      const full = await this.request();
      if (!full.ok) {
        throw new Error(`FetchRangeSource: full GET for size failed with status ${full.status}`);
      }
      const body = await this.readFullBody(full, "the response did not expose its total size (Content-Range)");
      this.cachedBody = body;
      this.cachedSize = body.byteLength;
      return this.cachedSize;
    }
    // Server ignored Range and sent the full body; cache it for later reads.
    const buf = await this.readFullBody(res, "the server ignored the Range header");
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
    const res = await this.request({ Range: `bytes=${offset}-${end}` });
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
    const buf =
      res.status === 206
        ? new Uint8Array(await res.arrayBuffer())
        : await this.readFullBody(res, "the server ignored the Range header");

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
   * Read a whole-resource body (a response that is not the requested range),
   * refusing one larger than `maxFullDownloadBytes`: up front from its
   * `Content-Length`, and — when the body is a stream — while reading it, so
   * a chunked or compressed response without an honest length is cut off
   * too.
   */
  private async readFullBody(res: FullBodyResponse, reason: string): Promise<Uint8Array> {
    this.checkFullDownload(res, reason);
    const reader = res.body?.getReader?.();
    if (!reader) {
      const buf = new Uint8Array(await res.arrayBuffer());
      this.refuseFullDownload(buf.byteLength, reason);
      return buf;
    }
    const chunks: Uint8Array[] = [];
    let total = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done || !value) break;
      total += value.byteLength;
      if (total > this.maxFullDownloadBytes) {
        await reader.cancel().catch(() => undefined);
        this.refuseFullDownload(total, reason, "more than ");
      }
      chunks.push(value);
    }
    const out = new Uint8Array(total);
    let at = 0;
    for (const chunk of chunks) {
      out.set(chunk, at);
      at += chunk.byteLength;
    }
    return out;
  }

  /** Throw the full-download refusal if `bytes` exceeds the limit. */
  private refuseFullDownload(bytes: number, reason: string, qualifier = ""): void {
    if (bytes > this.maxFullDownloadBytes) {
      throw new Error(
        `FetchRangeSource: ${reason}, so reading it would download the whole file ` +
          `(${qualifier}${bytes} bytes; limit ${this.maxFullDownloadBytes}). Serve it with HTTP range support ` +
          `(and, in a browser, \`Access-Control-Expose-Headers: Content-Range\`), or raise ` +
          `\`maxFullDownloadBytes\` to allow the full download.`,
      );
    }
  }

  /**
   * Refuse a whole-resource response whose `Content-Length` already exceeds
   * `maxFullDownloadBytes`, before reading its body.
   */
  private checkFullDownload(
    res: { headers: { get(name: string): string | null } },
    reason: string,
  ): void {
    const declared = Number(res.headers.get("content-length") ?? NaN);
    if (Number.isFinite(declared)) {
      this.refuseFullDownload(declared, reason);
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
