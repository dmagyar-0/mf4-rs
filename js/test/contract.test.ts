/**
 * Guards the cross-language API contract (`docs/api-contract.json`).
 *
 * The public names of the TypeScript package must be exactly what the
 * contract lists (camelCased): adding, renaming or removing a class member,
 * a type field or an export fails this test until the contract — and with it
 * the Rust and Python bindings, which have the same test — is updated.
 *
 * Also checks that `Mdf` and `MdfIndex` answer every contract call
 * identically for the same file.
 */

import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import * as api from "../src/index";
import {
  BytesRangeSource,
  ChannelNotFoundError,
  Mdf,
  MdfIndex,
  MdfWriter,
  type ReadOptions,
  type Signal,
} from "../src/index";

interface ClassSpec {
  constructors: string[];
  accessors: string[];
  methods: string[];
  only: Record<string, string[]>;
  extra: Record<string, string[]>;
}

const contract = JSON.parse(
  readFileSync(join(__dirname, "..", "..", "..", "docs", "api-contract.json"), "utf8"),
) as {
  classes: Record<string, ClassSpec>;
  types: Record<string, { fields: string[] }>;
  functions: Record<string, string[]>;
  module_exports: Record<string, string[]>;
};

const LANG = "typescript";
const camel = (name: string): string => name.replace(/_(\w)/g, (_, c: string) => c.toUpperCase());
const sorted = (names: Iterable<string>): string[] => [...names].sort();

function available(spec: ClassSpec, names: string[]): string[] {
  return names.filter((n) => !spec.only[n] || spec.only[n]!.includes(LANG)).map(camel);
}

test("class members match the contract", () => {
  const classes: Record<string, { prototype: object }> = { Mdf, MdfIndex, MdfWriter };
  for (const [name, spec] of Object.entries(contract.classes)) {
    const cls = classes[name]!;
    const extra = spec.extra[LANG]!.map(camel);
    const statics = Object.getOwnPropertyNames(cls).filter(
      (n) => !["length", "name", "prototype"].includes(n),
    );
    const members = Object.getOwnPropertyNames(cls.prototype).filter((n) => n !== "constructor");
    const expectedStatics = available(spec, spec.constructors);
    const expectedMembers = [...available(spec, spec.accessors), ...available(spec, spec.methods)];
    for (const e of extra) {
      (statics.includes(e) ? expectedStatics : expectedMembers).push(e);
    }
    assert.deepEqual(sorted(statics), sorted(expectedStatics), `${name}: static members`);
    assert.deepEqual(sorted(members), sorted(expectedMembers), `${name}: instance members`);
  }
});

test("module exports match the contract", () => {
  assert.deepEqual(sorted(Object.keys(api)), sorted(contract.module_exports[LANG]!));
  for (const [name, languages] of Object.entries(contract.functions)) {
    assert.equal(camel(name) in api, languages.includes(LANG), name);
  }
});

function buildFile(): Uint8Array {
  const w = new MdfWriter();
  w.initMdfFile();
  w.setStartTime(1_700_000_000_000_000_000n);
  for (const [group, base] of [["Engine", 0], ["Body", 100]] as const) {
    const g = w.addChannelGroup(group);
    w.setChannelGroupComment(g, "a comment");
    w.addTimeChannel(g, "Time");
    const speed = w.addFloatChannel(g, "Speed");
    const gear = w.addIntChannel(g, "Gear");
    w.addStringChannel(g, "Label");
    w.addChannel(g, "Raw", "SignedIntegerLE");
    w.addLinearConversion(speed, 1, 2);
    w.addValueToTextConversion(gear, [[0, "N"], [1, "D"]], "?");
    w.startDataBlock(g);
    for (let i = 0; i < 20; i++) w.writeRecord(g, [i * 0.5, base + i, i % 3, `L${i}`, -i]);
    w.finishDataBlock(g);
  }
  const bytes = w.finalize();
  w.dispose();
  return bytes;
}

