// `raw: true` in ReadOptions: values as stored, without the conversion. The
// golden contract test covers Mdf and a bytes-built MdfIndex; this covers an
// index built over a RangeSource, whose conversions are resolved lazily.

import assert from "node:assert/strict";
import { test } from "node:test";

import { Mdf, MdfIndex, MdfWriter } from "../src/index";
import { BytesRangeSource } from "../src/range-source";

/** `Time = 0.01 * tick`, `Speed = 1 + 2 * raw`, `Gear` value-to-text. */
function buildFile(): Uint8Array {
  const w = new MdfWriter();
  w.initMdfFile();
  const g = w.addChannelGroup("Engine");
  const time = w.addIntChannel(g, "Time");
  w.setTimeChannel(time);
  const speed = w.addFloatChannel(g, "Speed");
  const gear = w.addIntChannel(g, "Gear");
  w.addLinearConversion(time, 0, 0.01);
  w.addLinearConversion(speed, 1, 2);
  w.addValueToTextConversion(gear, [[0, "N"], [1, "D"]], "?");
  w.startDataBlock(g);
  for (let i = 0; i < 10; i++) w.writeRecord(g, [i * 10, i, i % 3]);
  w.finishDataBlock(g);
  const bytes = w.finalize();
  w.dispose();
  return bytes;
}

test("raw reads skip the conversion on Mdf and on a lazily built MdfIndex", async () => {
  const bytes = buildFile();
  const mdf = Mdf.fromBytes(bytes);
  const index = await MdfIndex.fromRangeSource(new BytesRangeSource(bytes));
  const range = (n: number, f: (i: number) => number) => Array.from({ length: n }, (_, i) => f(i));

  assert.deepEqual(Array.from(mdf.values("Speed", { raw: true })), range(10, (i) => i));
  assert.deepEqual(Array.from(mdf.values("Speed")), range(10, (i) => 1 + 2 * i));
  assert.deepEqual(mdf.read("Gear", { raw: true }).values, range(10, (i) => i % 3));
  assert.equal(mdf.read("Gear").values[2], "?");

  // Time windows select on the physical axis, also for the raw master.
  const window = { startTime: 0.2, endTime: 0.4, raw: true };
  assert.deepEqual(Array.from(mdf.values("Speed", window)), [2, 3, 4]);
  assert.deepEqual(Array.from(mdf.values("Time", window)), [20, 30, 40]);

  const cases: [string, object][] = [
    ["Speed", { raw: true }],
    ["Gear", { raw: true }],
    ["Time", { raw: true }],
    ["Speed", window],
    ["Time", window],
    ["Gear", { startRecord: 3, recordCount: 4, raw: true }],
    ["Speed", {}],
  ];
  for (const [name, options] of cases) {
    const label = `${name} ${JSON.stringify(options)}`;
    assert.deepEqual(await index.read(name, options), mdf.read(name, options), label);
    assert.deepEqual(Array.from(await index.values(name, options)), Array.from(mdf.values(name, options)), label);
  }
  const s = mdf.read("Speed", { raw: true });
  assert.ok(Math.abs(s.timestamps[9]! - 0.09 * 10) < 1e-12, "timestamps stay physical");
  index.dispose();
  mdf.dispose();
});
