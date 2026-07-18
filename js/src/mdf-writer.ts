import { loadWasmModule, type WasmMdfWriter } from "./wasm-module";

/**
 * A single record value: `number` for numeric channels, `string` for string
 * channels, or `bigint` for integer channels whose value exceeds
 * `Number.MAX_SAFE_INTEGER`.
 */
export type RecordValue = number | string | bigint;

/**
 * Streaming writer that produces an MDF 4 file entirely in memory.
 *
 * Mirrors the Python `MdfWriter`: `initMdfFile()`, then define structure
 * (`addChannelGroup`, `addTimeChannel`, `addFloatChannel`, `addIntChannel`,
 * `addChannel`, `setTimeChannel`), then write data (`startDataBlock`,
 * `writeRecord`, `finishDataBlock`), then `finalize()` to get a
 * `Uint8Array`. Group/channel IDs are opaque strings — pass them back to
 * later calls.
 */
export class MdfWriter {
  private readonly inner: WasmMdfWriter;

  constructor() {
    const wasm = loadWasmModule();
    this.inner = new wasm.MdfWriter();
  }

  /** Write the identification (`##ID`) and header (`##HD`) blocks. Call once, before adding any channel group. */
  initMdfFile(): void {
    this.inner.initMdfFile();
  }

  /** Set the measurement start time (header `abs_time`) in nanoseconds since the Unix epoch. */
  setStartTime(absTimeNs: bigint): void {
    this.inner.setStartTime(absTimeNs);
  }

  /** Append a new channel group. Returns an opaque group ID. */
  addChannelGroup(name?: string | null): string {
    return this.inner.addChannelGroup(name);
  }

  /**
   * Add a generic channel using the data type's natural bit width.
   * `dataType` is a symbolic name such as `"FloatLE"`, `"UnsignedIntegerLE"`,
   * `"SignedIntegerLE"`, `"StringUtf8"` (float defaults to 32 bits — use
   * `addFloatChannel` for f64).
   */
  addChannel(groupId: string, name: string, dataType: string): string {
    return this.inner.addChannel(groupId, name, dataType);
  }

  /** Add a 64-bit float channel and mark it as the group's master/time channel. */
  addTimeChannel(groupId: string, name: string): string {
    return this.inner.addTimeChannel(groupId, name);
  }

  /** Add a 64-bit little-endian float (`f64`) data channel. */
  addFloatChannel(groupId: string, name: string): string {
    return this.inner.addFloatChannel(groupId, name);
  }

  /** Add a 32-bit little-endian float (`f32`) data channel. */
  addFloat32Channel(groupId: string, name: string): string {
    return this.inner.addFloat32Channel(groupId, name);
  }

  /** Add a 64-bit little-endian unsigned integer (`u64`) data channel. */
  addIntChannel(groupId: string, name: string): string {
    return this.inner.addIntChannel(groupId, name);
  }

  /** Add a 64-bit little-endian signed integer (`i64`) data channel. */
  addSintChannel(groupId: string, name: string): string {
    return this.inner.addSintChannel(groupId, name);
  }

  /**
   * Add a variable-length (VLSD) UTF-8 string channel. Pass JS `string`
   * values for this channel to `writeRecord`.
   */
  addStringChannel(groupId: string, name: string): string {
    return this.inner.addStringChannel(groupId, name);
  }

  /** Mark an existing channel as the group's master/time channel. */
  setTimeChannel(channelId: string): void {
    this.inner.setTimeChannel(channelId);
  }

  /** Open a fresh `##DT` data block for a channel group. */
  startDataBlock(groupId: string): void {
    this.inner.startDataBlock(groupId);
  }

  /** Append a single record: one value per channel, in the order channels were added. */
  writeRecord(groupId: string, values: RecordValue[]): void {
    this.inner.writeRecord(groupId, values);
  }

  /** Close the open `##DT` block for a channel group. Call once per group after all records. */
  finishDataBlock(groupId: string): void {
    this.inner.finishDataBlock(groupId);
  }

  /**
   * Flush the writer and return the finished MDF file as a `Uint8Array`.
   * After this the writer is consumed; any further call raises.
   */
  finalize(): Uint8Array {
    return this.inner.finalize();
  }

  /** Free the underlying wasm memory. Safe to call multiple times. */
  dispose(): void {
    this.inner.free();
  }

  /** `using`/`Symbol.dispose` support: frees the underlying wasm memory. */
  [Symbol.dispose](): void {
    this.dispose();
  }
}
