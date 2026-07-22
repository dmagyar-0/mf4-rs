import { test } from "node:test";
import assert from "node:assert/strict";

import { MdfIndex } from "../src/mdf-index";
import { BytesRangeSource, type RangeSource } from "../src/range-source";
import { buildSampleFile } from "./helpers";

/** A `RangeSource` decorator that tallies how many bytes/requests flow through. */
class CountingSource implements RangeSource {
  bytesRead = 0;
  reads = 0;
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

test("MdfIndex.fromRangeSource fetches only metadata, not the bulk data", async () => {
  // ~7.2 MB of sample data (300k records x 24 bytes), split across multiple
  // ##DT fragments — far larger than the metadata the build needs.
  const { bytes, values } = buildSampleFile(300_000);
  assert.ok(bytes.length > 5_000_000, `expected a large file, got ${bytes.length} bytes`);

  const counter = new CountingSource(new BytesRangeSource(bytes));
  const index = await MdfIndex.fromRangeSource(counter);

  // The whole point: only a small slice of the file is transferred to build
  // the index (metadata blocks + a fixed seed prefix), never the samples.
  assert.ok(
    counter.bytesRead < bytes.length * 0.15,
    `fetched ${counter.bytesRead} of ${bytes.length} bytes (${(
      (counter.bytesRead / bytes.length) *
      100
    ).toFixed(1)}%) — expected < 15%`,
  );

  // The index is correct: reading a channel (over the full source) matches.
  const read = await index.values("Value", new BytesRangeSource(bytes));
  assert.deepEqual(Array.from(read), values);

  // And it agrees with a full-bytes build.
  const eager = MdfIndex.fromBytes(bytes);
  assert.deepEqual(index.channelNames(), eager.channelNames());
  assert.equal(index.fileSize(), bytes.length);
  index.dispose();
  eager.dispose();
});

test("MdfIndex.fromRangeSource handles files smaller than the seed prefix", async () => {
  const { bytes, values } = buildSampleFile(10);
  const index = await MdfIndex.fromRangeSource(new BytesRangeSource(bytes));
  const read = await index.values("Value", new BytesRangeSource(bytes));
  assert.deepEqual(Array.from(read), values);
  index.dispose();
});
