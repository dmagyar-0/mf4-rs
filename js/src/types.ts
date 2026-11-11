/**
 * The plain data types of the cross-language API contract, shared by `Mdf`
 * and `MdfIndex`. The Rust and Python APIs expose the same types with the
 * same fields in `snake_case` — see `docs/API.md`.
 */

/** A decoded channel value: numeric, a big integer, text, raw bytes, or invalid (`null`). */
export type ChannelValue = number | string | bigint | Uint8Array | null;

/** A decoded channel paired with its group's time axis. */
export interface Signal {
  /** Channel name. */
  name: string;
  /** Physical unit. */
  unit?: string | null;
  /**
   * Master-channel values (seconds relative to the measurement start for a
   * time master). Empty when the group has no master, or when this channel
   * *is* the master (its `values` are the time axis).
   */
  timestamps: Float64Array;
  /** One value per record, conversions applied (`null` = invalid sample). */
  values: ChannelValue[];
}

/** Metadata of one channel. */
export interface ChannelInfo {
  name?: string | null;
  unit?: string | null;
  /** Channel comment (`##TX` text, or the raw `##MD` XML). */
  comment?: string | null;
  /**
   * Canonical data-type name, e.g. `"FloatLE"`, `"UnsignedIntegerLE"`,
   * `"StringUtf8"` — the same names `MdfWriter.addChannel` takes.
   */
  dataType: string;
  /** Width of the stored value in bits. */
  bitCount: number;
  /** The channel is its group's master (time) channel. */
  isMaster: boolean;
  /** Variable-length (VLSD) channel, e.g. strings. */
  isVlsd: boolean;
  /** The channel carries a conversion (applied by `read` / `values`). */
  hasConversion: boolean;
}

/** Metadata of one channel group. */
export interface GroupInfo {
  name?: string | null;
  comment?: string | null;
  /** Number of records (samples per channel). */
  recordCount: number;
  /** Names of the group's named channels, in file order. */
  channelNames: string[];
  /** Name of the group's master (time) channel, local or MDF 4.2 remote. */
  masterChannel?: string | null;
  /** The group's channels, in file order. */
  channels: ChannelInfo[];
}

/**
 * Select part of a channel. Use **either** a time window (`startTime` /
 * `endTime`, inclusive, in the master channel's unit — seconds relative to the
 * measurement start for a time master) **or** a record window
 * (`startRecord` / `recordCount`). Omitted bounds are open.
 */
export interface ReadWindow {
  startTime?: number;
  endTime?: number;
  startRecord?: number;
  recordCount?: number;
}

/** Options accepted by `read` / `values`. */
export interface ReadOptions extends ReadWindow {
  /** Group name, to disambiguate a channel name used by several groups. */
  group?: string | null;
  /**
   * TypeScript only, `MdfIndex` only: read sample bytes from this source for
   * this one call instead of the index's bound source.
   */
  source?: import("./range-source").RangeSource;
}

/**
 * A channel's conversion (raw -> physical value), fully resolved.
 *
 * `type` is one of `Identity`, `Linear`, `Rational`, `Algebraic`,
 * `TableLookupInterp`, `TableLookupNoInterp`, `RangeLookup`, `ValueToText`,
 * `RangeToText`, `TextToValue`, `TextToText`, `BitfieldText`. `values` holds
 * the block's parameters (`cc_val`): e.g. `[offset, factor]` for `Linear`
 * (`phys = values[0] + values[1] * raw`), the six rational coefficients, or
 * the table keys. `texts` / `refs` are keyed by the position in the block's
 * reference list; how they pair with `values` depends on `type` (MDF 4.1
 * §6.17) — for `ValueToText`, `values[i]` maps to `texts[i]` (or the nested
 * `refs[i]`) and index `values.length` is the default.
 */
export interface ConversionInfo {
  type: string;
  values: number[];
  precision: number;
  flags: number;
  /** `[min, max]` physical range, when the block declares one. */
  physicalRange?: [number, number];
  /** Formula of an `Algebraic` conversion (variable `X`). */
  formula?: string;
  texts?: Record<string, string>;
  refs?: Record<string, ConversionInfo>;
  /** Default (fallback) conversion, when it is itself a conversion. */
  default?: ConversionInfo;
}

/** First and last master (time) value of a channel's group. */
export interface TimeRange {
  start: number;
  end: number;
}

/** A `[offset, length]` byte span into the source MDF file. */
export type ByteRange = [number, number];
