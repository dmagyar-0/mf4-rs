/**
 * Public data-transfer types shared by `Mdf` and `MdfIndex`.
 */

/** A decoded channel value: numeric, a big integer, text, raw bytes, or invalid (`null`). */
export type ChannelValue = number | string | bigint | Uint8Array | null;

/** A decoded channel paired with its group's time axis. */
export interface Signal {
  name: string;
  unit?: string | null;
  timestamps: Float64Array;
  values: ChannelValue[];
}

/** Metadata for a single channel within a group. */
export interface ChannelInfo {
  name: string;
  unit?: string | null;
  comment?: string | null;
  dataType: string;
  isMaster: boolean;
  bitCount: number;
}

/** Metadata for a channel group, as returned by `Mdf.groups()`. */
export interface GroupInfo {
  name?: string | null;
  comment?: string | null;
  recordCount: number;
  channels: ChannelInfo[];
}

/** Metadata for a single channel, as returned by `MdfIndex.groups()` / `MdfIndex.channel()`. */
export interface IndexChannelInfo {
  name?: string | null;
  unit?: string | null;
  /** Channel comment (`##TX` text, or the raw `##MD` XML). */
  comment?: string | null;
  /** MDF data type, e.g. `"FloatLE"`, `"UnsignedIntegerLE"`, `"StringUtf8"`. */
  dataType: string;
  isMaster: boolean;
  bitCount: number;
  /** Variable-length (VLSD) channel, e.g. strings. */
  isVlsd: boolean;
  /** The channel carries a conversion (applied by `values`/`read`). */
  hasConversion: boolean;
}

/** Metadata for a channel group, as returned by `MdfIndex.groups()`. */
export interface IndexGroupInfo {
  name?: string | null;
  comment?: string | null;
  recordCount: number;
  channelNames: string[];
  /** Name of the group's master (time) channel, local or MDF 4.2 remote. */
  masterChannel?: string | null;
  channels: IndexChannelInfo[];
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

/** Options accepted by `values` / `read`. */
export interface ReadOptions extends ReadWindow {
  /** Group name, to disambiguate a channel name used by several groups. */
  group?: string | null;
  /** Where to read sample bytes from (`MdfIndex` only). Defaults to the
   * index's bound source. */
  source?: import("./range-source").RangeSource;
}

/** First and last master (time) value of a channel's group. */
export interface TimeRange {
  start: number;
  end: number;
}

/** A `[offset, length]` byte span into the source MDF file. */
export type ByteRange = [number, number];
