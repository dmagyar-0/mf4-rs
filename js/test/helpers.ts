// Register the Node wasm loader (the `.` package entry does this for real
// consumers; tests import the wrapper classes from source directly).
import "../src/wasm-module-node";
import { MdfWriter } from "../src/mdf-writer";

/**
 * Build a small MDF file in memory: one group, a `Time` master channel, a
 * `Value` float channel, and a `Count` integer channel, over `recordCount`
 * records. Shared by several test files.
 */
export function buildSampleFile(recordCount = 100): {
  bytes: Uint8Array;
  times: number[];
  values: number[];
  counts: number[];
  startTimeNs: bigint;
} {
  const writer = new MdfWriter();
  writer.initMdfFile();
  const startTimeNs = 1_700_000_000_000_000_000n;
  writer.setStartTime(startTimeNs);

  const groupId = writer.addChannelGroup("Group1");
  const timeId = writer.addTimeChannel(groupId, "Time");
  writer.addFloatChannel(groupId, "Value");
  writer.addIntChannel(groupId, "Count");
  writer.setTimeChannel(timeId);

  const times: number[] = [];
  const values: number[] = [];
  const counts: number[] = [];

  writer.startDataBlock(groupId);
  for (let i = 0; i < recordCount; i++) {
    const t = i * 0.01;
    const v = Math.sin(i * 0.1);
    times.push(t);
    values.push(v);
    counts.push(i);
    writer.writeRecord(groupId, [t, v, i]);
  }
  writer.finishDataBlock(groupId);

  const bytes = writer.finalize();
  return { bytes, times, values, counts, startTimeNs };
}
