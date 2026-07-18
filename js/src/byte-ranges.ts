import type { ByteRange } from "./types";

/**
 * Merge overlapping/adjacent `[offset, length]` ranges into the smallest
 * sorted, non-overlapping set that still covers every input byte.
 */
export function mergeByteRanges(ranges: ByteRange[]): ByteRange[] {
  if (ranges.length === 0) {
    return [];
  }
  const sorted = ranges
    .map(([offset, length]): [number, number] => [offset, offset + length])
    .sort((a, b) => a[0] - b[0]);

  const merged: [number, number][] = [];
  for (const [start, end] of sorted) {
    const last = merged[merged.length - 1];
    if (last && start <= last[1]) {
      last[1] = Math.max(last[1], end);
    } else {
      merged.push([start, end]);
    }
  }
  return merged.map(([start, end]): ByteRange => [start, end - start]);
}
