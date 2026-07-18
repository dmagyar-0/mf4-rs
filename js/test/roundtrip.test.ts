import { test } from "node:test";
import assert from "node:assert/strict";

import { Mdf } from "../src/mdf";
import { buildSampleFile } from "./helpers";

test("MdfWriter -> Mdf round-trip: groups/channels metadata", () => {
  const { bytes } = buildSampleFile(100);
  const mdf = Mdf.fromBytes(bytes);

  const groups = mdf.groups();
  assert.equal(groups.length, 1);
  const group = groups[0]!;
  assert.equal(group.name, "Group1");
  assert.equal(group.recordCount, 100);

  const names = group.channels.map((c) => c.name);
  assert.deepEqual(names, ["Time", "Value", "Count"]);
  assert.equal(group.channels[0]!.isMaster, true);
  assert.equal(group.channels[1]!.isMaster, false);

  assert.deepEqual(mdf.channelNames(), ["Time", "Value", "Count"]);
  mdf.dispose();
});

test("MdfWriter -> Mdf round-trip: values() matches written data", () => {
  const { bytes, values } = buildSampleFile(100);
  const mdf = Mdf.fromBytes(bytes);

  const read = mdf.values("Value", "Group1");
  assert.equal(read.length, values.length);
  for (let i = 0; i < values.length; i++) {
    assert.ok(Math.abs(read[i]! - values[i]!) < 1e-9, `index ${i}: ${read[i]} vs ${values[i]}`);
  }
  mdf.dispose();
});

test("MdfWriter -> Mdf round-trip: read() pairs values with timestamps", () => {
  const { bytes, times, values, counts } = buildSampleFile(100);
  const mdf = Mdf.fromBytes(bytes);

  const signal = mdf.read("Value", "Group1");
  assert.equal(signal.name, "Value");
  assert.equal(signal.timestamps.length, times.length);
  assert.equal(signal.values.length, values.length);
  for (let i = 0; i < times.length; i++) {
    assert.ok(Math.abs(signal.timestamps[i]! - times[i]!) < 1e-9);
    assert.ok(Math.abs((signal.values[i] as number) - values[i]!) < 1e-9);
  }

  const countSignal = mdf.read("Count", "Group1");
  for (let i = 0; i < counts.length; i++) {
    assert.equal(Number(countSignal.values[i]), counts[i]);
  }
  mdf.dispose();
});

test("Mdf.startTimeNs() reflects the writer's setStartTime()", () => {
  const { bytes, startTimeNs } = buildSampleFile(10);
  const mdf = Mdf.fromBytes(bytes);
  assert.equal(mdf.startTimeNs(), startTimeNs);
  mdf.dispose();
});
