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
import { toColumnOriented, toColumnOrientedStream } from "../src/column";
import { toColumnOrientedFile } from "../src/column-file";

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
    const expected = src.read(c, { group: g });
    const actual = col.read(c, { group: g });
    assert.deepEqual(actual.values, expected.values, `${g}/${c}`);
    assert.deepEqual(actual.timestamps, expected.timestamps, `${g}/${c} timestamps`);
    assert.deepEqual(Array.from(col.values(c, { group: g })), Array.from(src.values(c, { group: g })));
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
    const signal = await index.read(c, { source: counter, group: g });
    const reads = counter.reads - before;
    const transferred = counter.bytes - beforeBytes;
    const expected = ref.read(c, { group: g });
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
  await index.values("Speed", { source: counter, group: "Engine" });
  assert.equal(counter.reads - before, 1);
  assert.ok(counter.bytes - beforeBytes < 8 * FAST + 4096, `${counter.bytes - beforeBytes} bytes`);
  ref.dispose();
  index.dispose();
});

test("readMany fetches a shared time axis once and matches the source groups", async () => {
  const source = buildSource();
  const { bytes } = toColumnOriented(source, { blockSize: 64 * 1024 });
  const ref = Mdf.fromBytes(source);
  const column = Mdf.fromBytes(bytes);
  const counter = new CountingSource(new BytesRangeSource(bytes));
  const index = await MdfIndex.fromRangeSource(counter, { seedBytes: 64 * 1024 });

  // Engine and Chassis share one time axis: three signals, one time fetch.
  const names = ["Speed", "Rpm", "Lateral"];
  const before = counter.bytes;
  const signals = await index.readMany(names, { source: counter });
  const transferred = counter.bytes - before;
  names.forEach((name, i) => {
    const expected = ref.read(name);
    assert.deepEqual(signals[i]!.values, expected.values, name);
    assert.deepEqual(signals[i]!.timestamps, expected.timestamps, `${name} timestamps`);
  });
  assert.ok(transferred < 4 * 8 * FAST + 4096, `${transferred} bytes for 3 signals + 1 time axis`);
  assert.deepEqual(column.readMany(names).map((s) => s.values), signals.map((s) => s.values));

  // A time window, searched once.
  const window = { startTime: 1, endTime: 2 };
  const windowed = await index.readMany(names, { ...window, source: counter });
  names.forEach((name, i) => assert.deepEqual(windowed[i]!.values, ref.read(name, window).values, name));

  // The column file's groups (and a time channel shared under another
  // group's name) answer like the source file's.
  for (const g of ["Engine", "Chassis", "Slow"]) {
    assert.deepEqual(column.group(g), ref.group(g), `Mdf group ${g}`);
    assert.deepEqual(index.group(g), ref.group(g), `MdfIndex group ${g}`);
    assert.deepEqual(column.read("Time", { group: g }).values, ref.read("Time", { group: g }).values, `${g}/Time`);
  }
  ref.dispose();
  column.dispose();
  index.dispose();
});

test("readMany fetches a large contiguous batch as bounded concurrent chunks", async () => {
  const w = new MdfWriter();
  w.initMdfFile();
  const g = w.addChannelGroup("Big");
  w.addTimeChannel(g, "t");
  w.addFloatChannel(g, "a");
  w.addFloatChannel(g, "b");
  const n = 700_000;
  const t = Float64Array.from({ length: n }, (_, i) => i * 1e-3);
  const a = Float64Array.from({ length: n }, (_, i) => i % 1013);
  w.startDataBlock(g);
  w.writeColumns(g, [t, a, a]);
  w.finishDataBlock(g);
  const { bytes } = toColumnOriented(w.finalize(), {});
  w.dispose();

  const sizes: number[] = [];
  let inFlight = 0;
  let peak = 0;
  const recording: RangeSource = {
    size: () => Promise.resolve(bytes.length),
    knownSize: () => bytes.length,
    read: async (offset, length) => {
      sizes.push(length);
      peak = Math.max(peak, ++inFlight);
      await new Promise((resolve) => setImmediate(resolve));
      inFlight--;
      return bytes.subarray(offset, offset + length);
    },
  };
  const index = MdfIndex.fromBytes(bytes);
  const [sa, sb] = await index.readMany(["a", "b"], { source: recording });
  assert.equal(sa!.values.length, n);
  assert.deepEqual(sb!.values.slice(0, 3), [0, 1, 2]);
  assert.equal(sb!.timestamps[n - 1], (n - 1) * 1e-3);
  // Three columns of 5.6 MB sit back to back: one 16.8 MB span, fetched as
  // 8 MiB chunks in parallel rather than one huge request.
  assert.ok(Math.max(...sizes) <= 8 * 1024 * 1024, `largest request ${Math.max(...sizes)}`);
  assert.ok(sizes.length >= 3 && peak >= 2, `${sizes.length} requests, ${peak} in flight`);
  index.dispose();
});

test("a failed fetch starts no further requests", async () => {
  const bytes = toColumnOriented(buildSource(), {}).bytes;
  const index = MdfIndex.fromBytes(bytes);
  let started = 0;
  // The first request fails at once; the others answer slowly.
  const failing: RangeSource = {
    size: () => Promise.resolve(bytes.length),
    read: async (offset, length) => {
      if (started++ === 0) throw new Error("boom");
      await new Promise((resolve) => setTimeout(resolve, 5));
      return bytes.subarray(offset, offset + length);
    },
  };
  // Two time axes: the first round probes more than 16 separate ranges.
  await assert.rejects(index.readMany(["Speed", "Pressure"], { source: failing, startTime: 1, endTime: 2 }), /boom/);
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.ok(started <= 16, `${started} requests started`);
  index.dispose();
});
