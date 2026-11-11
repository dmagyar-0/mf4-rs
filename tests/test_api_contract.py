#!/usr/bin/env python3
"""Guards the cross-language API contract (``docs/api-contract.json``).

The public names of the ``mf4_rs`` Python package must be exactly what the
contract lists: adding, renaming or removing a class, method, property, type
field or function fails this test until the contract — and with it the Rust
and TypeScript bindings, which have the same test — is updated.

Also checks that ``Mdf`` and ``MdfIndex`` answer every contract call
identically for the same file.

Run with: ``python tests/test_api_contract.py`` (exits 0 if ``mf4_rs`` or
numpy are not installed).
"""

import json
import os
import sys
import tempfile

try:
    import numpy as np
    import mf4_rs
except ImportError as exc:  # pragma: no cover
    # Skipping is fine on a developer machine, never in CI: a contract test
    # that silently does not run guards nothing.
    if os.environ.get("CI"):
        raise
    print(f"SKIP: {exc}")
    sys.exit(0)

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
with open(os.path.join(ROOT, "docs", "api-contract.json")) as f:
    CONTRACT = json.load(f)

LANG = "python"


def public(obj):
    return {n for n in dir(obj) if not n.startswith("_")}


def expected_class(spec):
    names = set(spec["extra"][LANG])
    for section in ("constructors", "accessors", "methods"):
        for name in spec[section]:
            only = spec["only"].get(name)
            if only is None or LANG in only:
                names.add(name)
    return names


def test_module_exports():
    assert public(mf4_rs) == set(CONTRACT["module_exports"][LANG]), (
        sorted(public(mf4_rs) ^ set(CONTRACT["module_exports"][LANG]))
    )
    for name, languages in CONTRACT["functions"].items():
        assert (name in public(mf4_rs)) == (LANG in languages), name


def test_class_members():
    for name, spec in CONTRACT["classes"].items():
        actual, expected = public(getattr(mf4_rs, name)), expected_class(spec)
        assert actual == expected, f"{name}: {sorted(actual ^ expected)} differ from the contract"
    # Accessors are properties in Python, everything else is callable.
    for name, spec in CONTRACT["classes"].items():
        cls = getattr(mf4_rs, name)
        for accessor in spec["accessors"]:
            assert not callable(getattr(cls, accessor)), f"{name}.{accessor} must be a property"


def test_type_fields():
    for name in ("Signal", "GroupInfo", "ChannelInfo"):
        spec = CONTRACT["types"][name]
        expected = set(spec["fields"]) | set(spec["methods"][LANG])
        assert public(getattr(mf4_rs, name)) == expected, name


def test_exceptions():
    errors = CONTRACT["errors"]
    base = getattr(mf4_rs, errors["base"][LANG])
    assert issubclass(getattr(mf4_rs, errors["channel_not_found"][LANG]), base)


def write_fixture(path):
    w = mf4_rs.MdfWriter(path)
    w.init_mdf_file()
    w.set_start_time(1_700_000_000_000_000_000)
    for group, base in (("Engine", 0.0), ("Body", 100.0)):
        g = w.add_channel_group(group)
        w.set_channel_group_comment(g, "a comment")
        w.add_time_channel(g, "Time")
        speed = w.add_float_channel(g, "Speed")
        gear = w.add_int_channel(g, "Gear")
        w.add_string_channel(g, "Label")
        w.add_channel(g, "Raw", "SignedIntegerLE")
        w.add_linear_conversion(speed, 1.0, 2.0)
        w.add_value_to_text_conversion(gear, [(0, "N"), (1, "D")], "?")
        w.start_data_block(g)
        for i in range(20):
            w.write_record(g, [i * 0.5, base + i, i % 3, f"L{i}", -i])
        w.finish_data_block(g)
    w.finalize()


def info(obj, type_name):
    """A contract type as a plain dict (nested infos included)."""
    out = {}
    for field in CONTRACT["types"][type_name]["fields"]:
        value = getattr(obj, field)
        if field == "channels":
            value = [info(c, "ChannelInfo") for c in value]
        out[field] = value
    return out


def same_signal(a, b):
    assert (a.name, a.unit) == (b.name, b.unit)
    np.testing.assert_array_equal(a.timestamps, b.timestamps)
    assert a.values.dtype == b.values.dtype
    if a.values.dtype == object:
        assert list(a.values) == list(b.values)
    else:
        np.testing.assert_array_equal(a.values, b.values)


WINDOWS = [
    {},
    {"group": "Body"},
    {"start_time": 2.0, "end_time": 4.5},
    {"group": "Body", "end_time": 1.0},
    {"start_time": 100.0},
    {"start_record": 3, "record_count": 5},
    {"group": "Body", "start_record": 19, "record_count": 1},
]