const WINDOWS: ReadOptions[] = [
  {},
  { group: "Body" },
  { startTime: 2, endTime: 4.5 },
  { group: "Body", endTime: 1 },
  { startTime: 100 },
  { startRecord: 3, recordCount: 5 },
  { group: "Body", startRecord: 19, recordCount: 1 },
];

function plain(signal: Signal): unknown {
  return { ...signal, timestamps: Array.from(signal.timestamps) };
}

test("type fields match the contract and every reader gives the same answers", async () => {
  const bytes = buildFile();
  const mdf = Mdf.fromBytes(bytes);
  const dir = mkdtempSync(join(tmpdir(), "mf4-contract-"));
  const file = join(dir, "contract.mf4");
  writeFileSync(file, bytes);

  const index = MdfIndex.fromBytes(bytes);
  const fromJson = MdfIndex.fromJson(index.toJson(), new BytesRangeSource(bytes));
  fromJson.validate();
  await index.save(join(dir, "index.json"));
  const loaded = await MdfIndex.load(join(dir, "index.json"));
  assert.equal(loaded.source(), undefined);
  loaded.setSource(new BytesRangeSource(bytes));
  const lazy = await MdfIndex.fromRangeSource(new BytesRangeSource(bytes));
  const fromFile = await MdfIndex.fromFile(file);
  const mdfFromFile = await Mdf.fromFile(file);

  const groups = mdf.groups();
  const fields = (type: string): string[] => sorted(contract.types[type]!.fields.map(camel));
  assert.deepEqual(sorted(Object.keys(groups[0]!)), fields("GroupInfo"));
  assert.deepEqual(sorted(Object.keys(groups[0]!.channels[0]!)), fields("ChannelInfo"));
  assert.deepEqual(sorted(Object.keys(mdf.read("Speed"))), fields("Signal"));
  assert.deepEqual(sorted(Object.keys(mdf.timeRange("Speed")!)), fields("TimeRange"));
  for (const key of Object.keys(mdf.conversion("Gear")!)) {
    assert.ok(fields("ConversionInfo").includes(key), `ConversionInfo.${key}`);
  }
  assert.deepEqual(groups[0]!.channelNames, ["Time", "Speed", "Gear", "Label", "Raw"]);
  assert.equal(groups[0]!.masterChannel, "Time");
  assert.equal(groups[0]!.comment, "a comment");
  assert.equal(mdf.channel("Speed")!.dataType, "FloatLE");

  assert.deepEqual(mdfFromFile.groups(), groups);
  assert.deepEqual(plain(mdfFromFile.read("Speed", { group: "Body" })), plain(mdf.read("Speed", { group: "Body" })));

  for (const [label, reader] of Object.entries({ index, fromJson, loaded, lazy, fromFile })) {
    assert.deepEqual(reader.groups(), groups, `${label}: groups`);
    assert.deepEqual(reader.group("Body"), mdf.group("Body"), `${label}: group`);
    assert.equal(reader.group("Nope"), undefined);
    assert.equal(reader.channel("Nope"), undefined);
    assert.deepEqual(reader.channelNames(), mdf.channelNames());
    assert.equal(reader.startTimeNs(), 1_700_000_000_000_000_000n);
    assert.equal(mdf.startTimeNs(), 1_700_000_000_000_000_000n);
    assert.equal(reader.fileSize(), bytes.length);
    for (const name of ["Time", "Speed", "Gear", "Label", "Raw"]) {
      for (const group of [undefined, "Body"]) {
        assert.deepEqual(reader.channel(name, group), mdf.channel(name, group), `${label}: channel ${name}`);
        assert.deepEqual(await reader.timeRange(name, group), mdf.timeRange(name, group), `${label}: timeRange`);
        assert.deepEqual(await reader.conversion(name, group), mdf.conversion(name, group), `${label}: conversion`);
      }
      for (const window of WINDOWS) {
        const what = `${label}: ${name} ${JSON.stringify(window)}`;
        assert.deepEqual(plain(await reader.read(name, window)), plain(mdf.read(name, window)), what);
        assert.deepEqual(Array.from(await reader.values(name, window)), Array.from(mdf.values(name, window)), what);
      }
    }
    await assert.rejects(reader.read("Nope"), ChannelNotFoundError);
    await assert.rejects(reader.values("Nope", { group: "Body" }), ChannelNotFoundError);
    await assert.rejects(reader.timeRange("Nope"), ChannelNotFoundError);
    await assert.rejects(reader.conversion("Nope"), ChannelNotFoundError);
    assert.throws(() => reader.byteRanges("Nope"), ChannelNotFoundError);
    await assert.rejects(reader.read("Speed", { startTime: 0, startRecord: 0 }));
    reader.dispose();
  }
  assert.throws(() => mdf.read("Nope"), ChannelNotFoundError);
  assert.throws(() => mdf.values("Nope", { group: "Body" }), ChannelNotFoundError);
  assert.throws(() => mdf.timeRange("Nope"), ChannelNotFoundError);
  assert.throws(() => mdf.conversion("Nope"), ChannelNotFoundError);
  assert.throws(() => mdf.read("Speed", { startTime: 0, startRecord: 0 }));

  const window = mdf.read("Speed", { startTime: 2, endTime: 3 });
  assert.deepEqual(Array.from(window.timestamps), [2, 2.5, 3]);
  assert.deepEqual(window.values, [9, 11, 13]);
  assert.equal(mdf.read("Time").timestamps.length, 0);
  assert.deepEqual(mdf.timeRange("Speed"), { start: 0, end: 9.5 });
  assert.equal(mdf.conversion("Gear")!.texts!["2"], "?");
  assert.equal(mdf.conversion("Time"), null);
  assert.deepEqual(mdf.read("Gear").values.slice(0, 4), ["N", "D", "?", "N"]);
  mdf.dispose();
  mdfFromFile.dispose();
});

