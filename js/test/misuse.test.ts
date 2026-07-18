import { test } from "node:test";
import assert from "node:assert/strict";
import * as os from "node:os";
import * as path from "node:path";
import * as fsp from "node:fs/promises";

import { Mdf } from "../src/mdf";
import { MdfIndex } from "../src/mdf-index";
import { MdfWriter } from "../src/mdf-writer";
import { FileRangeSource, BytesRangeSource } from "../src/range-source";
import { buildSampleFile } from "./helpers";

test("Mdf.values() throws for an unknown channel name", () => {
  const { bytes } = buildSampleFile(10);
  const mdf = Mdf.fromBytes(bytes);
  assert.throws(() => mdf.values("DoesNotExist", "Group1"));
  mdf.dispose();
});

test("Mdf.read() throws for an unknown channel name", () => {
  const { bytes } = buildSampleFile(10);
  const mdf = Mdf.fromBytes(bytes);
  assert.throws(() => mdf.read("DoesNotExist"));
  mdf.dispose();
});

test("MdfIndex.valuesFromFragments() throws for an unknown channel name", async () => {
  const { bytes } = buildSampleFile(10);
  const index = MdfIndex.fromBytes(bytes);
  const source = new BytesRangeSource(bytes);
  await assert.rejects(() => index.values("DoesNotExist", source, "Group1"));
  index.dispose();
});

test("MdfWriter.finalize() consumes the writer; further calls raise", () => {
  const writer = new MdfWriter();
  writer.initMdfFile();
  const gid = writer.addChannelGroup("G");
  const tid = writer.addTimeChannel(gid, "Time");
  writer.setTimeChannel(tid);
  writer.startDataBlock(gid);
  writer.writeRecord(gid, [0]);
  writer.finishDataBlock(gid);
  writer.finalize();
  assert.throws(() => writer.finalize());
});

test("Mdf.fromFile() reads a file written to disk via MdfWriter", async () => {
  const { bytes, values } = buildSampleFile(20);
  const tmpPath = path.join(await fsp.mkdtemp(path.join(os.tmpdir(), "mf4-rs-test-")), "sample.mf4");
  await fsp.writeFile(tmpPath, bytes);

  const mdf = await Mdf.fromFile(tmpPath);
  const read = mdf.values("Value", "Group1");
  assert.equal(read.length, values.length);
  for (let i = 0; i < values.length; i++) {
    assert.ok(Math.abs(read[i]! - values[i]!) < 1e-9);
  }
  mdf.dispose();
  await fsp.rm(path.dirname(tmpPath), { recursive: true, force: true });
});

test("FileRangeSource reads matching bytes at arbitrary offsets", async () => {
  const { bytes } = buildSampleFile(20);
  const tmpPath = path.join(await fsp.mkdtemp(path.join(os.tmpdir(), "mf4-rs-test-")), "sample.mf4");
  await fsp.writeFile(tmpPath, bytes);

  const source = new FileRangeSource(tmpPath);
  assert.equal(await source.size(), bytes.length);
  const chunk = await source.read(10, 20);
  assert.deepEqual(Array.from(chunk), Array.from(bytes.subarray(10, 30)));
  await source.close();
  await fsp.rm(path.dirname(tmpPath), { recursive: true, force: true });
});