def test_readers_agree():
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "contract.mf4")
        write_fixture(path)
        mdf = mf4_rs.Mdf.from_file(path)
        with open(path, "rb") as f:
            data = f.read()
        from_bytes = mf4_rs.Mdf.from_bytes(data)
        index = mf4_rs.MdfIndex.from_file(path)
        reloaded = mf4_rs.MdfIndex.from_json(index.to_json())
        reloaded.validate()
        assert reloaded.source is None
        reloaded.set_source(path)
        index.save(os.path.join(tmp, "i.json"))
        loaded = mf4_rs.MdfIndex.load(os.path.join(tmp, "i.json"))
        loaded.set_source(path)
        in_memory = mf4_rs.MdfIndex.from_bytes(data)
        in_memory.set_source(path)

        groups = [info(g, "GroupInfo") for g in mdf.groups]
        assert groups[0]["channel_names"] == ["Time", "Speed", "Gear", "Label", "Raw"]
        assert groups[0]["master_channel"] == "Time" and groups[0]["comment"] == "a comment"
        assert mdf.channel("Speed").data_type == "FloatLE" and mdf.channel("Speed").has_conversion

        for reader in (from_bytes, index, reloaded, loaded, in_memory):
            assert [info(g, "GroupInfo") for g in reader.groups] == groups
            assert info(reader.group("Body"), "GroupInfo") == groups[1]
            assert reader.group("Nope") is None and reader.channel("Nope") is None
            assert reader.channel_names == mdf.channel_names
            assert reader.start_time_ns == mdf.start_time_ns == 1_700_000_000_000_000_000
            for name in ("Time", "Speed", "Gear", "Label", "Raw"):
                for group in (None, "Body"):
                    assert info(reader.channel(name, group), "ChannelInfo") == info(
                        mdf.channel(name, group), "ChannelInfo"
                    )
                    assert reader.time_range(name, group) == mdf.time_range(name, group)
                    assert reader.conversion(name, group) == mdf.conversion(name, group)
                for window in WINDOWS:
                    same_signal(reader.read(name, **window), mdf.read(name, **window))
                    np.testing.assert_array_equal(reader.values(name, **window), mdf.values(name, **window))
            same_signal(reader["Speed", "Body"], mdf.read("Speed", group="Body"))
            for call in (reader.read, reader.values, reader.time_range, reader.conversion):
                try:
                    call("Nope")
                except mf4_rs.ChannelNotFoundError:
                    pass
                else:
                    raise AssertionError(f"{call} did not raise ChannelNotFoundError")
            try:
                reader.read("Speed", start_time=0.0, start_record=0)
            except mf4_rs.MdfException:
                pass
            else:
                raise AssertionError("mixed windows must be rejected")

        window = mdf.read("Speed", start_time=2.0, end_time=3.0)
        assert list(window.timestamps) == [2.0, 2.5, 3.0] and list(window.values) == [9.0, 11.0, 13.0]
        assert len(mdf.read("Time").timestamps) == 0
        assert mdf.time_range("Speed") == (0.0, 9.5)
        gear = mdf.conversion("Gear")
        assert set(gear) <= set(CONTRACT["types"]["ConversionInfo"]["fields"])
        assert gear["type"] == "ValueToText" and gear["texts"][2] == "?"
        assert mdf.conversion("Time") is None
        assert list(mdf.read("Gear").values[:4]) == ["N", "D", "?", "N"]
        assert len(index.byte_ranges("Speed")) == 1
        assert len(index.byte_ranges_for_records("Speed", 2, 3, group="Body")) == 1

        try:
            import pandas  # noqa: F401
        except ImportError:
            return
        series = mdf.read("Speed").to_series()
        assert series.name == "Speed" and str(series.index.dtype).startswith("datetime64")
        assert list(series.to_numpy()[:2]) == [1.0, 3.0]


