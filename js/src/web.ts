/**
 * mf4-rs — browser entry point (`mf4-rs/web`).
 *
 * Uses the `wasm-pack --target web` build (`js/pkg-web/`), which instantiates
 * the wasm module asynchronously. Because browser wasm instantiation cannot be
 * synchronous, you must `await init()` **once** before constructing `Mdf`,
 * `MdfIndex`, or `MdfWriter`:
 *
 * ```ts
 * import { init, Mdf } from "mf4-rs/web";
 *
 * await init();                 // fetches mf4_rs_bg.wasm next to the module
 * const mdf = Mdf.fromBytes(bytes);
 * ```
 *
 * `init()` accepts the same argument as the underlying wasm-bindgen loader —
 * pass a URL, `Response`, `BufferSource`, or `WebAssembly.Module` to control
 * where the `.wasm` binary comes from (useful behind CDNs or strict CSPs). It
 * is idempotent: repeated calls return the same in-flight/settled promise.
 *
 * Once initialised, the `Mdf` / `MdfIndex` / `MdfWriter` classes and the
 * `RangeSource` helpers behave exactly as in the Node entry point.
 */

// The `pkg-web` build is a gitignored artifact produced by
// `npm run build:wasm:web`; its types exist only after that runs.
import __wbg_init, * as wasmNamespace from "../pkg-web/mf4_rs.js";
import { setWasmModule, type WasmModule } from "./wasm-module";

/** Argument accepted by {@link init}, mirroring the generated wasm loader. */
export type InitInput = Parameters<typeof __wbg_init>[0];

let initPromise: Promise<void> | undefined;

/**
 * Instantiate the wasm module. Call once (and `await` it) before using any
 * of the reader/writer/index classes. Idempotent.
 *
 * @param input Optional override for the `.wasm` source (URL / `Response` /
 *   `BufferSource` / `WebAssembly.Module`). Omit to load `mf4_rs_bg.wasm`
 *   resolved relative to this module.
 */
export function init(input?: InitInput): Promise<void> {
  if (!initPromise) {
    initPromise = Promise.resolve(__wbg_init(input)).then(() => {
      setWasmModule(wasmNamespace as unknown as WasmModule);
    });
  }
  return initPromise;
}

export { Mdf } from "./mdf";
export { MdfIndex } from "./mdf-index";
export { MdfWriter, type RecordValue } from "./mdf-writer";

export {
  FetchRangeSource,
  BytesRangeSource,
  readAllRanges,
  type RangeSource,
  type FetchLike,
} from "./range-source";

export { mergeByteRanges } from "./byte-ranges";

export type {
  ChannelValue,
  Signal,
  ChannelInfo,
  GroupInfo,
  IndexGroupInfo,
  ByteRange,
} from "./types";
