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
  body?: {
    getReader?(): { read(): Promise<{ done: boolean; value?: Uint8Array }>; cancel(): Promise<void> };
    cancel?(): Promise<void>;
  } | null;
};

/**
 * A response body as bytes: streamed chunk by chunk when the response has a
 * readable body (`touch` is called for every chunk, restarting a stall
 * timer; `check` sees the running total and may throw to stop), else via
 * `arrayBuffer()`. Every wait goes through `guard`, which rejects on a stall
 * even when the `fetch` implementation ignores its abort signal.
 */
async function readBody(
  res: FullBodyResponse,
  touch: () => void,
  guard: <T>(pending: Promise<T>) => Promise<T>,
  check: (total: number) => void = () => {},
): Promise<Uint8Array> {
  const reader = res.body?.getReader?.();
  if (!reader) {
    const buf = new Uint8Array(await guard(res.arrayBuffer()));
    check(buf.byteLength);
    return buf;
  }
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    let next: { done: boolean; value?: Uint8Array };
    try {
      next = await guard(reader.read());
    } catch (e) {
      void reader.cancel().catch(() => undefined); // never awaited: it could hang
      throw e;
    }
    const { done, value } = next;
    if (done || !value) break;
    touch();
    total += value.byteLength;
    try {
      check(total);
    } catch (e) {
      void reader.cancel().catch(() => undefined); // never awaited: it could hang
      throw e;
    }
    chunks.push(value);
  }
  if (chunks.length === 1) return chunks[0]!;
  const out = new Uint8Array(total);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.byteLength;
  }
  return out;
}

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
   * Abort (and retry) a request that sends nothing for this many
   * milliseconds — no response headers, or no next body chunk. A slow but
   * steady download is not limited. Default 60 000; `0` disables it.
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
    for (const [name, value] of [["retryDelayMs", this.retryDelayMs], ["timeoutMs", this.timeoutMs]] as const) {
      // Timers clamp delays above 2^31 - 1 ms to ~1 ms.
      if (!Number.isFinite(value) || value < 0 || value > 2_147_483_647) {
        throw new RangeError(`FetchRangeSource: ${name} must be between 0 and 2147483647 ms, got ${value}`);
      }
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
   * `fetch` the URL and hand the response to `consume`, retrying transient
   * failures (see {@link FetchRangeSourceOptions.retries}): a transient
   * status (the response is cancelled before retrying), a network error
   * (`TypeError`, also thrown when a body is cut off) or a stall longer than
   * `timeoutMs` — waiting for the headers or between body chunks
   * (`consume` calls `touch` for every chunk it reads and wraps every wait
   * in `guard`, so a stall is detected even when the `fetch` implementation
   * ignores the abort signal). Other errors,
   * including those `consume` throws, are final. A response that is still
   * transient after the last retry is passed to `consume` for its status
   * handling.
   */
  private async request<T>(
    headers: Record<string, string> | undefined,
    consume: (
      res: FullBodyResponse,
      touch: () => void,
      guard: <U>(pending: Promise<U>) => Promise<U>,
    ) => Promise<T>,
  ): Promise<T> {
    let delay = this.retryDelayMs;
    for (let attempt = 0; ; attempt++) {
      const last = attempt >= this.retries;
      const controller =
        this.timeoutMs > 0 && typeof AbortController !== "undefined" ? new AbortController() : undefined;
      let timer: ReturnType<typeof setTimeout> | undefined;
      let stalled = false;
      let onStall: (reason: Error) => void = () => {};
      const stall = new Promise<never>((_, reject) => {
        onStall = reject;
      });
      stall.catch(() => undefined); // only observed through `guard`
      const guard = <U>(pending: Promise<U>): Promise<U> =>
        this.timeoutMs > 0 ? Promise.race([pending, stall]) : pending;
      const touch = (): void => {
        if (this.timeoutMs <= 0) return;
        clearTimeout(timer);
        timer = setTimeout(() => {
          stalled = true;
          controller?.abort();
          onStall(new Error("stalled"));
        }, this.timeoutMs);
      };
      touch();
      try {
        const init: { headers?: Record<string, string>; signal?: AbortSignal } = {};
        if (headers) init.headers = headers;
        if (controller) init.signal = controller.signal;
        const res: FullBodyResponse = await guard(
          this.fetchImpl(this.url, Object.keys(init).length > 0 ? init : undefined),
        );
        if (TRANSIENT_STATUSES.has(res.status) && !last) {
          clearTimeout(timer);
          void res.body?.cancel?.().catch(() => undefined); // never awaited: it could hang
          await sleep(Math.min(retryAfterMs(res.headers.get("retry-after")) ?? delay, MAX_RETRY_DELAY_MS));
          delay *= 2;
          continue;
        }
        touch();
        return await consume(res, touch, guard);
      } catch (e) {
        const transient = stalled || e instanceof TypeError;
        if (!transient || last) {
          if (stalled) {
            throw new Error(`FetchRangeSource: ${this.url} sent nothing for ${this.timeoutMs} ms`, { cause: e });
          }
          throw e;
        }
        await sleep(delay);
        delay *= 2;
      } finally {
        clearTimeout(timer);
      }
    }
  }

  async size(): Promise<number> {
    if (this.cachedSize !== undefined) {
      return this.cachedSize;
    }
    const known = await this.request({ Range: "bytes=0-0" }, async (res, touch, guard) => {
      if (!res.ok) {
        throw new Error(`FetchRangeSource: HEAD-equivalent request failed with status ${res.status}`);
      }
      if (res.status === 206) {
        const contentRange = res.headers.get("content-range");
        if (contentRange) {
          const total = contentRange.split("/")[1];
          if (total && total !== "*") {
            void res.body?.cancel?.().catch(() => undefined); // never awaited: it could hang
            return Number(total);
          }
        }
        // 206 with an unknown total: the body is a 1-byte partial and must not
        // be cached as the full file. Fall back to a plain GET (below). (In a
        // browser this usually means the server's CORS policy does not expose
        // `Content-Range`: add `Access-Control-Expose-Headers: Content-Range`.)
        void res.body?.cancel?.().catch(() => undefined); // never awaited: it could hang
        return undefined;
      }
      // Server ignored Range and sent the full body; cache it for later reads.
      this.cachedBody = await this.readFullBody(res, "the server ignored the Range header", touch, guard);
      return this.cachedBody.byteLength;
    });
    if (known !== undefined) {
      this.cachedSize = known;
      return known;
    }
    const body = await this.request(undefined, async (full, touch, guard) => {
      if (!full.ok) {
        throw new Error(`FetchRangeSource: full GET for size failed with status ${full.status}`);
      }
      return this.readFullBody(full, "the response did not expose its total size (Content-Range)", touch, guard);
    });
    this.cachedBody = body;
    this.cachedSize = body.byteLength;
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
    const result = await this.request({ Range: `bytes=${offset}-${end}` }, async (res, touch, guard) => {
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
        // Server returned 200 with the full body (ignored Range).
        return { full: await this.readFullBody(res, "the server ignored the Range header", touch, guard) };
      }
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
      // Stop reading as soon as the body is longer than the range asked for.
      const tooLong = new Error(`FetchRangeSource: 206 response body is longer than the ${length} bytes requested at offset ${offset}`);
      const buf = await readBody(res, touch, guard, (total) => {
        if (total > length) throw tooLong;
      });
      if (buf.byteLength !== length) {
        throw new Error(
          `FetchRangeSource: 206 response body is ${buf.byteLength} bytes but ${length} were ` +
            `requested at offset ${offset}`,
        );
      }
      return { partial: buf };
    });
    if (result.partial) return result.partial;

    this.cachedBody = result.full!;
    if (this.cachedSize === undefined) {
      this.cachedSize = this.cachedBody.byteLength;
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
  private async readFullBody(
    res: FullBodyResponse,
    reason: string,
    touch: () => void,
    guard: <U>(pending: Promise<U>) => Promise<U>,
  ): Promise<Uint8Array> {
    this.checkFullDownload(res, reason);
    return readBody(res, touch, guard, (total) => {
      if (total > this.maxFullDownloadBytes) this.refuseFullDownload(total, reason, "more than ");
    });
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
