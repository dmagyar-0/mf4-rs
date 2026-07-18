import { test } from "node:test";
import assert from "node:assert/strict";

import { Mdf } from "../src/mdf";
import { MdfIndex } from "../src/mdf-index";
import { MdfWriter } from "../src/mdf-writer";
import { BytesRangeSource } from "../src/range-source";
import { buildSampleFile } from "./helpers";

/**
 * Regression for the `signalByteRanges` contract (fix #2): its output must
 * fully cover the group's data section so that feeding it straight to
 * `valuesFromFragments` / `readFromFragments` works for *any* channel in a
 * 3+-channel group — not only the group's last channel.
 */
test("signalByteRanges output feeds valuesFromFragments/readFromFragments for every channel", async () => {
  const { bytes, times, values, counts } = buildSampleFile(64);
  const index = MdfIndex.fromBytes(bytes);
  const source = new BytesRangeSource(bytes);

  const expected: Record<string, number[]> = {
    Time: times,
    Value: values,
    Count: counts,
  };

  for (const name of ["Time", "Value", "Count"] as const) {
    const ranges = index.signalByteRanges(name, "Group1");
    assert.ok(ranges.length > 0, `${name}: expected non-empty ranges`);
    const fragments = await Promise.all(ranges.map(([o, l]) => source.read(o, l)));

    const vals = index.valuesFromFragments(name, "Group1", ranges, fragments);
    assert.equal(vals.length, expected[name]!.length, `${name}: value count`);
    for (let i = 0; i < vals.length; i++) {
      assert.ok(Math.abs(vals[i]! - expected[name]![i]!) < 1e-9, `${name}[${i}]`);
    }

    const sig = index.readFromFragments(name, "Group1", ranges, fragments);
    assert.equal(sig.name, name);
    assert.equal(sig.values.length, expected[name]!.length);
    for (let i = 0; i < sig.values.length; i++) {
      assert.ok(Math.abs((sig.values[i] as number) - expected[name]![i]!) < 1e-9);
    }
  }

  index.dispose();
});

/** Build a two-group file where both groups share a channel name (`Value`). */
function buildMultiGroupFile(): Uint8Array {
  const writer = new MdfWriter();
  writer.initMdfFile();

  const gA = writer.addChannelGroup("GroupA");
  const tA = writer.addTimeChannel(gA, "Time");
  writer.addFloatChannel(gA, "Value");
  writer.setTimeChannel(tA);
  writer.startDataBlock(gA);
  for (let i = 0; i < 10; i++) {
    writer.writeRecord(gA, [i * 0.1, i]);
  }
  writer.finishDataBlock(gA);

  const gB = writer.addChannelGroup("GroupB");
  const tB = writer.addTimeChannel(gB, "Time");
  writer.addFloatChannel(gB, "Value");
  writer.setTimeChannel(tB);
  writer.startDataBlock(gB);
  for (let i = 0; i < 5; i++) {
    writer.writeRecord(gB, [i * 0.2, i * 100]);
  }
  writer.finishDataBlock(gB);

  const bytes = writer.finalize();
  writer.dispose();
  return bytes;
}

test("multi-group file: group disambiguation selects the right group's data", async () => {
  const bytes = buildMultiGroupFile();

  const mdf = Mdf.fromBytes(bytes);
  const groups = mdf.groups();
  assert.equal(groups.length, 2);
  assert.deepEqual(
    groups.map((g) => g.name),
    ["GroupA", "GroupB"],
  );

  const a = mdf.values("Value", "GroupA");
  const b = mdf.values("Value", "GroupB");
  assert.deepEqual(Array.from(a), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  assert.deepEqual(Array.from(b), [0, 100, 200, 300, 400]);
  mdf.dispose();

  // Same disambiguation through the lazy index path.
  const index = MdfIndex.fromBytes(bytes);
  const source = new BytesRangeSource(bytes);
  const lazyA = await index.values("Value", source, "GroupA");
  const lazyB = await index.values("Value", source, "GroupB");
  assert.deepEqual(Array.from(lazyA), Array.from(a));
  assert.deepEqual(Array.from(lazyB), Array.from(b));
  index.dispose();
});

test("0-record group reads back as empty", async () => {
  const writer = new MdfWriter();
  writer.initMdfFile();
  const g = writer.addChannelGroup("Empty");
  const t = writer.addTimeChannel(g, "Time");
  writer.addFloatChannel(g, "Value");
  writer.setTimeChannel(t);
  writer.startDataBlock(g);
  // No records written.
  writer.finishDataBlock(g);
  const bytes = writer.finalize();
  writer.dispose();

  const mdf = Mdf.fromBytes(bytes);
  assert.equal(mdf.groups()[0]!.recordCount, 0);
  const vals = mdf.values("Value", "Empty");
  assert.equal(vals.length, 0);
  const sig = mdf.read("Value", "Empty");
  assert.equal(sig.values.length, 0);
  assert.equal(sig.timestamps.length, 0);
  mdf.dispose();

  const index = MdfIndex.fromBytes(bytes);
  const source = new BytesRangeSource(bytes);
  const lazy = await index.values("Value", source, "Empty");
  assert.equal(lazy.length, 0);
  index.dispose();
});

test("BigInt / large-integer write -> read round-trip", () => {
  const writer = new MdfWriter();
  writer.initMdfFile();
  const g = writer.addChannelGroup("Big");
  const t = writer.addTimeChannel(g, "Time");
  writer.addIntChannel(g, "Big"); // u64 channel
  writer.setTimeChannel(t);

  // A value well beyond Number.MAX_SAFE_INTEGER (2^53 - 1).
  const big = 9_223_372_036_854_775_807n; // i64::MAX, fits in u64
  writer.startDataBlock(g);
  writer.writeRecord(g, [0, big]);
  writer.writeRecord(g, [1, 42n]); // BigInt small value too
  writer.finishDataBlock(g);
  const bytes = writer.finalize();
  writer.dispose();

  const mdf = Mdf.fromBytes(bytes);
  const sig = mdf.read("Big", "Big");
  assert.equal(sig.values.length, 2);
  assert.equal(sig.values[0], big); // emitted as BigInt (> 2^53)
  assert.equal(typeof sig.values[0], "bigint");
  assert.equal(Number(sig.values[1]), 42);
  mdf.dispose();
});

test("string (VLSD) channel write -> read round-trip", () => {
  const writer = new MdfWriter();
  writer.initMdfFile();
  const g = writer.addChannelGroup("Strings");
  const t = writer.addTimeChannel(g, "Time");
  writer.addStringChannel(g, "Msg");
  writer.setTimeChannel(t);

  const messages = ["hello", "world", "mf4-rs", "", "utf8: ✓"];
  writer.startDataBlock(g);
  messages.forEach((m, i) => writer.writeRecord(g, [i * 0.5, m]));
  writer.finishDataBlock(g);
  const bytes = writer.finalize();
  writer.dispose();

  const mdf = Mdf.fromBytes(bytes);
  const sig = mdf.read("Msg", "Strings");
  assert.equal(sig.values.length, messages.length);
  assert.deepEqual(sig.values, messages);
  mdf.dispose();
});

test("addChannel rejects string data types, directing to addStringChannel", () => {
  const writer = new MdfWriter();
  writer.initMdfFile();
  const g = writer.addChannelGroup("G");
  assert.throws(() => writer.addChannel(g, "S", "StringUtf8"), /addStringChannel/);
  writer.dispose();
});
