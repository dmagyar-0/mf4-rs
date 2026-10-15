import { getWasmModule } from "./wasm-module";

/**
 * Tuning for the column-oriented conversion. All sizes are in bytes; omitted
 * fields keep the defaults.
 */
export interface ColumnOptions {
  /** Target payload size of one data block. A signal larger than this is
   * split into several equally sized blocks (whole records each) written back
   * to back. Default 4 MiB. */
  blockSize?: number;
  /** Upper bound for the per-channel write buffers. Default 64 MiB. */
  memoryBudget?: number;
  /** Size of each sequential read from the source. Default 4 MiB. */
  readChunkSize?: number;
  /** Merge byte-identical time axes of different source groups into one
   * time channel group. Default `true`. */
  shareIdenticalTime?: boolean;
}

/** Summary of a column-oriented conversion. */
export interface ColumnReport {
  /** Channel groups found in the source. */
  sourceGroups: number;
  /** Time (master) channel groups written. */
  timeGroups: number;
  /** Signal channel groups written (one per signal). */
  signalGroups: number;
  /** Source groups whose time axis was identical to an earlier group's and
   * therefore share that group's time channel. */
  sharedTimeGroups: number;
  /** Data blocks (`##DV`/`##DT`/`##SD`) written. */
  dataBlocks: number;
  /** Byte offset where sample data starts: fetching this prefix is enough to
   * build an index of the file. */
  metadataSize: number;
  /** Output size in bytes. */
  fileSize: number;
  /** Channels that could not be represented as a column
   * (`"group/channel: reason"`). */
  skippedChannels: string[];
}

/** Synchronous random-access byte source for {@link toColumnOrientedStream}. */
export interface SyncByteSource {
  /** Total size in bytes. */
  size: number;
  /** Return exactly `length` bytes starting at `offset`. */
  read(offset: number, length: number): Uint8Array;
}

/** Synchronous positional writer for {@link toColumnOrientedStream}: store
 * `bytes` at absolute output position `offset`. Positions never written must
 * read back as zero (true for a fresh file). */
export type SyncByteSink = (offset: number, bytes: Uint8Array) => void;

/**
 * Convert an MDF 4 file into a **column-oriented MDF 4.20** file optimised for
 * reading single signals from cloud storage.
 *
 * Every signal gets its own channel group whose data blocks sit back to back
 * (one range request per signal); timestamps are written once per source
 * group and shared via the MDF 4.2 remote-master link; all metadata sits at
 * the front of the file, so `MdfIndex.fromUrl` builds from a single prefix
 * fetch. Raw bytes are copied — conversions, units, comments, invalidation
 * bits and VLSD channels carry over unchanged.
 *
 * Input and output are held in memory; for large files use
 * {@link toColumnOrientedStream} or (Node) {@link toColumnOrientedFile}.
 */
export function toColumnOriented(
  data: Uint8Array,
  options?: ColumnOptions,
): { bytes: Uint8Array; report: ColumnReport } {
  const { bytes, report } = getWasmModule().toColumnOriented(data, options ?? {});
  return { bytes, report: report as ColumnReport };
}

/**
 * Streaming variant of {@link toColumnOriented}: reads the source through
 * `source.read` (sequentially, in `readChunkSize` pieces) and writes the
 * output through `sink` at computed absolute offsets, so memory stays bounded
 * by `memoryBudget` whatever the file size.
 *
 * Both callbacks must be synchronous — e.g. Node's `fs.readSync` /
 * `fs.writeSync`, or an OPFS `FileSystemSyncAccessHandle` in a browser worker.
 */
export function toColumnOrientedStream(
  source: SyncByteSource,
  sink: SyncByteSink,
  options?: ColumnOptions,
): ColumnReport {
  if (!Number.isSafeInteger(source.size) || source.size < 0) {
    throw new RangeError(`source.size must be a non-negative safe integer, got ${source.size}`);
  }
  return getWasmModule().toColumnOrientedStream(
    source.size,
    (offset, length) => source.read(offset, length),
    (offset, bytes) => sink(offset, bytes),
    options ?? {},
  ) as ColumnReport;
}

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
  const fs = await import("node:fs");
  const path = await import("node:path");
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
