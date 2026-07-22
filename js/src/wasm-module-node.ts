/**
 * Node-only loader for the synchronous `pkg-node` wasm-bindgen build.
 *
 * The `--target nodejs` glue instantiates the wasm module synchronously at
 * `require()` time, which is what lets the Node API stay synchronous
 * (`Mdf.fromBytes(...)` and friends need no `await`). This file is imported
 * only by the Node entry point (`index.ts`); the browser entry (`web.ts`)
 * never touches it, so the `require("../../pkg-node/...")` below is never
 * pulled into a web bundle.
 *
 * Compiled to `dist/src/wasm-module-node.js`, two directories below `js/`,
 * so the relative path back up to `js/pkg-node/mf4_rs.js` is
 * `../../pkg-node/mf4_rs.js`.
 */

import { registerWasmLoader, getWasmModule, type WasmModule } from "./wasm-module";

function nodeLoad(): WasmModule {
  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    return require("../../pkg-node/mf4_rs.js") as WasmModule;
  } catch (err) {
    throw new Error(
      "Failed to load the mf4-rs wasm module from js/pkg-node. " +
        "Build it first with `npm run build:wasm` (requires wasm-pack). " +
        `Original error: ${(err as Error).message}`,
    );
  }
}

// Register the lazy loader as a side effect of importing this module so that
// the synchronous Node API can resolve the wasm module on first use.
registerWasmLoader(nodeLoad);

/**
 * Load (and cache) the generated `pkg-node` wasm-bindgen module.
 *
 * Throws a friendly error if `pkg-node` hasn't been built yet. Retained for
 * backwards compatibility; the class constructors resolve the module lazily
 * via `getWasmModule()` on their own.
 */
export function loadWasmModule(): WasmModule {
  return getWasmModule();
}