test("writeColumns matches writeRecord", () => {
  const w = new MdfWriter();
  w.initMdfFile();
  const g = w.addChannelGroup();
  w.addTimeChannel(g, "Time");
  w.addFloat32Channel(g, "F32");
  w.addIntChannel(g, "U64");
  w.addSintChannel(g, "I64");
  w.startDataBlock(g);
  w.writeRecord(g, [0, 1.5, 7, -7]);
  w.writeColumns(g, [
    new Float64Array([1, 2]),
    new Float32Array([2.5, 3.5]),
    new BigUint64Array([8n, 9n]),
    new BigInt64Array([-8n, -9n]),
  ]);
  w.finishDataBlock(g);
  const mdf = Mdf.fromBytes(w.finalize());
  w.dispose();
  assert.deepEqual(Array.from(mdf.values("Time")), [0, 1, 2]);
  assert.deepEqual(Array.from(mdf.values("F32")), [1.5, 2.5, 3.5]);
  assert.deepEqual(Array.from(mdf.values("U64")), [7, 8, 9]);
  assert.deepEqual(Array.from(mdf.values("I64")), [-7, -8, -9]);
  assert.throws(() => w.writeRecord(g, [0, 0, 0, 0]));
  mdf.dispose();

  // Columns are range-checked like records: nothing is truncated.
  const narrow = new MdfWriter();
  narrow.initMdfFile();
  const n = narrow.addChannelGroup();
  narrow.addChannel(n, "U32", "UnsignedIntegerLE");
  narrow.addChannel(n, "I32", "SignedIntegerLE");
  narrow.startDataBlock(n);
  narrow.writeColumns(n, [new BigUint64Array([2n ** 32n - 1n]), new BigInt64Array([-(2n ** 31n)])]);
  assert.throws(() => narrow.writeColumns(n, [new BigUint64Array([2n ** 32n]), new BigInt64Array([0n])]));
  assert.throws(() => narrow.writeColumns(n, [new BigUint64Array([0n]), new BigInt64Array([2n ** 31n])]));
  assert.throws(() => narrow.writeColumns(n, [new Float64Array([0]), new BigInt64Array([0n])]));
  // Types the writer cannot encode are refused up front.
  for (const dataType of ["FloatBE", "UnsignedIntegerBE", "SignedIntegerBE", "NoSuchType"]) {
    assert.throws(() => narrow.addChannel(n, "x", dataType), dataType);
  }
  narrow.dispose();
});

