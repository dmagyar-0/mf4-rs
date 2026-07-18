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
  const direct = mdf.values("Value", "Group1");
  mdf.dispose();

  const { server, url } = await startServer(bytes, true);
  try {
    const index = MdfIndex.fromBytes(bytes);
    const source = new FetchRangeSource(url);
    const lazy = await index.values("Value", source, "Group1");
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
  const direct = mdf.read("Value", "Group1");
  mdf.dispose();

  const { server, url } = await startServer(bytes, false);
  try {
    const index = MdfIndex.fromBytes(bytes);
    const source = new FetchRangeSource(url);
    const lazy = await index.read("Value", source, "Group1");
    assert.equal(lazy.name, direct.name);
    assert.deepEqual(Array.from(lazy.timestamps), Array.from(direct.timestamps));
    assert.deepEqual(lazy.values, direct.values);
    index.dispose();
  } finally {
    await closeServer(server);
  }
});
