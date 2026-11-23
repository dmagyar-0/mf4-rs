// Records split across the ##DT fragments of a ##DL list: the blocks form one
// continuous byte stream, so a record may start in one block and end in the
// next. `MdfIndex` (built locally or from a range source) must read such a
// file exactly as `Mdf` does — whole reads, windows, readMany and timeRange.
import { test } from "node:test";
import assert from "node:assert/strict";

import { Mdf } from "../src/mdf";
import { MdfIndex } from "../src/mdf-index";
import { BytesRangeSource } from "../src/range-source";
import type { ReadOptions, Signal } from "../src/types";
import { buildSampleFile } from "./helpers";

/** A raw MDF block: 24-byte header, links, data. */
function block(id: string, links: bigint[], data: Uint8Array): Uint8Array {
  const out = new Uint8Array(24 + links.length * 8 + data.length);
  const view = new DataView(out.buffer);
  out.set(new TextEncoder().encode(id), 0);
  view.setBigUint64(8, BigInt(out.length), true);
  view.setBigUint64(16, BigInt(links.length), true);
  links.forEach((l, i) => view.setBigUint64(24 + i * 8, l, true));
  out.set(data, 24 + links.length * 8);
  return out;
}

/**
 * Rewrite the first data group's single ##DT block as a ##DL list of
 * fragments with the given data sizes (records may straddle fragments).
 */
function splitDataBlock(bytes: Uint8Array, sizes: number[]): Uint8Array {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const u64 = (o: number) => Number(view.getBigUint64(o, true));
  const dg = u64(64 + 24);
  const dt = u64(dg + 40);
  assert.equal(new TextDecoder().decode(bytes.subarray(dt, dt + 4)), "##DT");
  const len = u64(dt + 8) - 24;
  assert.equal(sizes.reduce((a, b) => a + b, 0), len, "fragment sizes must cover the data");
  const data = bytes.subarray(dt + 24, dt + 24 + len);

  const parts: Uint8Array[] = [bytes];
  let size = bytes.length;
  const append = (b: Uint8Array): number => {
    const pad = (8 - (size % 8)) % 8;
    if (pad) {
      parts.push(new Uint8Array(pad));
      size += pad;
    }
    const at = size;
    parts.push(b);
    size += b.length;
    return at;
  };
  const links: bigint[] = [0n];
  const offsets: bigint[] = [];
  let pos = 0;
  for (const s of sizes) {
    links.push(BigInt(append(block("##DT", [], data.subarray(pos, pos + s)))));
    offsets.push(BigInt(pos));
    pos += s;
  }
  const dlData = new Uint8Array(8 + offsets.length * 8);
  const dlView = new DataView(dlData.buffer);
  dlView.setUint32(4, sizes.length, true);
  offsets.forEach((o, i) => dlView.setBigUint64(8 + i * 8, o, true));
  const dl = append(block("##DL", links, dlData));

  const out = new Uint8Array(size);
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  new DataView(out.buffer).setBigUint64(dg + 40, BigInt(dl), true);
  return out;
}

const RECORDS = 1000; // Time, Value, Count: 24-byte records, 24000 bytes
const SPLITS = [1000, 7, 13_000, 4, 20, 9969];

function plain(s: Signal) {
  return { values: s.values, timestamps: Array.from(s.timestamps) };
}

test("MdfIndex reads records split across data blocks exactly as Mdf does", async () => {
  const { bytes, values } = buildSampleFile(RECORDS);
  const split = splitDataBlock(bytes, SPLITS);
  const mdf = Mdf.fromBytes(split);
  assert.deepEqual(Array.from(mdf.values("Value")), values, "Mdf reads the split file");

  const time = Array.from(mdf.values("Time"));
  const windows: ReadOptions[] = [
    {},
    { startRecord: 41, recordCount: 2 }, // record 41 spans three fragments
    { startRecord: 40, recordCount: 600 },
    { startRecord: RECORDS - 1, recordCount: 1 },
    { startTime: time[40], endTime: time[43] },
    { startTime: time[583] },
  ];
  const indexes: [string, MdfIndex][] = [
    ["fromBytes", MdfIndex.fromBytes(split)],
    ["fromRangeSource", await MdfIndex.fromRangeSource(new BytesRangeSource(split))],
  ];
  for (const [label, index] of indexes) {
    for (const options of windows) {
      for (const name of ["Value", "Count", "Time"]) {
        assert.deepEqual(
          plain(await index.read(name, options)),
          plain(mdf.read(name, options)),
          `${label}: read(${name}, ${JSON.stringify(options)})`,
        );
        assert.deepEqual(
          Array.from(await index.values(name, options)),
          Array.from(mdf.values(name, options)),
          `${label}: values(${name}, ${JSON.stringify(options)})`,
        );
      }
      const many = await index.readMany(["Value", "Count"], options);
      assert.deepEqual(
        many.map(plain),
        mdf.readMany(["Value", "Count"], options).map(plain),
        `${label}: readMany(${JSON.stringify(options)})`,
      );
    }
    assert.deepEqual(await index.timeRange("Value"), mdf.timeRange("Value"), `${label}: timeRange`);
  }
});
