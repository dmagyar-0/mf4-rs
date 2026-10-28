import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import * as path from "node:path";

import { buildSampleFile } from "./helpers";
import { Mdf } from "../src/mdf";
import { MdfIndex } from "../src/mdf-index";
import { BytesRangeSource, type RangeSource } from "../src/range-source";
import { toAbsoluteNs } from "../src/signal-util";

/** Counts requests and bytes served from an in-memory buffer. */
class CountingSource implements RangeSource {
  requests = 0;
  bytes = 0;
  private readonly inner: BytesRangeSource;
  constructor(private readonly data: Uint8Array) {
    this.inner = new BytesRangeSource(data);
  }
  async size(): Promise<number> {
    return this.data.byteLength;
  }
  async read(offset: number, length: number): Promise<Uint8Array> {
    this.requests++;
    this.bytes += length;
    return this.inner.read(offset, length);
  }
}

test("MdfIndex keeps its source: values/read need no source argument", async () => {
  const { bytes, values, times } = buildSampleFile(50);
  const fromBytes = MdfIndex.fromBytes(bytes);
  assert.deepEqual(Array.from(await fromBytes.values("Value")), values);
  assert.deepEqual(Array.from((await fromBytes.read("Value", { group: "Group1" })).timestamps), times);
  // The original (source, group) signature still works.
  assert.deepEqual(Array.from(await fromBytes.values("Value", new BytesRangeSource(bytes), "Group1")), values);

  const json = fromBytes.toJson();
  fromBytes.dispose();
  const loaded = MdfIndex.fromJson(json);
  await assert.rejects(loaded.values("Value"), /no source/);
  loaded.setSource(new BytesRangeSource(bytes));
  assert.deepEqual(Array.from(await loaded.values("Value")), values);
  loaded.dispose();

  const remote = await MdfIndex.fromRangeSource(new BytesRangeSource(bytes));
  assert.ok(remote.source);
  assert.deepEqual(Array.from(await remote.values("Value")), values);
  remote.dispose();
});

test("MdfIndex metadata: start time, channel info, comments", () => {
  const { bytes, startTimeNs } = buildSampleFile(5);
  const idx = MdfIndex.fromBytes(bytes);
  assert.equal(idx.startTimeNs(), startTimeNs);
  const [group] = idx.groups();
  assert.equal(group!.masterChannel, "Time");
  assert.deepEqual(
    group!.channels.map((c) => [c.name, c.dataType, c.isMaster, c.bitCount]),
    [
      ["Time", "FloatLE", true, 64],
      ["Value", "FloatLE", false, 64],
      ["Count", "UnsignedIntegerLE", false, 64],
    ],
  );
  const value = idx.channel("Value", "Group1");
  assert.equal(value?.isVlsd, false);
  assert.equal(value?.hasConversion, false);
  assert.equal(idx.channel("Nope"), undefined);
  idx.dispose();
});

