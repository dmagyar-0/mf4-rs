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
