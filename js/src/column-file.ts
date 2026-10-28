import * as fs from "node:fs";
import * as path from "node:path";
import { toColumnOrientedStream, type ColumnOptions, type ColumnReport } from "./column";

/**
 * Convert the MDF file at `inputPath` into a column-oriented file at
 * `outputPath`, streaming with synchronous file I/O so neither file is loaded
 * into memory. Node only.
 */
export async function toColumnOrientedFile(
  inputPath: string,
  outputPath: string,
  options?: ColumnOptions,
): Promise<ColumnReport> {
  if (path.resolve(inputPath) === path.resolve(outputPath)) {
    throw new Error("toColumnOrientedFile: input and output must be different files");
  }
  const input = fs.openSync(inputPath, "r");
  try {
    const size = fs.fstatSync(input).size;
    const output = fs.openSync(outputPath, "w");
    try {
      return toColumnOrientedStream(
        {
          size,
          read(offset, length) {
            const buf = new Uint8Array(length);
            let done = 0;
            while (done < length) {
              const n = fs.readSync(input, buf, done, length - done, offset + done);
              if (n === 0) throw new Error(`unexpected end of ${inputPath} at ${offset + done}`);
              done += n;
            }
            return buf;
          },
        },
        (offset, bytes) => {
          let done = 0;
          while (done < bytes.length) {
            done += fs.writeSync(output, bytes, done, bytes.length - done, offset + done);
          }
        },
        options,
      );
    } finally {
      fs.closeSync(output);
    }
  } finally {
    fs.closeSync(input);
  }
}
