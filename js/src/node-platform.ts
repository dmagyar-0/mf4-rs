/**
 * Node-only side of `platform.ts`: registers the `fs`-backed file reader and writer.
 * Imported only by the Node entry point (`index.ts`), never by `web.ts`.
 */

import { readFile, writeFile } from "node:fs/promises";
import { registerReadFile, registerWriteFile } from "./platform";

registerReadFile(async (path) => {
  const buf = await readFile(path);
  return new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
});

registerWriteFile((path, data) => writeFile(path, data));
