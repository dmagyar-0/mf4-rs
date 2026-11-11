import type { ReadWindow } from "./types";

/** `true` when `options` selects a window rather than the whole channel. */
export function hasWindow(options: ReadWindow): boolean {
  return (
    options.startTime !== undefined ||
    options.endTime !== undefined ||
    options.startRecord !== undefined ||
    options.recordCount !== undefined
  );
}

/**
 * Validate the window part of `ReadOptions` and return only its defined
 * fields — the object handed to the wasm `read` / `values` / `readWindowStep`.
 */
export function windowOf(w: ReadWindow): ReadWindow {
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
