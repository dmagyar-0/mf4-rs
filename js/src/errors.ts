/**
 * Error raised by data operations (`read`, `values`, `timeRange`,
 * `conversion`, `byteRanges`) when the named channel does not exist.
 * Metadata lookups (`group`, `channel`) return `undefined` instead.
 */
export class ChannelNotFoundError extends Error {
  /** The channel name that was looked up. */
  readonly channel: string;
  /** The group the lookup was restricted to, if any. */
  readonly group: string | undefined;

  constructor(channel: string, group?: string | null) {
    super(
      group == null
        ? `Channel '${channel}' not found`
        : `Channel '${channel}' not found in group '${group}'`,
    );
    this.name = "ChannelNotFoundError";
    this.channel = channel;
    this.group = group ?? undefined;
  }
}

const NOT_FOUND = /^Channel '(.*)' not found(?: in group '(.*)')?$/s;

/**
 * Run a wasm call, turning the core library's "channel not found" error into
 * a typed {@link ChannelNotFoundError}.
 */
export function guard<T>(call: () => T): T {
  try {
    return call();
  } catch (e) {
    const match = e instanceof Error ? NOT_FOUND.exec(e.message) : null;
    if (match) {
      throw new ChannelNotFoundError(match[1]!, match[2]);
    }
    throw e;
  }
}
