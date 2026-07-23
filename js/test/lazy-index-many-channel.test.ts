import { test } from "node:test";
import assert from "node:assert/strict";

// Register the Node wasm loader (the `.` package entry does this for real
// consumers; tests import the wrapper classes from source directly).
import "../src/wasm-module-node";
import { MdfIndex } from "../src/mdf-index";
import { MdfWriter } from "../src/mdf-writer";
import { BytesRangeSource, type RangeSource } from "../src/range-source";

/**
 * Build a file with many channel groups, each followed by its own data block —
 * so the metadata is scattered throughout the file (interleaved with data)
 * rather than sitting contiguously at the front. This is the layout that used
 * to make `buildIndexStep` O(N²): the walk restarted from scratch and aborted
 * on the first missing range, needing one round-trip per metadata block.
 */
function buildManyChannelFile(groupCount: number, recordsPerGroup: number): Uint8Array {
  const writer = new MdfWriter();
  writer.initMdfFile();
  writer.setStartTime(1_700_000_000_000_000_000n);

  for (let g = 0; g < groupCount; g++) {
    const groupId = writer.addChannelGroup(`Group_${g}`);
    const timeId = writer.addTimeChannel(groupId, "Time");
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

/** A `RangeSource` that tallies how many read requests flow through. */
class CountingSource implements RangeSource {
  reads = 0;
  bytesRead = 0;
  constructor(private readonly inner: RangeSource) {}
  size(): Promise<number> {
    return this.inner.size!();
  }
  async read(offset: number, length: number): Promise<Uint8Array> {
    this.reads++;
    const bytes = await this.inner.read(offset, length);
    this.bytesRead += bytes.length;
    return bytes;
  }
}

test("MdfIndex.fromRangeSource builds a many-channel file in few requests", async () => {
  // ~185 groups x 16 channels = ~2960 channels, with metadata interleaved with
  // data throughout the file.
  const bytes = buildManyChannelFile(185, 200);

  const counter = new CountingSource(new BytesRangeSource(bytes));
  const t0 = Date.now();
  const index = await MdfIndex.fromRangeSource(counter);
  const elapsed = Date.now() - t0;

  // The fix: a bounded, small number of requests instead of one per metadata
  // block (~5500 before). This is the core acceptance criterion.
  assert.ok(
    counter.reads < 50,
    `expected < 50 reads, got ${counter.reads} (elapsed ${elapsed}ms)`,
  );

  // The incrementally built index must be identical to a direct build.
  const eager = MdfIndex.fromBytes(bytes);
  assert.deepEqual(index.channelNames(), eager.channelNames());
  assert.equal(index.channelNames().length, 185 * 16);
  assert.equal(index.fileSize(), bytes.length);

  // And a real read still works end to end.
  const values = await index.values("Signal_0_0", new BytesRangeSource(bytes), "Group_0");
  assert.equal(values.length, 200);

  index.dispose();
  eager.dispose();
});
