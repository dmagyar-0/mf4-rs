import { test } from "node:test";
import assert from "node:assert/strict";

import "./helpers";
import { Mdf } from "../src/mdf";
import { MdfIndex } from "../src/mdf-index";
import { MdfWriter } from "../src/mdf-writer";
import { BytesRangeSource, type RangeSource } from "../src/range-source";

/** Time master, a linearly scaled `Temp` and a value-to-text `Gear`. */
function buildFile(): Uint8Array {
  const w = new MdfWriter();
  w.initMdfFile();
  const g = w.addChannelGroup("Engine");
  const t = w.addTimeChannel(g, "Time");
  const temp = w.addIntChannel(g, "Temp");
  const gear = w.addIntChannel(g, "Gear");
  w.addFloatChannel(g, "Plain");
  w.setTimeChannel(t);
  w.addLinearConversion(temp, -40, 0.5);
  w.addValueToTextConversion(gear, [[0, "N"], [1, "D"], [2, "R"]], "?");
  w.startDataBlock(g);
  for (let i = 0; i < 4; i++) w.writeRecord(g, [i * 0.1, 100 + i, i, 1.5]);
  w.finishDataBlock(g);
  const bytes = w.finalize();
  w.dispose();
  return bytes;
}

const expectedLinear = { type: "Linear", values: [-40, 0.5], precision: 0, flags: 0 };

function checkGear(info: unknown): void {
  const gear = info as { type: string; values: number[]; texts: Record<string, string> };
  assert.equal(gear.type, "ValueToText");
  assert.deepEqual(gear.values, [0, 1, 2]);
  assert.deepEqual(gear.texts, { 0: "N", 1: "D", 2: "R", 3: "?" });
}

test("writer conversions are applied on read", () => {
  const mdf = Mdf.fromBytes(buildFile());
  assert.deepEqual(Array.from(mdf.values("Temp")), [10, 10.5, 11, 11.5]);
  assert.deepEqual(mdf.read("Gear").values, ["N", "D", "R", "?"]);
  mdf.dispose();
});

test("Mdf.conversionInfo describes a channel's conversion", () => {
  const mdf = Mdf.fromBytes(buildFile());
  assert.deepEqual(mdf.conversionInfo("Temp"), expectedLinear);
  checkGear(mdf.conversionInfo("Gear", "Engine"));
  assert.equal(mdf.conversionInfo("Plain"), null);
  assert.throws(() => mdf.conversionInfo("Nope"), /not found/);
  mdf.dispose();
});

test("MdfIndex.conversionInfo: local index and lazily over a RangeSource", async () => {
  const bytes = buildFile();
  const local = MdfIndex.fromBytes(bytes);
  assert.deepEqual(await local.conversionInfo("Temp"), expectedLinear);
  checkGear(await local.conversionInfo("Gear"));
  assert.equal(await local.conversionInfo("Plain"), null);
  // A local index stores conversions resolved: no source needed.
  const json = local.toJson();
  local.dispose();
  const loaded = MdfIndex.fromJson(json);
  checkGear(await loaded.conversionInfo("Gear"));
  loaded.dispose();

  // A network-built index fetches the conversion block on demand.
  let requests = 0;
  const inner = new BytesRangeSource(bytes);
  const counting: RangeSource = {
    size: () => inner.size(),
    read: (o, l) => {
      requests++;
      return inner.read(o, l);
    },
  };
  const remote = await MdfIndex.fromRangeSource(counting);
  assert.equal(remote.channel("Gear")?.hasConversion, true);
  requests = 0;
  checkGear(await remote.conversionInfo("Gear"));
  assert.deepEqual(await remote.conversionInfo("Temp", { group: "Engine" }), expectedLinear);
  assert.ok(requests <= 4, `${requests} requests`);
  remote.setSource(undefined);
  await assert.rejects(remote.conversionInfo("Gear"), /no source/);
  assert.equal(await remote.conversionInfo("Plain"), null);
  remote.dispose();
});
