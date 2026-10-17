import { test } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

import "../src/wasm-module-node";
import { Mdf } from "../src/mdf";
import { MdfIndex } from "../src/mdf-index";
import { MdfWriter } from "../src/mdf-writer";
import { BytesRangeSource, type RangeSource } from "../src/range-source";
import {
  toColumnOriented,
  toColumnOrientedFile,
  toColumnOrientedStream,
} from "../src/column";

const FAST = 60_000;
const SLOW = 2_000;

/** Two groups on the same time axis (Engine, Chassis) plus one slower group. */
function buildSource(): Uint8Array {
  const w = new MdfWriter();
  w.initMdfFile();
  w.setStartTime(1_700_000_000_000_000_000n);
  const groups: [string, number, string[]][] = [
    ["Engine", FAST, ["Speed", "Rpm"]],
    ["Chassis", FAST, ["Lateral"]],
    ["Slow", SLOW, ["Pressure"]],
  ];
  const ids: string[] = [];
  for (const [name, , channels] of groups) {
    const g = w.addChannelGroup(name);
    const t = w.addTimeChannel(g, "Time");
    w.setTimeChannel(t);
    for (const c of channels) {
      if (c === "Rpm") w.addIntChannel(g, c);
      else w.addFloatChannel(g, c);
    }
    ids.push(g);
  }
  groups.forEach(([name, n, channels], gi) => {
    const step = name === "Slow" ? 0.1 : 0.01;
    w.startDataBlock(ids[gi]!);
    for (let i = 0; i < n; i++) {
      w.writeRecord(ids[gi]!, [i * step, ...channels.map((_, k) => (k === 1 ? i % 9000 : Math.sin(i * 0.001 + gi)))]);
    }
    w.finishDataBlock(ids[gi]!);
  });
  return w.finalize();
}

const SIGNALS: [string, string][] = [
  ["Engine", "Speed"],
  ["Engine", "Rpm"],
  ["Chassis", "Lateral"],
  ["Slow", "Pressure"],
];

class CountingSource implements RangeSource {
  reads = 0;
  bytes = 0;
  constructor(private readonly inner: RangeSource) {}
  size(): Promise<number> {
    return this.inner.size!();
  }
  async read(offset: number, length: number): Promise<Uint8Array> {
    this.reads++;
    this.bytes += length;
    return this.inner.read(offset, length);
  }
}

test("toColumnOriented round-trips every signal through Mdf and MdfIndex", () => {
  const source = buildSource();
  const { bytes, report } = toColumnOriented(source, { blockSize: 64 * 1024 });
  assert.equal(report.signalGroups, 4);
  assert.equal(report.timeGroups, 2);
  assert.equal(report.sharedTimeGroups, 1);
  assert.equal(report.fileSize, bytes.length);
  assert.deepEqual(report.skippedChannels, []);

  const src = Mdf.fromBytes(source);
  const col = Mdf.fromBytes(bytes);
  const idx = MdfIndex.fromBytes(bytes);
  for (const [g, c] of SIGNALS) {
    const expected = src.read(c, g);
    const actual = col.read(c, g);
    assert.deepEqual(actual.values, expected.values, `${g}/${c}`);
    assert.deepEqual(actual.timestamps, expected.timestamps, `${g}/${c} timestamps`);
    assert.deepEqual(Array.from(col.values(c, g)), Array.from(src.values(c, g)));
  }
  // Group metadata reports the shared time channel for signal groups.
  for (const g of idx.groups()) {
    if (g.channelNames[0] !== "Time") assert.equal(g.masterChannel, "Time");
  }
  src.dispose();
  col.dispose();
  idx.dispose();
});

test("streaming and file conversion produce the same file", async () => {
  const source = buildSource();
  const options = { blockSize: 32 * 1024, memoryBudget: 1, readChunkSize: 1 };
  const memory = toColumnOriented(source, options).bytes;

  const chunks = new Map<number, Uint8Array>();
  let size = 0;
  let maxRead = 0;
  const report = toColumnOrientedStream(
    {
      size: source.length,
      read(offset, length) {
        maxRead = Math.max(maxRead, length);
        return source.slice(offset, offset + length);
      },
    },
    (offset, bytes) => {
      chunks.set(offset, bytes.slice());
      size = Math.max(size, offset + bytes.length);
    },
    options,
  );
  const streamed = new Uint8Array(size);
  for (const [offset, bytes] of chunks) streamed.set(bytes, offset);
  assert.equal(streamed.length, report.fileSize);
  // Bounded reads: metadata blocks and record-aligned data chunks only.
  assert.ok(maxRead <= 64 * 1024, `largest read ${maxRead}`);

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "mf4-col-"));
  try {
    const input = path.join(dir, "in.mf4");
    const output = path.join(dir, "out.mf4");
    fs.writeFileSync(input, source);
    const fileReport = await toColumnOrientedFile(input, output, options);
    const fromFile = new Uint8Array(fs.readFileSync(output));
    assert.equal(fromFile.length, fileReport.fileSize);
    await assert.rejects(toColumnOrientedFile(input, input));

    // Identical apart from the file-history timestamp (8 bytes).
    for (const other of [streamed, fromFile]) {
      assert.equal(other.length, memory.length);
      let diff = 0;
      for (let i = 0; i < memory.length; i++) if (memory[i] !== other[i]) diff++;
      assert.ok(diff <= 8, `${diff} bytes differ`);
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("a column file indexes in one range request and reads a signal in at most two", async () => {
  const source = buildSource();
  const { bytes, report } = toColumnOriented(source, { blockSize: 64 * 1024 });
  const ref = Mdf.fromBytes(source);

  const counter = new CountingSource(new BytesRangeSource(bytes));
  const index = await MdfIndex.fromRangeSource(counter, { seedBytes: 64 * 1024 });
  const buildReads = counter.reads;
  // The whole structure is in the metadata prefix.
  assert.ok(report.metadataSize < 64 * 1024, `metadata ${report.metadataSize}`);
  assert.equal(buildReads, 1, "index build reads");

  for (const [g, c] of SIGNALS) {
    const before = counter.reads;
    const beforeBytes = counter.bytes;
    const signal = await index.read(c, counter, g);
    const reads = counter.reads - before;
    const transferred = counter.bytes - beforeBytes;
    const expected = ref.read(c, g);
    assert.deepEqual(signal.values, expected.values, `${g}/${c}`);
    assert.deepEqual(signal.timestamps, expected.timestamps, `${g}/${c} timestamps`);
    // Values and shared time channel: one contiguous span each (a single
    // request when the signal sits right after its time group).
    assert.ok(reads <= 2, `${g}/${c}: ${reads} reads`);
    console.log(`  ${g}/${c}: ${reads} range request(s), ${transferred} bytes for ${expected.values.length} samples`);
    const n = expected.values.length;
    assert.ok(transferred < 2 * 8 * n + 4096, `${g}/${c}: ${transferred} bytes for ${n} samples`);
  }
  // values(): the signal's own span only (no shared time channel).
  const before = counter.reads;
  const beforeBytes = counter.bytes;
  await index.values("Speed", counter, "Engine");
  assert.equal(counter.reads - before, 1);
  assert.ok(counter.bytes - beforeBytes < 8 * FAST + 4096, `${counter.bytes - beforeBytes} bytes`);
  ref.dispose();
  index.dispose();
});
