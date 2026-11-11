import { getWasmModule, type WasmMdfWriter } from "./wasm-module";

/**
 * A single record value: `number` for numeric channels, `string` for string
 * channels, `bigint` for integer channels whose value exceeds
 * `Number.MAX_SAFE_INTEGER`, or `Uint8Array` for byte-array channels.
 */
export type RecordValue = number | string | bigint | Uint8Array;

/** One channel's values for `writeColumns`, typed to match the channel. */
export type ColumnValues = Float64Array | Float32Array | BigUint64Array | BigInt64Array;

/**
 * Streaming writer that produces an MDF 4 file entirely in memory.
 *
 * The same writer API as Rust and Python (see `docs/API.md`):
 * `initMdfFile()`, then define structure (`addChannelGroup`,
 * `addTimeChannel`, `addFloatChannel`, …, conversions), then write data
 * (`startDataBlock`, `writeRecord` / `writeColumns`, `finishDataBlock`), then
 * `finalize()` — which here returns the file as a `Uint8Array`, since wasm
 * has no filesystem. Group/channel IDs are opaque strings — pass them back
 * to later calls.
 */
export class MdfWriter {
  private readonly inner: WasmMdfWriter;

  constructor() {
    const wasm = getWasmModule();
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

  /** Set a channel group's comment. */
  setChannelGroupComment(groupId: string, comment: string): void {
    this.inner.setChannelGroupComment(groupId, comment);
  }

  /**
   * Add a channel using the data type's default bit width. `dataType` is a
   * canonical name as reported by `ChannelInfo.dataType`: `"FloatLE"`,
   * `"UnsignedIntegerLE"`, `"SignedIntegerLE"`, `"StringUtf8"`, `"ByteArray"`,
   * … (float defaults to 32 bits — use `addFloatChannel` for f64). Text
   * types are written as variable-length channels.
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

  /**
   * Attach a linear conversion to a channel: readers return
   * `offset + factor * raw`. Call before `startDataBlock`.
   */
  addLinearConversion(channelId: string, offset: number, factor: number): void {
    this.inner.addLinearConversion(channelId, offset, factor);
  }

  /**
   * Attach a value-to-text conversion to an integer channel: readers return
   * the text mapped to each raw value, or `defaultText` for unmapped values.
   * Call before `startDataBlock`.
   */
  addValueToTextConversion(
    channelId: string,
    mapping: Array<[number, string]>,
    defaultText: string,
  ): void {
    this.inner.addValueToTextConversion(channelId, mapping, defaultText);
  }

  /** Open a fresh `##DT` data block for a channel group. */
  startDataBlock(groupId: string): void {
    this.inner.startDataBlock(groupId);
  }

  /** Append a single record: one value per channel, in the order channels were added. */
  writeRecord(groupId: string, values: RecordValue[]): void {
    this.inner.writeRecord(groupId, values);
  }

  /**
   * Append many records at once from one typed array per channel (the fast
   * path): `Float64Array`, `Float32Array`, `BigUint64Array` or
   * `BigInt64Array` matching each channel, all of the same length.
   */
  writeColumns(groupId: string, columns: ColumnValues[]): void {
    this.inner.writeColumns(groupId, columns);
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
