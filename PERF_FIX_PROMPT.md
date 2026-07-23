# Performance Fix: `buildIndexStep` O(N²) pathology for files with many channels

## Problem

`MdfIndex.fromRangeSource()` is catastrophically slow for MDF files with many channels. A 35 MB file with ~3000 channels takes **1500+ HTTP requests and never completes** when building the index over the network. Even with an in-memory source (zero network latency), a 3 MB file with 3000 channels takes **9+ seconds** vs **25ms** for `fromBytes`.

## Root Cause

The `buildIndexStep` WASM function (called from the JS `fromRangeSource` loop) **restarts the metadata walk from scratch on every call**. The Rust implementation:

1. Reconstructs a `RecordingRangeReader` from all accumulated `ranges`/`fragments`
2. Calls `MdfIndex::from_range_reader()` which calls `reader_walk::walk()`
3. The walk traverses the metadata linked list from the beginning
4. On the first uncovered byte range, `RecordingRangeReader` records the miss and aborts
5. Control returns to JS, which fetches that range and calls `buildIndexStep` again

With N metadata blocks, this is **O(N²)**: step K must re-traverse K-1 blocks before hitting the K-th miss. For ~3000 channels (≈5500 metadata blocks including names/units/groups), this means:
- **~5,500 calls** to `buildIndexStep`
- Each call re-parses all previously fetched fragments from JS→WASM
- Even with zero network latency (in-memory source), takes **9+ seconds** vs **25ms** for `fromBytes`

## Reproducing the Issue

Create a test file with many channel groups using `MdfWriter`:

```typescript
import { MdfWriter } from "../src/mdf-writer";

function buildManyChannelFile(groupCount: number, recordsPerGroup: number): Uint8Array {
  const writer = new MdfWriter();
  writer.initMdfFile();
  writer.setStartTime(1_700_000_000_000_000_000n);

  for (let g = 0; g < groupCount; g++) {
    const groupId = writer.addChannelGroup(`Group_${g}`);
    const timeId = writer.addTimeChannel(groupId, "Time");
    // Add several channels per group to inflate metadata
    for (let c = 0; c < 15; c++) {
      writer.addFloatChannel(groupId, `Signal_${g}_${c}`);
    }
    writer.setTimeChannel(timeId);
    writer.startDataBlock(groupId);
    for (let i = 0; i < recordsPerGroup; i++) {
      const values = [i * 0.01, ...Array(15).fill(Math.sin(i * 0.1))];
      writer.writeRecord(groupId, values);
    }
    writer.finishDataBlock(groupId);
  }

  return writer.finalize();
}

// ~185 groups × 16 channels = ~2960 channels, representative of production MOTION files
const bytes = buildManyChannelFile(185, 1000);
```

Then benchmark:

```typescript
import { MdfIndex } from "../src/mdf-index";
import { BytesRangeSource } from "../src/range-source";

// Baseline — direct parsing (fast)
const t0 = Date.now();
const indexDirect = MdfIndex.fromBytes(bytes);
console.log(`fromBytes: ${Date.now() - t0}ms`);

// Problem case — range-source loop (slow due to O(N²))
let readCount = 0;
const source: RangeSource = {
  size: async () => bytes.length,
  read: async (offset, length) => {
    readCount++;
    return new Uint8Array(bytes.buffer, offset, Math.min(length, bytes.length - offset));
  },
};

const t1 = Date.now();
const indexLazy = await MdfIndex.fromRangeSource(source);
console.log(`fromRangeSource: ${Date.now() - t1}ms, reads: ${readCount}`);
```

Expected output showing the problem:
```
fromBytes: ~30ms
fromRangeSource: ~9000ms, reads: ~5500
```

## Desired Fix

Make `buildIndexStep` (or a replacement) process **all available data in a single call** rather than stopping at the first miss and returning.

### Option A: Make the walk resumable (preferred)

Change `RecordingRangeReader` to **not abort on the first miss**. Instead:
1. On a miss, record it but **continue the walk** using a sentinel/skip for the missing data
2. After the walk completes (or hits a structural dependency on missing data), return **all** missed ranges at once
3. The JS fetches all needed ranges in parallel, then calls `buildIndexStep` once more

This changes the JS return type from `{ done, needed: [offset, length] }` to `{ done, needed: [[offset, length], ...] }` (array of all needed ranges).

### Option B: Collect all misses in a single pass

Change `RecordingRangeReader` to collect **every** missed range without aborting. Since the metadata is a linked list, the walk can't skip structural dependencies (the next block's address comes from the current block). However, for each block that IS reachable, record ALL sub-reads that miss. Then the walk naturally terminates at the first structurally-blocking miss, but returns all misses encountered up to that point.

For example, when walking channel blocks, the walk reads:
- `cn_bytes` (160 bytes at `ch_addr`) — structural, must succeed to continue
- `name` (text block at `cn.name_addr`) — non-structural, could skip
- `unit` (text block at `cn.unit_addr`) — non-structural, could skip

If the channel block itself is available but name/unit are not, record those misses and continue to the next channel. This reduces round-trips from O(N) to O(depth of structural chain).

### Option C: Minimal JS-side workaround (already implemented as interim fix)

In `fromRangeSource`, after the first miss, batch-fetch the entire region from `BUILD_SEED_BYTES` to the missed offset in one request. This works for files where metadata is contiguous near the front (the common case for real MDF files from loggers). Already implemented — reduces the 35 MB production file from 1500+ requests to ~21 requests.

**However**, this doesn't fix the O(N²) CPU cost of the repeated JS↔WASM round-trips and fragment re-parsing, which dominates for in-memory sources.

## Key Files

- `src/wasm.rs` — `WasmMdfIndex::build_index_step()`, `RecordingRangeReader`, `assemble_from_fragments()`
- `src/parsing/reader_walk.rs` — `walk()` function (the linear metadata traversal)
- `src/index.rs` — `MdfIndex::from_range_reader()`
- `js/src/mdf-index.ts` — `MdfIndex.fromRangeSource()` (the JS loop calling `buildIndexStep`)

## Acceptance Criteria

- `fromRangeSource` on a ~3000-channel file completes in **< 500ms** with an in-memory `BytesRangeSource` (currently 9+ seconds)
- Total `source.read()` calls for that file should be **< 50** (currently ~5,500)
- The existing `js/test/lazy-index.test.ts` continues to pass (< 15% of file fetched for its test file)
- No regression in `fromBytes` performance
- The fix should work for files where metadata is scattered throughout (not just contiguous at the front)
