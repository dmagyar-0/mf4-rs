import { test } from "node:test";
import assert from "node:assert/strict";

import { Mdf } from "../src/mdf";
import { MdfIndex } from "../src/mdf-index";
import { BytesRangeSource } from "../src/range-source";
import { buildSampleFile } from "./helpers";

test("MdfIndex.fromBytes exposes the same group/channel metadata as Mdf", () => {
  const { bytes } = buildSampleFile(50);
  const index = MdfIndex.fromBytes(bytes);
  index.validate();

  const groups = index.groups();
  assert.equal(groups.length, 1);
  assert.equal(groups[0]!.name, "Group1");
  assert.equal(groups[0]!.recordCount, 50);
  assert.deepEqual(groups[0]!.channelNames, ["Time", "Value", "Count"]);
  assert.equal(groups[0]!.masterChannel, "Time");
  assert.deepEqual(index.channelNames(), ["Time", "Value", "Count"]);
  assert.equal(index.fileSize(), bytes.length);
  index.dispose();
});

test("MdfIndex JSON round-trip (toJson -> fromJson) preserves reads", async () => {
  const { bytes, values } = buildSampleFile(50);
  const index = MdfIndex.fromBytes(bytes);
  const json = index.toJson();
  index.dispose();

  const reloaded = MdfIndex.fromJson(json);
  const source = new BytesRangeSource(bytes);
  const read = await reloaded.values("Value", source, "Group1");
  assert.equal(read.length, values.length);
  for (let i = 0; i < values.length; i++) {
    assert.ok(Math.abs(read[i]! - values[i]!) < 1e-9);
  }
  reloaded.dispose();
});

test("MdfIndex lazy values()/read() over BytesRangeSource match Mdf.values()/read()", async () => {
  const { bytes } = buildSampleFile(75);
  const mdf = Mdf.fromBytes(bytes);
  const direct = mdf.values("Value", "Group1");
  const directSignal = mdf.read("Value", "Group1");

  const index = MdfIndex.fromBytes(bytes);
  const source = new BytesRangeSource(bytes);

  const lazyValues = await index.values("Value", source, "Group1");
  assert.equal(lazyValues.length, direct.length);
  for (let i = 0; i < direct.length; i++) {
    assert.equal(lazyValues[i], direct[i]);
  }

  const lazySignal = await index.read("Value", source, "Group1");
  assert.equal(lazySignal.name, directSignal.name);
  assert.deepEqual(Array.from(lazySignal.timestamps), Array.from(directSignal.timestamps));
  assert.deepEqual(lazySignal.values, directSignal.values);

  mdf.dispose();
  index.dispose();
});

test("MdfIndex byteRanges()/signalByteRanges()/byteRangesForRecords() return non-empty ranges", () => {
  const { bytes } = buildSampleFile(50);
  const index = MdfIndex.fromBytes(bytes);

  const byteRanges = index.byteRanges("Value", "Group1");
  assert.ok(byteRanges.length > 0);
  for (const [offset, length] of byteRanges) {
    assert.ok(offset >= 0);
    assert.ok(length > 0);
  }

  const signalRanges = index.signalByteRanges("Value", "Group1");
  assert.ok(signalRanges.length > 0);

  const windowRanges = index.byteRangesForRecords("Value", 0n, 10n, "Group1");
  assert.ok(windowRanges.length > 0);

  index.dispose();
});
