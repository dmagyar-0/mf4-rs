import type { ChannelValue, ReadOptions, ReadWindow, Signal, TimeRange } from "./types";

/** `true` when `options` selects a window rather than the whole channel. */
export function hasWindow(options: ReadWindow): boolean {
  return (
    options.startTime !== undefined ||
    options.endTime !== undefined ||
    options.startRecord !== undefined ||
    options.recordCount !== undefined
  );
}

/** Validate a window and return only its defined fields. */
export function checkWindow(w: ReadWindow): ReadWindow {
  const byTime = w.startTime !== undefined || w.endTime !== undefined;
  const byRecord = w.startRecord !== undefined || w.recordCount !== undefined;
  if (byTime && byRecord) {
    throw new Error("Pass either startTime/endTime or startRecord/recordCount, not both.");
  }
  for (const key of ["startTime", "endTime"] as const) {
    const v = w[key];
    if (v !== undefined && (typeof v !== "number" || Number.isNaN(v))) {
      throw new RangeError(`${key} must be a number, got ${v}`);
    }
  }
  for (const key of ["startRecord", "recordCount"] as const) {
    const v = w[key];
    if (v !== undefined && (!Number.isSafeInteger(v) || v < 0)) {
      throw new RangeError(`${key} must be a non-negative integer, got ${v}`);
    }
  }
  const out: ReadWindow = {};
  if (w.startTime !== undefined) out.startTime = w.startTime;
  if (w.endTime !== undefined) out.endTime = w.endTime;
  if (w.startRecord !== undefined) out.startRecord = w.startRecord;
  if (w.recordCount !== undefined) out.recordCount = w.recordCount;
  return out;
}

/** Numeric view of decoded values: numbers kept, bigints converted, anything
 * else (text, bytes, invalid) becomes `NaN` — the `values()` semantics. */
export function toFloat64(values: ChannelValue[]): Float64Array {
  const out = new Float64Array(values.length);
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    out[i] = typeof v === "number" ? v : typeof v === "bigint" ? Number(v) : NaN;
  }
  return out;
}

/** First index in sorted `xs` for which `below(x)` is false. */
function partitionPoint(xs: ArrayLike<number>, below: (x: number) => boolean): number {
  let lo = 0;
  let hi = xs.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (below(xs[mid]!)) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * Cut an in-memory `Signal` to a window. A time window needs `timeAxis` — the
 * signal's timestamps, or for the master itself its own values.
 */
export function sliceSignal(signal: Signal, window: ReadWindow, timeAxis: ArrayLike<number>): Signal {
  let start: number;
  let end: number;
  if (window.startTime !== undefined || window.endTime !== undefined) {
    if (timeAxis.length === 0) {
      throw new Error(`Channel '${signal.name}' has no master (time) channel; cannot select by time.`);
    }
    const t0 = window.startTime;
    const t1 = window.endTime;
    start = t0 === undefined ? 0 : partitionPoint(timeAxis, (t) => t < t0);
    end = t1 === undefined ? timeAxis.length : partitionPoint(timeAxis, (t) => t <= t1);
    end = Math.max(start, end);
  } else {
    const total = signal.values.length;
    start = window.startRecord ?? 0;
    const count = window.recordCount ?? Math.max(0, total - start);
    end = start + count;
    if (end > total) {
      throw new RangeError(
        `record window starting at ${start} for ${count} records exceeds the group's ${total} records`,
      );
    }
  }
  return {
    name: signal.name,
    unit: signal.unit,
    timestamps: signal.timestamps.length ? signal.timestamps.slice(start, end) : signal.timestamps,
    values: signal.values.slice(start, end),
  };
}

/** Time axis of a signal: its timestamps, or its own values for a master. */
export function timeAxisOf(signal: Signal): ArrayLike<number> {
  return signal.timestamps.length ? signal.timestamps : toFloat64(signal.values);
}

/** First/last entry of a time axis, or `undefined` when empty. */
export function rangeOf(axis: ArrayLike<number>): TimeRange | undefined {
  return axis.length ? { start: axis[0]!, end: axis[axis.length - 1]! } : undefined;
}

/**
 * Convert relative master times (seconds) to absolute nanoseconds since the
 * Unix epoch: `startTimeNs + round(t * 1e9)`.
 *
 * MDF stores the time master as values relative to the measurement start
 * (`startTimeNs()`), usually float64 seconds. Relative seconds are precise in a
 * `number`; absolute epoch nanoseconds are not (they exceed 2^53), hence the
 * `BigInt64Array`. Non-finite times become `0n`.
 */
export function toAbsoluteNs(timestamps: ArrayLike<number>, startTimeNs: bigint): BigInt64Array {
  const out = new BigInt64Array(timestamps.length);
  for (let i = 0; i < timestamps.length; i++) {
    const t = timestamps[i]!;
    out[i] = Number.isFinite(t) ? startTimeNs + BigInt(Math.round(t * 1e9)) : 0n;
  }
  return out;
}

/** Split a `values`/`read` argument into a group name and options. */
export function normaliseOptions(
  groupOrOptions: string | null | undefined | ReadOptions,
): ReadOptions {
  if (groupOrOptions === null || groupOrOptions === undefined) return {};
  if (typeof groupOrOptions === "string") return { group: groupOrOptions };
  return groupOrOptions;
}
