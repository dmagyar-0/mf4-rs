import { test } from "node:test";
import assert from "node:assert/strict";
import * as http from "node:http";
import type { AddressInfo } from "node:net";

import { Mdf } from "../src/mdf";
import { MdfIndex } from "../src/mdf-index";
import { FetchRangeSource } from "../src/range-source";
import { buildSampleFile } from "./helpers";

/** Start an HTTP server serving `bytes`, honouring `Range` requests when `honourRange` is true. */
function startServer(
  bytes: Uint8Array,
  honourRange: boolean,
): Promise<{ server: http.Server; url: string }> {
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const rangeHeader = req.headers.range;
      if (honourRange && rangeHeader) {
        const match = /^bytes=(\d+)-(\d+)$/.exec(rangeHeader);
        if (match) {
          const start = Number(match[1]);
          const end = Number(match[2]);
          const chunk = bytes.subarray(start, end + 1);
          res.writeHead(206, {
            "Content-Type": "application/octet-stream",
            "Content-Range": `bytes ${start}-${end}/${bytes.length}`,
            "Content-Length": chunk.length,
          });
          res.end(Buffer.from(chunk));
          return;
        }
      }
      // Ignore Range: always send the full body with 200.
      res.writeHead(200, {
        "Content-Type": "application/octet-stream",
        "Content-Length": bytes.length,
      });
      res.end(Buffer.from(bytes));
    });
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address() as AddressInfo;
      resolve({ server, url: `http://127.0.0.1:${addr.port}/file.mf4` });
    });
  });
}

function closeServer(server: http.Server): Promise<void> {
  return new Promise((resolve, reject) => {
    server.close((err) => (err ? reject(err) : resolve()));
  });
}

test("FetchRangeSource over a Range-honouring server matches direct read", async () => {
  const { bytes } = buildSampleFile(60);
  const mdf = Mdf.fromBytes(bytes);
  const direct = mdf.values("Value", { group: "Group1" });
  mdf.dispose();

  const { server, url } = await startServer(bytes, true);
  try {
    const index = MdfIndex.fromBytes(bytes);
    const source = new FetchRangeSource(url);
    const lazy = await index.values("Value", { source: source, group: "Group1" });
    assert.equal(lazy.length, direct.length);
    for (let i = 0; i < direct.length; i++) {
      assert.equal(lazy[i], direct[i]);
    }
    assert.equal(await source.size(), bytes.length);
    index.dispose();
  } finally {
    await closeServer(server);
  }
});

test("FetchRangeSource over a Range-ignoring (200-only) server matches direct read", async () => {
  const { bytes } = buildSampleFile(60);
  const mdf = Mdf.fromBytes(bytes);
  const direct = mdf.read("Value", { group: "Group1" });
  mdf.dispose();

  const { server, url } = await startServer(bytes, false);
  try {
    const index = MdfIndex.fromBytes(bytes);
    const source = new FetchRangeSource(url);
    const lazy = await index.read("Value", { source: source, group: "Group1" });
    assert.equal(lazy.name, direct.name);
    assert.deepEqual(Array.from(lazy.timestamps), Array.from(direct.timestamps));
    assert.deepEqual(lazy.values, direct.values);
    index.dispose();
  } finally {
    await closeServer(server);
  }
});

test("MdfIndex refuses a URL whose file no longer matches the index", async () => {
  const original = buildSampleFile(60).bytes;
  const replaced = buildSampleFile(200).bytes;
  const { server, url } = await startServer(replaced, true);
  try {
    const index = MdfIndex.fromBytes(original);
    index.setSource(new FetchRangeSource(url));
    for (const read of [
      () => index.values("Value", { group: "Group1" }),
      () => index.read("Value", { group: "Group1", startTime: 0.1, endTime: 0.2 }),
      () => index.timeRange("Value", "Group1"),
    ]) {
      await assert.rejects(read(), /does not match the index/);
    }
    index.dispose();
  } finally {
    await closeServer(server);
  }
});

test("FetchRangeSource refuses to download a large file whole when Range is ignored", async () => {
  const { bytes } = buildSampleFile(60);
  const { server, url } = await startServer(bytes, false);
  try {
    // Limit below the file size: the 200 body is refused before it is read.
    const strict = new FetchRangeSource(url, undefined, { maxFullDownloadBytes: bytes.length - 1 });
    await assert.rejects(strict.read(100, 10), /ignored the Range header/);
    await assert.rejects(strict.size(), /ignored the Range header/);
    // Within the limit it still works by slicing the cached body.
    const lenient = new FetchRangeSource(url, undefined, { maxFullDownloadBytes: bytes.length });
    assert.deepEqual(await lenient.read(100, 10), bytes.subarray(100, 110));
  } finally {
    await closeServer(server);
  }
});

test("Mdf and MdfIndex refuse a time window on a time axis that resets", async () => {
  const { MdfWriter } = await import("../src/mdf-writer");
  const writer = new MdfWriter();
  writer.initMdfFile();
  const g = writer.addChannelGroup("G");
  const t = writer.addTimeChannel(g, "t");
  writer.addFloatChannel(g, "X");
  writer.setTimeChannel(t);
  writer.startDataBlock(g);
  for (let i = 0; i < 2000; i++) writer.writeRecord(g, [(i % 1000) * 0.01, i]);
  writer.finishDataBlock(g);
  const bytes = writer.finalize();
  const mdf = Mdf.fromBytes(bytes);
  const index = MdfIndex.fromBytes(bytes);
  assert.throws(() => mdf.read("X", { startTime: 2, endTime: 3 }), /not monotonic/);
  await assert.rejects(index.read("X", { startTime: 2, endTime: 3 }), /not monotonic/);
  await assert.rejects(index.values("X", { startTime: 2, endTime: 3 }), /not monotonic/);
  assert.deepEqual(
    (await index.read("X", { startRecord: 1200, recordCount: 2 })).values,
    mdf.read("X", { startRecord: 1200, recordCount: 2 }).values,
  );
  mdf.dispose();
  index.dispose();
});

test("FetchRangeSource enforces the full-download cap on a chunked body without Content-Length", async () => {
  const { bytes } = buildSampleFile(60);
  const server = http.createServer((_req, res) => {
    // Ignore Range and stream the body chunked (no Content-Length).
    res.writeHead(200, { "Content-Type": "application/octet-stream" });
    const step = 256;
    for (let i = 0; i < bytes.length; i += step) res.write(Buffer.from(bytes.subarray(i, i + step)));
    res.end();
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/file.mf4`;
  try {
    const strict = new FetchRangeSource(url, undefined, { maxFullDownloadBytes: 1024 });
    await assert.rejects(strict.read(100, 10), /download the whole file \(more than \d+ bytes/);
    const lenient = new FetchRangeSource(url);
    assert.deepEqual(await lenient.read(100, 10), bytes.subarray(100, 110));
  } finally {
    await closeServer(server);
  }
});
