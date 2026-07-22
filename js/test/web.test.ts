import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import * as http from "node:http";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import type { AddressInfo } from "node:net";

import { buildSampleFile } from "./helpers";

/**
 * Exercises the browser (`mf4-rs/web`) entry point: the async `init()`, the
 * shared reader/writer/index classes over the `--target web` wasm build, and
 * the `MdfIndex.fromUrl` HTTP convenience constructor. Runs against the
 * compiled ESM output in `dist-web/` (produced by `npm run build:web`), so it
 * skips cleanly when that build hasn't been produced yet.
 */

// dist/test/web.test.js -> ../../dist-web and ../../pkg-web
const distWeb = path.join(__dirname, "../../dist-web/web.js");
const wasmPath = path.join(__dirname, "../../pkg-web/mf4_rs_bg.wasm");
const webBuilt = existsSync(distWeb) && existsSync(wasmPath);

// Preserve a real ESM `import()` in the CommonJS test output — `tsc` would
// otherwise downlevel `import()` to `require()`, which cannot load ESM.
const dynamicImport = new Function("specifier", "return import(specifier)") as (
  specifier: string,
) => Promise<Record<string, any>>;

test(
  "web entry: init(), writer/reader roundtrip, and MdfIndex.fromUrl",
  { skip: webBuilt ? false : "web build not present (run `npm run build:web`)" },
  async () => {
    const web = await dynamicImport(pathToFileURL(distWeb).href);
    const { init, Mdf, MdfIndex, BytesRangeSource, FetchRangeSource } = web;

    // Feed the wasm bytes directly — Node's fetch cannot load a file:// URL.
    await init({ module_or_path: readFileSync(wasmPath) });

    const { bytes, values } = buildSampleFile(20);

    // Reader roundtrip through the web build.
    const mdf = Mdf.fromBytes(bytes);
    assert.deepEqual(Array.from(mdf.values("Value")), values);
    mdf.dispose();

    // Index built from bytes, read lazily via an in-memory RangeSource.
    const idx = MdfIndex.fromBytes(bytes);
    const local = await idx.values("Value", new BytesRangeSource(bytes));
    assert.deepEqual(Array.from(local), values);
    idx.dispose();

    // Index built from a URL (downloads in full), then lazy reads over HTTP.
    const server = http.createServer((_req, res) => {
      res.writeHead(200, { "Content-Type": "application/octet-stream" });
      res.end(Buffer.from(bytes));
    });
    await new Promise<void>((resolve) => server.listen(0, resolve));
    try {
      const url = `http://127.0.0.1:${(server.address() as AddressInfo).port}/file.mf4`;
      const urlIdx = await MdfIndex.fromUrl(url);
      const remote = await urlIdx.values("Value", new FetchRangeSource(url));
      assert.deepEqual(Array.from(remote), values);
      urlIdx.dispose();
    } finally {
      server.close();
    }
  },
);
