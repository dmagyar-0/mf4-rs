import { test } from "node:test";
import assert from "node:assert/strict";

// Register the Node wasm loader (the `.` package entry does this for real
// consumers; tests import the wrapper classes from source directly).
import "../src/wasm-module-node";
import { MdfIndex } from "../src/mdf-index";
import { MdfWriter } from "../src/mdf-writer";
import { BytesRangeSource, type RangeSource } from "../src/range-source";

/**
 * Build a file whose metadata clusters are separated by **large** data blocks.
 *
 * This is the shape a per-message bus logger produces (each channel group's
 * data written before the next group's metadata) and it is the case the
 * round-number-based look-ahead handled badly: the window ratcheted up to its
 * ceiling and every subsequent small metadata cluster was fetched with a
 * multi-megabyte request that dragged the surrounding data blocks along with
 * it. `lazy-index-many-channel.test.ts` covers the opposite shape — data blocks
 * small enough that reading straight through them is the right call.
 *
 * 16 channels x 8 bytes = 128 B per record, so `recordsPerGroup` records give a
 * data block of `recordsPerGroup * 128` bytes between consecutive clusters.
 */
function buildWidelySpacedFile(groupCount: number, recordsPerGroup: number): Uint8Array {
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
    const row = Array(15).fill(1.5);
    for (let i = 0; i < recordsPerGroup; i++) {
      writer.writeRecord(groupId, [i * 0.01, ...row]);
    }
    writer.finishDataBlock(groupId);
  }

  return writer.finalize();
}

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

const GROUPS = 20;
const RECORDS = 4000; // 512 KiB data block between clusters

test("MdfIndex.fromRangeSource does not drag in data blocks between metadata clusters", async () => {
  const bytes = buildWidelySpacedFile(GROUPS, RECORDS);

  const counter = new CountingSource(new BytesRangeSource(bytes));
  const index = await MdfIndex.fromRangeSource(counter);

  // The acceptance criterion: metadata is a small fraction of this file, so the
  // build must transfer a small fraction of it. Before the read-ahead window
  // reset on a seek, this fetched essentially the whole file.
  assert.ok(
    counter.bytesRead < bytes.length * 0.3,
    `expected < 30% of ${bytes.length} bytes, got ${counter.bytesRead} ` +
      `(${((100 * counter.bytesRead) / bytes.length).toFixed(1)}%)`,
  );

  // Correctness is not traded away for the smaller transfer.
  const eager = MdfIndex.fromBytes(bytes);
  assert.deepEqual(index.channelNames(), eager.channelNames());
  assert.equal(index.channelNames().length, GROUPS * 16);
  assert.equal(index.fileSize(), bytes.length);

  index.dispose();
  eager.dispose();
});

test("a smaller lookaheadBytes transfers strictly fewer bytes", async () => {
  const bytes = buildWidelySpacedFile(GROUPS, RECORDS);

  const wide = new CountingSource(new BytesRangeSource(bytes));
  const wideIndex = await MdfIndex.fromRangeSource(wide, { lookaheadBytes: 64 * 1024 });

  const narrow = new CountingSource(new BytesRangeSource(bytes));
  const narrowIndex = await MdfIndex.fromRangeSource(narrow, {
    seedBytes: 32 * 1024,
    lookaheadBytes: 16 * 1024,
  });

  assert.ok(
    narrow.bytesRead < wide.bytesRead,
    `expected the narrower window to transfer less: narrow=${narrow.bytesRead} wide=${wide.bytesRead}`,
  );
  // Both must still produce the same index.
  assert.deepEqual(narrowIndex.channelNames(), wideIndex.channelNames());

  wideIndex.dispose();
  narrowIndex.dispose();
});

test("invalid build options are rejected", async () => {
  const bytes = buildWidelySpacedFile(2, 10);
  const source = new BytesRangeSource(bytes);

  await assert.rejects(
    () => MdfIndex.fromRangeSource(source, { seedBytes: 0 }),
    /seedBytes must be a positive integer/,
  );
  await assert.rejects(
    () => MdfIndex.fromRangeSource(source, { lookaheadBytes: -1 }),
    /lookaheadBytes must be a positive integer/,
  );
  await assert.rejects(
    () => MdfIndex.fromRangeSource(source, { bridgeBytes: 1.5 }),
    /bridgeBytes must be a positive integer/,
  );

  // A ceiling below the base is raised to the base rather than silently
  // clamping every window down to it.
  const index = await MdfIndex.fromRangeSource(source, {
    lookaheadBytes: 64 * 1024,
    maxLookaheadBytes: 1024,
  });
  assert.equal(index.channelNames().length, 2 * 16);
  index.dispose();
});
