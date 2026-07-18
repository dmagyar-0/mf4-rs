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

/** Metadata for a channel group, as returned by `MdfIndex.groups()` (lighter than `GroupInfo`). */
export interface IndexGroupInfo {
  name?: string | null;
  recordCount: number;
  channelNames: string[];
  masterChannel?: string | null;
}

/** A `[offset, length]` byte span into the source MDF file. */
export type ByteRange = [number, number];
