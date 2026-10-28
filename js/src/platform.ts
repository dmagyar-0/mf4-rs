/**
 * Hooks for the few Node-only conveniences (`Mdf.fromFile`,
 * `MdfIndex.fromFile`) used by classes shared with the browser entry.
 *
 * This module must never import a `node:` built-in: the shared classes are
 * bundled into `mf4-rs/web`, and bundlers such as webpack 5 fail on any
 * `node:` specifier they can see, even inside a dynamic `import()`. The Node
 * entry (`index.ts`) registers the real implementation via `node-platform.ts`.
 */

type ReadFile = (path: string) => Promise<Uint8Array>;

let readFileImpl: ReadFile | undefined;

/** Install the Node file reader (done by the `mf4-rs` Node entry point). */
export function registerReadFile(fn: ReadFile): void {
  readFileImpl = fn;
}

/** Read a whole file through the registered Node reader. */
export function readFileNodeOnly(path: string, caller: string): Promise<Uint8Array> {
  if (!readFileImpl) {
    throw new Error(
      `${caller} is only available from the Node entry point ('mf4-rs'). In the ` +
        "browser, obtain the bytes yourself and use fromBytes(), or read lazily " +
        "through a RangeSource (MdfIndex.fromUrl / MdfIndex.fromRangeSource).",
    );
  }
  return readFileImpl(path);
}
