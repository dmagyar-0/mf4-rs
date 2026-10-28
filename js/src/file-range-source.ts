import type { RangeSource } from "./range-source";
import { open, type FileHandle } from "node:fs/promises";

/**
 * `RangeSource` backed by a local file, read via `fs/promises`.
 *
 * Node-only.
 */
export class FileRangeSource implements RangeSource {
  private readonly path: string;
  private handlePromise: Promise<FileHandle> | undefined;

  constructor(path: string) {
    this.path = path;
  }

  private async handle(): Promise<FileHandle> {
    if (!this.handlePromise) {
      this.handlePromise = open(this.path, "r");
    }
    return this.handlePromise;
  }

  async size(): Promise<number> {
    const fh = await this.handle();
    const stat = await fh.stat();
    return stat.size;
  }

  async read(offset: number, length: number): Promise<Uint8Array> {
    if (length <= 0) {
      return new Uint8Array(0);
    }
    const fh = await this.handle();
    const buffer = Buffer.alloc(length);
    // A single fh.read may return fewer bytes than requested; loop until the
    // whole span is read, treating a 0-byte read as EOF.
    let total = 0;
    while (total < length) {
      const { bytesRead } = await fh.read(buffer, total, length - total, offset + total);
      if (bytesRead === 0) {
        throw new Error(
          `FileRangeSource: short read at offset ${offset + total} — wanted ${length - total} ` +
            `more byte(s) but hit EOF (requested [${offset}, ${offset + length}))`,
        );
      }
      total += bytesRead;
    }
    return new Uint8Array(buffer.buffer, buffer.byteOffset, length);
  }

  /** Close the underlying file handle, if opened. */
  async close(): Promise<void> {
    if (this.handlePromise) {
      const fh = await this.handlePromise;
      await fh.close();
      this.handlePromise = undefined;
    }
  }
}

