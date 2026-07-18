import { test } from "node:test";
import assert from "node:assert/strict";

import { Mdf } from "../src/mdf";
import { MdfIndex } from "../src/mdf-index";
import { MdfWriter } from "../src/mdf-writer";
import { BytesRangeSource } from "../src/range-source";

/**
 * VLSD (variable-length string) contract for the lazy index path.
 *
 * Index-based reads of VLSD channels are not yet supported: they must fail
 * with a clear error — and that failure must not affect reads of the
 * fixed-width channels in the same group, which must keep working through
 * the index path (validated against the direct reader).
 */

function buildVlsdFile(): { bytes: Uint8Array; strs: string[] } {
  const writer = new MdfWriter();
  writer.initMdfFile();
  const groupId = writer.addChannelGroup("VlsdGroup");
  writer.addTimeChannel(groupId, "Time");
  writer.addFloatChannel(groupId, "Value");
  writer.addStringChannel(groupId, "Label");

  const strs = ["", "a", "hello world", "unicode: äöü漢字", "0123456789 with spaces"];
  writer.startDataBlock(groupId);
  strs.forEach((s, i) => {
    writer.writeRecord(groupId, [i * 0.1, i * 1.5, s]);
  });
  writer.finishDataBlock(groupId);
  return { bytes: writer.finalize(), strs };
}

test("VLSD channel reads correctly via the direct reader", () => {
  const { bytes, strs } = buildVlsdFile();
  const mdf = Mdf.fromBytes(bytes);
  const sig = mdf.read("Label");
  assert.deepEqual(sig.values, strs);
  assert.equal(sig.timestamps.length, strs.length);
  mdf.dispose();
});

test("VLSD channel via the index path fails with a clear error", async () => {
  const { bytes } = buildVlsdFile();
  const idx = MdfIndex.fromJson(MdfIndex.fromBytes(bytes).toJson());
  const source = new BytesRangeSource(bytes);

  await assert.rejects(
    () => idx.read("Label", source),
    (e: Error) => /VLSD channel 'Label' not yet supported/.test(e.message),
  );
  await assert.rejects(
    () => idx.values("Label", source),
    (e: Error) => /VLSD channel 'Label' not yet supported/.test(e.message),
  );
  idx.dispose();
});

test("a failed VLSD index read does not poison other channels in the group", async () => {
  const { bytes } = buildVlsdFile();
  const mdf = Mdf.fromBytes(bytes);
  const idx = MdfIndex.fromJson(MdfIndex.fromBytes(bytes).toJson());
  const source = new BytesRangeSource(bytes);

  await assert.rejects(() => idx.read("Label", source));

  // Fixed-width channel in the same (VLSD-containing) group still reads via
  // the index path, and matches the direct reader.
  const lazy = await idx.read("Value", source);
  const direct = mdf.read("Value");
  assert.deepEqual(Array.from(lazy.values as number[]), Array.from(direct.values as number[]));
  assert.deepEqual(Array.from(lazy.timestamps), Array.from(direct.timestamps));

  const lazyVals = await idx.values("Value", source);
  assert.deepEqual(Array.from(lazyVals), Array.from(mdf.values("Value")));

  idx.dispose();
  mdf.dispose();
});