// ---------------------------------------------------------------------------
// Cross-language golden test
// ---------------------------------------------------------------------------
//
// Every language builds the same file with its own writer, answers the calls
// listed in tests/contract/scenario.json with both readers, and must
// reproduce tests/contract/golden.json (generated by the Rust test).

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type Case = { name: string; group?: string } & Record<string, Json | undefined>;

const contractDir = join(__dirname, "..", "..", "..", "tests", "contract");
const scenario = JSON.parse(readFileSync(join(contractDir, "scenario.json"), "utf8")) as {
  read_cases: Case[];
  channel_cases: Case[];
  write_cases: {
    channels: Record<string, string>;
    cases: { channel: string; value: Record<string, Json>; ok: boolean }[];
  };
};
const golden = JSON.parse(readFileSync(join(contractDir, "golden.json"), "utf8")) as Json;

function buildScenarioFile(): Uint8Array {
  const w = new MdfWriter();
  w.initMdfFile();
  w.setStartTime(1_700_000_000_123_456_789n);

  let g = w.addChannelGroup("Engine");
  w.setChannelGroupComment(g, "engine data");
  w.addTimeChannel(g, "Time");
  const speed = w.addFloatChannel(g, "Speed");
  const gear = w.addIntChannel(g, "Gear");
  w.addSintChannel(g, "Raw");
  w.addFloat32Channel(g, "Small");
  w.addStringChannel(g, "Label");
  w.addChannel(g, "Latin", "StringLatin1");
  w.addChannel(g, "Wide", "StringUtf16LE");
  w.addChannel(g, "Blob", "ByteArray");
  w.addLinearConversion(speed, 1, 2);
  w.addValueToTextConversion(gear, [[0, "N"], [1, "D"]], "?");
  w.startDataBlock(g);
  for (let i = 0; i < 6; i++) {
    w.writeRecord(g, [i * 0.5, i, i % 3, -i, i + 0.25, `L${i}`, `é${i}`, `ü€${i}`, new Uint8Array([i, i + 1])]);
  }
  w.finishDataBlock(g);

  g = w.addChannelGroup("NoMaster");
  w.addIntChannel(g, "Count");
  w.startDataBlock(g);
  for (let i = 0; i < 3; i++) w.writeRecord(g, [10 + i]);
  w.finishDataBlock(g);

  g = w.addChannelGroup("Empty");
  w.addTimeChannel(g, "Time");
  w.addFloatChannel(g, "Value");
  w.startDataBlock(g);
  w.finishDataBlock(g);

  const bytes = w.finalize();
  w.dispose();
  return bytes;
}

const WINDOW_KEYS = ["group", "start_time", "end_time", "start_record", "record_count"] as const;

function caseLabel(c: Case): string {
  // `1.0` prints as `1`, in every language.
  return [c.name, ...WINDOW_KEYS.filter((k) => c[k] !== undefined).map((k) => `${k}=${c[k]}`)].join(" ");
}

function caseOptions(c: Case): ReadOptions {
  const options: Record<string, unknown> = {};
  for (const key of WINDOW_KEYS) {
    if (c[key] !== undefined) options[camel(key)] = c[key];
  }
  return options as ReadOptions;
}

async function guarded(call: () => unknown): Promise<unknown> {
  try {
    return await call();
  } catch (e) {
    return e instanceof ChannelNotFoundError ? "error: channel not found" : "error";
  }
}

const floats = (values: ArrayLike<number>): Json[] =>
  Array.from(values, (v) => (Number.isNaN(v) ? null : v));

function signalJson(signal: Signal): unknown {
  return {
    name: signal.name,
    unit: signal.unit,
    timestamps: floats(signal.timestamps),
    values: signal.values.map((v) =>
      v instanceof Uint8Array ? { bytes: Array.from(v) } : typeof v === "bigint" ? Number(v) : v,
    ),
  };
}