test("windowed reads by record and by time match a full read", async () => {
  const n = 300_000; // 24-byte records -> two data blocks
  const { bytes, values, times } = buildSampleFile(n);
  const source = new CountingSource(bytes);
  const idx = await MdfIndex.fromRangeSource(source);

  // Record window straddling the first block boundary (4 MiB / 24 bytes).
  const start = 174_700;
  const byRecord = await idx.read("Value", { startRecord: start, recordCount: 100 });
  assert.deepEqual(byRecord.values, values.slice(start, start + 100));
  assert.deepEqual(Array.from(byRecord.timestamps), times.slice(start, start + 100));

  // Time window: 0.01 s spacing, inclusive bounds.
  source.requests = 0;
  source.bytes = 0;
  const t0 = times[120_000]!;
  const t1 = times[120_050]!;
  const byTime = await idx.read("Value", { startTime: t0, endTime: t1 });
  assert.deepEqual(byTime.values, values.slice(120_000, 120_051));
  assert.ok(source.bytes < 32 * 1024, `fetched ${source.bytes} bytes for a 51-record window`);
  console.log(`# time window: ${source.requests} requests, ${source.bytes} bytes`);

  const vals = await idx.values("Count", { startTime: t0, endTime: t1 });
  assert.equal(vals.length, 51);
  assert.equal(vals[0], 120_000);

  // Open-ended and empty windows.
  assert.equal((await idx.values("Value", { endTime: times[9]! })).length, 10);
  assert.equal((await idx.values("Value", { startTime: times[n - 3]! })).length, 3);
  assert.equal((await idx.values("Value", { startTime: 1e9 })).length, 0);
  await assert.rejects(idx.values("Value", { startTime: 0, startRecord: 0 }), /either/);
  await assert.rejects(idx.values("Value", { startRecord: n, recordCount: 1 }), /exceeds/);

  // Start/end time of the group: two single-record reads.
  assert.deepEqual(await idx.timeRange("Value"), { start: times[0], end: times[n - 1] });
  assert.deepEqual(await idx.timeRange("Time"), { start: times[0], end: times[n - 1] });
  idx.dispose();

  // Same windows through the in-memory reader.
  const mdf = Mdf.fromBytes(bytes);
  assert.deepEqual(mdf.read("Value", { startTime: t0, endTime: t1 }).values, byTime.values);
  assert.deepEqual(Array.from(mdf.values("Value", { startRecord: start, recordCount: 100 })), values.slice(start, start + 100));
  assert.deepEqual(mdf.timeRange("Value"), { start: times[0], end: times[n - 1] });
  mdf.dispose();
});

test("toAbsoluteNs adds the start time in exact integer nanoseconds", () => {
  const start = 1_700_000_000_123_456_789n;
  assert.deepEqual(Array.from(toAbsoluteNs([0, 0.5, 1e-9, NaN], start)), [
    start,
    start + 500_000_000n,
    start + 1n,
    0n,
  ]);
});

test("FetchRangeSource calls the platform fetch with the global receiver", async () => {
  const { bytes } = buildSampleFile(10);
  const realFetch = globalThis.fetch;
  // Browsers throw "Illegal invocation" when fetch runs with another receiver.
  const strictFetch = function (this: unknown, _input: string, init?: { headers?: Record<string, string> }) {
    if (this !== undefined && this !== globalThis) {
      throw new TypeError("Failed to execute 'fetch' on 'WorkerGlobalScope': Illegal invocation");
    }
    const m = /bytes=(\d+)-(\d+)/.exec(init?.headers?.Range ?? "");
    const body = m ? bytes.slice(Number(m[1]), Number(m[2]) + 1) : bytes;
    const headers = new Headers(m ? { "content-range": `bytes ${m[1]}-${m[2]}/${bytes.length}` } : {});
    return Promise.resolve(new Response(body as unknown as ArrayBuffer, { status: m ? 206 : 200, headers }));
  };
  (globalThis as { fetch: unknown }).fetch = strictFetch;
  try {
    const idx = await MdfIndex.fromUrl("http://example.invalid/x.mf4");
    assert.equal((await idx.values("Value")).length, 10);
    idx.dispose();
  } finally {
    globalThis.fetch = realFetch;
  }
});

test("the web build references no node: built-ins", { skip: !existsSync(path.join(__dirname, "../../dist-web")) }, () => {
  const dir = path.join(__dirname, "../../dist-web");
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".js"))) {
    const text = readFileSync(path.join(dir, file), "utf8");
    assert.doesNotMatch(text, /["']node:/, `${file} references a node: module`);
  }
});

test("modules shared with the web entry import no node: built-ins", () => {
  for (const file of ["mdf.js", "mdf-index.js", "range-source.js", "column.js", "platform.js"]) {
    const text = readFileSync(path.join(__dirname, "../src", file), "utf8");
    assert.doesNotMatch(text, /["']node:/, `${file} references a node: module`);
  }
});