def test_write_columns_matches_write_record():
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "columns.mf4")
        w = mf4_rs.MdfWriter(path)
        w.init_mdf_file()
        g = w.add_channel_group()
        w.add_time_channel(g, "Time")
        w.add_float32_channel(g, "F32")
        w.add_int_channel(g, "U64")
        w.add_sint_channel(g, "I64")
        w.start_data_block(g)
        w.write_record(g, [0.0, 1.5, 7, -7])
        w.write_columns(g, [
            np.array([1.0, 2.0]),
            np.array([2.5, 3.5], dtype=np.float32),
            np.array([8, 9], dtype=np.uint64),
            np.array([-8, -9], dtype=np.int64),
        ])
        w.finish_data_block(g)
        w.finalize()
        mdf = mf4_rs.Mdf.from_file(path)
        assert list(mdf.values("Time")) == [0.0, 1.0, 2.0]
        assert list(mdf.values("F32")) == [1.5, 2.5, 3.5]
        assert list(mdf.values("U64")) == [7.0, 8.0, 9.0]
        assert list(mdf.values("I64")) == [-7.0, -8.0, -9.0]

        # Columns are range-checked like records: nothing is truncated.
        w = mf4_rs.MdfWriter(os.path.join(tmp, "narrow.mf4"))
        w.init_mdf_file()
        g = w.add_channel_group()
        w.add_channel(g, "U32", "UnsignedIntegerLE")
        w.add_channel(g, "I32", "SignedIntegerLE")
        w.start_data_block(g)
        w.write_columns(g, [np.array([2**32 - 1], np.uint64), np.array([-(2**31)], np.int64)])
        for columns in (
            [np.array([2**32], np.uint64), np.array([0], np.int64)],
            [np.array([0], np.uint64), np.array([2**31], np.int64)],
            [np.array([0.0]), np.array([0], np.int64)],
        ):
            try:
                w.write_columns(g, columns)
            except mf4_rs.MdfException:
                pass
            else:
                raise AssertionError(f"write_columns accepted {columns}")
        # Types the writer cannot encode are refused up front.
        for data_type in ("FloatBE", "UnsignedIntegerBE", "SignedIntegerBE", "NoSuchType"):
            try:
                w.add_channel(g, "x", data_type)
            except mf4_rs.MdfException:
                pass
            else:
                raise AssertionError(f"add_channel accepted {data_type}")


# ---------------------------------------------------------------------------
# Cross-language golden test
# ---------------------------------------------------------------------------
#
# Every language builds the same file with its own writer, answers the calls
# listed in tests/contract/scenario.json with both readers, and must
# reproduce tests/contract/golden.json (generated by the Rust test).

with open(os.path.join(ROOT, "tests", "contract", "scenario.json")) as f:
    SCENARIO = json.load(f)
with open(os.path.join(ROOT, "tests", "contract", "golden.json")) as f:
    GOLDEN = json.load(f)


def build_scenario_file(path):
    w = mf4_rs.MdfWriter(path)
    w.init_mdf_file()
    w.set_start_time(1_700_000_000_123_456_789)

    g = w.add_channel_group("Engine")
    w.set_channel_group_comment(g, "engine data")
    w.add_time_channel(g, "Time")
    speed = w.add_float_channel(g, "Speed")
    gear = w.add_int_channel(g, "Gear")
    w.add_sint_channel(g, "Raw")
    w.add_float32_channel(g, "Small")
    w.add_string_channel(g, "Label")
    w.add_channel(g, "Latin", "StringLatin1")
    w.add_channel(g, "Wide", "StringUtf16LE")
    w.add_channel(g, "Blob", "ByteArray")
    w.add_linear_conversion(speed, 1.0, 2.0)
    w.add_value_to_text_conversion(gear, [(0, "N"), (1, "D")], "?")
    w.start_data_block(g)
    for i in range(6):
        w.write_record(g, [i * 0.5, float(i), i % 3, -i, i + 0.25, f"L{i}", f"\u00e9{i}", f"\u00fc\u20ac{i}", bytes([i, i + 1])])
    w.finish_data_block(g)

    g = w.add_channel_group("NoMaster")
    w.add_int_channel(g, "Count")
    w.start_data_block(g)
    for i in range(3):
        w.write_record(g, [10 + i])
    w.finish_data_block(g)

    g = w.add_channel_group("Empty")
    w.add_time_channel(g, "Time")
    w.add_float_channel(g, "Value")
    w.start_data_block(g)
    w.finish_data_block(g)

    w.finalize()


def case_label(case):
    parts = [case["name"]]
    for key in ("group", "start_time", "end_time", "start_record", "record_count"):
        if key in case:
            value = case[key]
            if isinstance(value, float) and value.is_integer():
                value = int(value)       # `1.0` prints as `1`, in every language
            parts.append(f"{key}={value}")
    return " ".join(parts)


def guarded(call):
    try:
        return call()
    except mf4_rs.ChannelNotFoundError:
        return "error: channel not found"
    except mf4_rs.MdfException:
        return "error"


def floats(values):
    return [None if v != v else float(v) for v in values]


def plain_value(v):
    if v is None:
        return None
    if isinstance(v, bytes):
        return {"bytes": list(v)}
    if isinstance(v, str):
        return v
    return None if v != v else float(v)


def signal_json(signal):
    return {
        "name": signal.name,
        "unit": signal.unit,
        "timestamps": floats(signal.timestamps),
        "values": [plain_value(v) for v in signal.values.tolist()],
    }


def canonical(value):
    """Canonical form shared by the three languages: no null-valued object
    fields, every number a float."""
    if isinstance(value, dict):
        return {str(k): canonical(v) for k, v in value.items() if v is not None}
    if isinstance(value, (list, tuple)):
        return [canonical(v) for v in value]
    if isinstance(value, bool) or value is None or isinstance(value, str):
        return value
    return float(value)