/** Canonical form shared by the three languages: snake_case field names, no
 * null/undefined object fields. */
function canonical(value: unknown): Json {
  if (value === null || value === undefined) return null;
  if (Array.isArray(value)) return value.map(canonical);
  if (typeof value === "object") {
    const out: { [key: string]: Json } = {};
    for (const [key, v] of Object.entries(value as object)) {
      if (v === null || v === undefined) continue;
      // camelCase field names only; case labels start with a channel name.
      const snake = /^[a-z]/.test(key) ? key.replace(/[A-Z]/g, (ch) => `_${ch.toLowerCase()}`) : key;
      out[snake] = canonical(v);
    }
    return out;
  }
  return value as Json;
}

type AnyReader = {
  groups(): unknown;
  channelNames(): string[];
  startTimeNs(): bigint | undefined;
  channel(name: string, group?: string | null): unknown;
  read(name: string, options?: ReadOptions): Signal | Promise<Signal>;
  values(name: string, options?: ReadOptions): Float64Array | Promise<Float64Array>;
  timeRange(name: string, group?: string | null): unknown;
  conversion(name: string, group?: string | null): unknown;
};

async function dump(reader: AnyReader): Promise<Json> {
  const reads: Record<string, unknown> = {};
  for (const c of scenario.read_cases) {
    const options = caseOptions(c);
    reads[caseLabel(c)] = {
      read: await guarded(async () => signalJson(await reader.read(c.name, options))),
      values: await guarded(async () => floats(await reader.values(c.name, options))),
    };
  }
  const channels: Record<string, unknown> = {};
  for (const c of scenario.channel_cases) {
    channels[caseLabel(c)] = {
      channel: reader.channel(c.name, c.group),
      timeRange: await guarded(async () => {
        const range = (await reader.timeRange(c.name, c.group)) as { start: number; end: number } | undefined;
        return range && [range.start, range.end];
      }),
      conversion: await guarded(() => reader.conversion(c.name, c.group)),
    };
  }
  return canonical({
    startTimeNs: reader.startTimeNs()?.toString(),
    channelNames: reader.channelNames(),
    groups: reader.groups(),
    reads,
    channels,
  });
}

test("golden dump matches the other languages", async () => {
  const bytes = buildScenarioFile();
  const mdf = Mdf.fromBytes(bytes);
  const index = MdfIndex.fromBytes(bytes);
  const lazy = await MdfIndex.fromRangeSource(new BytesRangeSource(bytes));
  for (const [label, reader] of Object.entries({ Mdf: mdf, MdfIndex: index, "MdfIndex (lazy)": lazy })) {
    assert.deepEqual(await dump(reader), golden, `${label} differs from tests/contract/golden.json`);
    reader.dispose();
  }
});

test("writeRecord accepts and refuses the same values as Python", () => {
  const spec = scenario.write_cases;
  const w = new MdfWriter();
  w.initMdfFile();
  const groups: Record<string, string> = {};
  for (const [name, how] of Object.entries(spec.channels)) {
    const g = w.addChannelGroup(name);
    if (how.startsWith("add_")) {
      (w as unknown as Record<string, (g: string, n: string) => string>)[camel(how)]!(g, name);
    } else {
      w.addChannel(g, name, how);
    }
    w.startDataBlock(g);
    groups[name] = g;
  }
  const toValue = (value: Record<string, Json>): unknown => {
    const [kind, raw] = Object.entries(value)[0]!;
    if (kind === "int") {
      const big = BigInt(raw as string);
      return Number.isSafeInteger(Number(big)) ? Number(big) : big;
    }
    if (kind === "float") return Number(raw);
    if (kind === "bytes") return new Uint8Array(raw as number[]);
    return raw;
  };
  for (const c of spec.cases) {
    let ok = true;
    try {
      w.writeRecord(groups[c.channel]!, [toValue(c.value) as never]);
    } catch {
      ok = false;
    }
    assert.equal(ok, c.ok, `writeRecord(${c.channel}, ${JSON.stringify(c.value)})`);
  }
  w.dispose();
});
