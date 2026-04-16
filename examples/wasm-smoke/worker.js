// Web Worker: imports the wasm-bindgen output and calls open_from_bytes.
// Build with: wasm-pack build --target web examples/wasm-smoke
// Then serve the project root with any static file server.

import init, { open_from_bytes } from './pkg/wasm_smoke.js';

let wasmReady = false;

async function ensureWasm() {
  if (!wasmReady) {
    await init();
    wasmReady = true;
  }
}

self.onmessage = async ({ data: { buf, filename, t0 } }) => {
  try {
    await ensureWasm();
    const bytes = new Uint8Array(buf);
    const t1 = performance.now();
    const summary = open_from_bytes(bytes);
    const t2 = performance.now();
    self.postMessage({
      ok: true,
      filename,
      time_to_parse_ms: (t2 - t1).toFixed(1),
      time_total_ms: (t2 - t0).toFixed(1),
      ...summary,
    });
  } catch (err) {
    self.postMessage({ ok: false, error: String(err) });
  }
};
