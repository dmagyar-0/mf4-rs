import { test } from "node:test";
import assert from "node:assert/strict";

// Register the Node wasm loader (the `.` package entry does this for real
// consumers; tests import the wrapper classes from source directly).
import "../src/wasm-module-node";
import { Mdf } from "../src/mdf";
import { MdfIndex } from "../src/mdf-index";
import { MdfWriter } from "../src/mdf-writer";
import { BytesRangeSource } from "../src/range-source";

/**
 * VLSD (variable-length string) contract for the lazy index path.
 *
 * Index-based reads of VLSD channels resolve each record's inline 8-byte
 * offset into the channel's ##SD fragment chain: `read` returns the strings
 * (aligned with the group master timestamps) and `values` returns NaN per
 * sample (strings are not numeric). `signalByteRanges` includes the ##SD
 * spans so the fragment path fetches everything the decoder needs — and the
 * JSON round trip preserves the fragment list.
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

test("VLSD channel reads via the lazy index path (JSON round trip)", async () => {
  const { bytes, strs } = buildVlsdFile();
  const idx = MdfIndex.fromJson(MdfIndex.fromBytes(bytes).toJson());
  const source = new BytesRangeSource(bytes);

  // read: strings, one per record, timestamps aligned with the master.
  const sig = await idx.read("Label", source);
  assert.deepEqual(sig.values, strs);
  assert.equal(sig.timestamps.length, strs.length);

  // values: the numeric fast path maps strings to NaN, one per record.
  const vals = await idx.values("Label", source);
  assert.equal(vals.length, strs.length);
  assert.ok(Array.from(vals).every((v) => Number.isNaN(v)));

  idx.dispose();
});

test("signalByteRanges for a VLSD channel covers its ##SD stream", () => {
  const { bytes } = buildVlsdFile();
  const idx = MdfIndex.fromBytes(bytes);

  // The VLSD channel needs more bytes than its group's fixed records alone:
  // its ranges must strictly contain the fixed-width channel's ranges.
  const fixed = idx.signalByteRanges("Value");
  const vlsd = idx.signalByteRanges("Label");
  const total = (rs: [number, number][]) => rs.reduce((n, [, len]) => n + len, 0);
  assert.ok(
    total(vlsd as [number, number][]) > total(fixed as [number, number][]),
    "VLSD ranges must include the ##SD data on top of the record data",
  );

  // byteRanges (static per-channel spans) still refuses VLSD channels.
  assert.throws(() => idx.byteRanges("Label"), /VLSD/);

  idx.dispose();
});

test("VLSD and fixed-width channels coexist on the index path", async () => {
  const { bytes } = buildVlsdFile();
  const mdf = Mdf.fromBytes(bytes);
  const idx = MdfIndex.fromJson(MdfIndex.fromBytes(bytes).toJson());
  const source = new BytesRangeSource(bytes);

  // Fixed-width channel in the same (VLSD-containing) group reads via the
  // index path and matches the direct reader.
  const lazy = await idx.read("Value", source);
  const direct = mdf.read("Value");
  assert.deepEqual(Array.from(lazy.values as number[]), Array.from(direct.values as number[]));
  assert.deepEqual(Array.from(lazy.timestamps), Array.from(direct.timestamps));

  const lazyVals = await idx.values("Value", source);
  assert.deepEqual(Array.from(lazyVals), Array.from(mdf.values("Value")));

  // And the VLSD channel matches the direct reader too.
  const lazyLabel = await idx.read("Label", source);
  assert.deepEqual(lazyLabel.values, mdf.read("Label").values);

  idx.dispose();
  mdf.dispose();
});