def dump(reader):
    reads = {}
    for case in SCENARIO["read_cases"]:
        options = {k: v for k, v in case.items() if k != "name"}
        reads[case_label(case)] = {
            "read": guarded(lambda: signal_json(reader.read(case["name"], **options))),
            "values": guarded(lambda: floats(reader.values(case["name"], **options))),
        }
    channels = {}
    for case in SCENARIO["channel_cases"]:
        name, group = case["name"], case.get("group")
        channel = reader.channel(name, group)
        channels[case_label(case)] = {
            "channel": info(channel, "ChannelInfo") if channel else None,
            "time_range": guarded(lambda: reader.time_range(name, group)),
            "conversion": guarded(lambda: reader.conversion(name, group)),
        }
    start = reader.start_time_ns
    return canonical({
        "start_time_ns": None if start is None else str(start),
        "channel_names": reader.channel_names,
        "groups": [info(g, "GroupInfo") for g in reader.groups],
        "reads": reads,
        "channels": channels,
    })


def first_difference(actual, expected, path="$"):
    if isinstance(actual, dict) and isinstance(expected, dict):
        for key in sorted(set(actual) | set(expected)):
            if key not in actual or key not in expected:
                return f"{path}.{key}: only in {'golden' if key in expected else 'actual'}"
            found = first_difference(actual[key], expected[key], f"{path}.{key}")
            if found:
                return found
        return None
    if isinstance(actual, list) and isinstance(expected, list) and len(actual) == len(expected):
        for i, (a, e) in enumerate(zip(actual, expected)):
            found = first_difference(a, e, f"{path}[{i}]")
            if found:
                return found
        return None
    return None if actual == expected else f"{path}: {actual!r} != golden {expected!r}"


def test_golden_dump_matches():
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "scenario.mf4")
        build_scenario_file(path)
        golden = canonical(GOLDEN)
        for label, reader in (("Mdf", mf4_rs.Mdf.from_file(path)), ("MdfIndex", mf4_rs.MdfIndex.from_file(path))):
            difference = first_difference(dump(reader), golden)
            assert difference is None, f"{label} differs from tests/contract/golden.json at {difference}"


def case_value(spec):
    (kind, value), = spec.items()
    if kind == "int":
        return int(value)
    if kind == "float":
        return float(value)
    if kind == "bytes":
        return bytes(value)
    return value


def test_write_record_coercion():
    """write_record accepts and refuses exactly the values the scenario lists
    (the TypeScript test runs the same cases)."""
    spec = SCENARIO["write_cases"]
    with tempfile.TemporaryDirectory() as tmp:
        w = mf4_rs.MdfWriter(os.path.join(tmp, "write.mf4"))
        w.init_mdf_file()
        groups = {}
        for name, how in spec["channels"].items():
            g = w.add_channel_group(name)
            if how.startswith("add_"):
                getattr(w, how)(g, name)
            else:
                w.add_channel(g, name, how)
            w.start_data_block(g)
            groups[name] = g
        for case in spec["cases"]:
            try:
                w.write_record(groups[case["channel"]], [case_value(case["value"])])
                ok = True
            except mf4_rs.MdfException:
                ok = False
            assert ok == case["ok"], f"write_record({case['channel']}, {case['value']}) ok={ok}, expected {case['ok']}"


def test_refused_record_leaves_no_trace():
    """A record refused halfway (text already encoded, then an integer that
    does not fit) must not leave its text value in the file."""
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "refused.mf4")
        w = mf4_rs.MdfWriter(path)
        w.init_mdf_file()
        g = w.add_channel_group()
        w.add_time_channel(g, "t")
        w.add_string_channel(g, "s")
        w.add_channel(g, "u", "UnsignedIntegerLE")
        w.start_data_block(g)
        w.write_record(g, [0.0, "a", 1])
        try:
            w.write_record(g, [1.0, "ORPHAN", 2**33])
        except mf4_rs.MdfException:
            pass
        else:
            raise AssertionError("out-of-range value accepted")
        w.write_record(g, [2.0, "b", 2])
        w.finish_data_block(g)
        w.finalize()
        for reader in (mf4_rs.Mdf.from_file(path), mf4_rs.MdfIndex.from_file(path)):
            signal = reader.read("s")
            assert list(signal.values) == ["a", "b"] and list(signal.timestamps) == [0.0, 2.0], signal.values


def main():
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    failed = 0
    for test in tests:
        try:
            test()
            print(f"PASS {test.__name__}")
        except Exception as exc:  # noqa: BLE001
            failed += 1
            print(f"FAIL {test.__name__}: {exc!r}")
    print(f"{len(tests) - failed}/{len(tests)} passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
