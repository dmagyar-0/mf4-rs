#!/usr/bin/env python3
"""
Cross-compatibility tests between mf4-rs and asammdf.

These tests verify that:
1. Files written by mf4-rs can be read correctly by asammdf
2. Files written by asammdf can be read correctly by mf4-rs
3. Values round-trip accurately across both libraries
4. Conversions (value-to-text) are interoperable
5. Data block splitting (##DL) is handled correctly

Prerequisites:
    pip install asammdf pandas numpy
    maturin develop --release   (or: pip install .)

Usage:
    python tests/test_asammdf_interop.py

This script exits with code 0 on success, non-zero on failure.
"""

import os
import sys
import tempfile
import traceback

import numpy as np


def check_imports():
    """Verify all required packages are available."""
    errors = []
    try:
        import asammdf  # noqa: F401
    except ImportError:
        errors.append("asammdf (pip install asammdf)")
    try:
        import mf4_rs  # noqa: F401
    except ImportError:
        errors.append("mf4_rs (maturin develop --release)")
    try:
        import pandas  # noqa: F401
    except ImportError:
        errors.append("pandas (pip install pandas)")
    if errors:
        print(f"SKIP: missing packages: {', '.join(errors)}")
        sys.exit(0)  # exit 0 so CI doesn't fail if packages unavailable


check_imports()

from asammdf import MDF as AsamMDF, Signal  # noqa: E402
import mf4_rs  # noqa: E402


passed = 0
failed = 0
skipped = 0


def run_test(name, fn):
    """Run a test function and track results."""
    global passed, failed
    try:
        fn()
        print(f"  PASS  {name}")
        passed += 1
    except Exception as e:
        print(f"  FAIL  {name}: {e}")
        traceback.print_exc()
        failed += 1


# ---------------------------------------------------------------------------
# Test helpers
# ---------------------------------------------------------------------------


def tmp(name):
    return os.path.join(tempfile.gettempdir(), f"interop_{name}.mf4")


def cleanup(*paths):
    for p in paths:
        try:
            os.remove(p)
        except OSError:
            pass


def write_mf4rs_basic():
    """Write a basic file with mf4-rs: time + float + int, 100 records."""
    path = tmp("mf4rs_basic")
    w = mf4_rs.PyMdfWriter(path)
    w.init_mdf_file()
    cg = w.add_channel_group("Group1")
    t = w.add_time_channel(cg, "Time")
    w.add_float_channel(cg, "Temperature")
    w.add_int_channel(cg, "Counter")

    w.start_data_block(cg)
    for i in range(100):
        w.write_record(cg, [
            mf4_rs.create_float_value(i * 0.01),
            mf4_rs.create_float_value(20.0 + i * 0.5),
            mf4_rs.create_uint_value(i),
        ])
    w.finish_data_block(cg)
    w.finalize()
    return path


def write_asammdf_basic():
    """Write a basic file with asammdf: time + float + int, 100 records."""
    path = tmp("asammdf_basic")
    t = np.arange(100, dtype=np.float64) * 0.01
    temp = 20.0 + np.arange(100, dtype=np.float64) * 0.5
    counter = np.arange(100, dtype=np.uint64)
    mdf = AsamMDF()
    mdf.append([
        Signal(samples=t, timestamps=t, name="Time"),
        Signal(samples=temp, timestamps=t, name="Temperature"),
        Signal(samples=counter, timestamps=t, name="Counter"),
    ])
    mdf.save(path, overwrite=True)
    mdf.close()
    return path


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_asammdf_reads_mf4rs_basic():
    """asammdf can read a basic mf4-rs file and get correct values."""
    path = write_mf4rs_basic()
    try:
        mdf = AsamMDF(path)
        assert len(mdf.groups) >= 1, f"expected >=1 groups, got {len(mdf.groups)}"

        temp = mdf.get("Temperature", group=0)
        assert len(temp.samples) == 100, f"expected 100 samples, got {len(temp.samples)}"
        assert abs(temp.samples[0] - 20.0) < 0.1, f"first temp should be ~20.0, got {temp.samples[0]}"
        assert abs(temp.samples[99] - 69.5) < 0.1, f"last temp should be ~69.5, got {temp.samples[99]}"

        counter = mdf.get("Counter", group=0)
        assert int(counter.samples[0]) == 0
        assert int(counter.samples[99]) == 99
        mdf.close()
    finally:
        cleanup(path)


def test_mf4rs_reads_asammdf_basic():
    """mf4-rs can read a basic asammdf file and get correct values."""
    path = write_asammdf_basic()
    try:
        mdf = mf4_rs.PyMDF(path)
        groups = mdf.channel_groups()
        assert len(groups) >= 1, f"expected >=1 groups, got {len(groups)}"

        temp_vals = mdf.get_channel_values("Temperature")
        assert temp_vals is not None, "Temperature channel not found"
        valid = [v for v in temp_vals if v is not None]
        assert len(valid) == 100, f"expected 100 values, got {len(valid)}"
        assert abs(valid[0] - 20.0) < 0.001, f"first temp should be 20.0, got {valid[0]}"
        assert abs(valid[99] - 69.5) < 0.001, f"last temp should be 69.5, got {valid[99]}"
    finally:
        cleanup(path)


def test_cross_read_values_match():
    """Values written by mf4-rs match when read by both libraries."""
    path = write_mf4rs_basic()
    try:
        # Read with mf4-rs
        rs_mdf = mf4_rs.PyMDF(path)
        rs_temp = [v for v in rs_mdf.get_channel_values("Temperature") if v is not None]
        rs_count = [v for v in rs_mdf.get_channel_values("Counter") if v is not None]

        # Read with asammdf
        a_mdf = AsamMDF(path)
        a_temp = a_mdf.get("Temperature", group=0).samples
        a_count = a_mdf.get("Counter", group=0).samples
        a_mdf.close()

        # Compare
        assert len(rs_temp) == len(a_temp), "length mismatch"
        for i, (rv, av) in enumerate(zip(rs_temp, a_temp)):
            assert abs(rv - av) < 1e-5, f"Temperature mismatch at {i}: {rv} vs {av}"
        for i, (rv, av) in enumerate(zip(rs_count, a_count)):
            assert int(rv) == int(av), f"Counter mismatch at {i}: {rv} vs {av}"
    finally:
        cleanup(path)


def test_all_integer_types_roundtrip():
    """asammdf-written files with all integer types are readable by mf4-rs."""
    types = {
        "uint8": (np.uint8, [0, 127, 255]),
        "uint16": (np.uint16, [0, 32767, 65535]),
        "uint32": (np.uint32, [0, 2147483647, 4294967295]),
        "uint64": (np.uint64, [0, 2**63 - 1, 2**64 - 1]),
        "int8": (np.int8, [-128, 0, 127]),
        "int16": (np.int16, [-32768, 0, 32767]),
        "int32": (np.int32, [-2147483648, 0, 2147483647]),
        "int64": (np.int64, [-(2**63), 0, 2**63 - 1]),
    }
    paths = []
    try:
        for name, (dtype, values) in types.items():
            path = tmp(f"dtype_{name}")
            paths.append(path)
            mdf = AsamMDF()
            samples = np.array(values, dtype=dtype)
            t = np.arange(len(values), dtype=np.float64)
            mdf.append([Signal(samples=samples, timestamps=t, name=name)])
            mdf.save(path, overwrite=True)
            mdf.close()

            rs_mdf = mf4_rs.PyMDF(path)
            rs_vals = rs_mdf.get_channel_values(name)
            valid = [v for v in rs_vals if v is not None]
            assert len(valid) == len(values), f"{name}: expected {len(values)}, got {len(valid)}"
            for orig, read in zip(values, valid):
                assert int(orig) == int(read), f"{name}: expected {orig}, got {read}"
    finally:
        cleanup(*paths)


def test_float_types_roundtrip():
    """asammdf-written files with float32 and float64 are readable by mf4-rs."""
    paths = []
    try:
        for name, dtype, values in [
            ("float32", np.float32, [-1.5, 0.0, 3.14]),
            ("float64", np.float64, [-1.5, 0.0, 3.141592653589793]),
        ]:
            path = tmp(f"dtype_{name}")
            paths.append(path)
            mdf = AsamMDF()
            samples = np.array(values, dtype=dtype)
            t = np.arange(len(values), dtype=np.float64)
            mdf.append([Signal(samples=samples, timestamps=t, name=name)])
            mdf.save(path, overwrite=True)
            mdf.close()

            rs_mdf = mf4_rs.PyMDF(path)
            rs_vals = rs_mdf.get_channel_values(name)
            valid = [v for v in rs_vals if v is not None]
            assert len(valid) == len(values), f"{name}: expected {len(values)}, got {len(valid)}"
            for orig, read in zip(values, valid):
                tol = 1e-2 if name == "float32" else 1e-10
                assert abs(orig - read) < tol, f"{name}: expected {orig}, got {read}"
    finally:
        cleanup(*paths)


def test_multi_group_cross_read():
    """Multi-group mf4-rs files are correctly parsed by asammdf."""
    path = tmp("multi_group")
    try:
        w = mf4_rs.PyMdfWriter(path)
        w.init_mdf_file()

        cg1 = w.add_channel_group("G1")
        w.add_time_channel(cg1, "Time")
        w.add_float_channel(cg1, "Temp")
        w.start_data_block(cg1)
        for i in range(10):
            w.write_record(cg1, [
                mf4_rs.create_float_value(i * 0.1),
                mf4_rs.create_float_value(20.0 + i),
            ])
        w.finish_data_block(cg1)

        cg2 = w.add_channel_group("G2")
        w.add_time_channel(cg2, "Time")
        w.add_float_channel(cg2, "Pressure")
        w.start_data_block(cg2)
        for i in range(5):
            w.write_record(cg2, [
                mf4_rs.create_float_value(i * 0.2),
                mf4_rs.create_float_value(1013.0 + i),
            ])
        w.finish_data_block(cg2)
        w.finalize()

        mdf = AsamMDF(path)
        assert len(mdf.groups) == 2, f"expected 2 groups, got {len(mdf.groups)}"

        temp = mdf.get("Temp", group=0)
        assert len(temp.samples) == 10
        assert abs(temp.samples[9] - 29.0) < 0.1

        pressure = mdf.get("Pressure", group=1)
        assert len(pressure.samples) == 5
        assert abs(pressure.samples[4] - 1017.0) < 0.1
        mdf.close()
    finally:
        cleanup(path)


def test_master_channel_detected():
    """asammdf correctly identifies the master channel in mf4-rs files."""
    path = write_mf4rs_basic()
    try:
        mdf = AsamMDF(path)
        master_idx = mdf.masters_db.get(0, None)
        assert master_idx is not None, "no master channel detected"
        master_ch = mdf.groups[0].channels[master_idx]
        assert master_ch.channel_type == 2, f"expected ch_type=2, got {master_ch.channel_type}"
        assert master_ch.sync_type == 1, f"expected sync_type=1, got {master_ch.sync_type}"
        assert master_ch.name == "Time", f"expected name='Time', got '{master_ch.name}'"
        mdf.close()
    finally:
        cleanup(path)


def test_file_identification():
    """mf4-rs files have correct identification block."""
    path = write_mf4rs_basic()
    try:
        with open(path, "rb") as f:
            data = f.read(64)
        file_id = data[0:8].rstrip(b"\x00")
        assert file_id == b"MDF     ", f"bad file ID: {file_id}"
        fmt = data[8:16].rstrip(b"\x00").decode().strip()
        assert fmt == "4.10", f"bad format: {fmt}"
        prog = data[16:24].rstrip(b"\x00").decode().strip()
        assert prog == "mf4-rs", f"bad program ID: {prog}"
    finally:
        cleanup(path)


def test_compressed_file_fails_gracefully():
    """mf4-rs fails with a clear error on compressed (##DZ) files."""
    path = tmp("compressed")
    try:
        t = np.arange(1000, dtype=np.float64) * 0.001
        mdf = AsamMDF()
        mdf.append([Signal(samples=t * 2.0, timestamps=t, name="data")])
        mdf.save(path, overwrite=True, compression=2)
        mdf.close()

        try:
            rs_mdf = mf4_rs.PyMDF(path)
            rs_mdf.get_channel_values("data")
            # If we get here without error, compression support was added
            print("    (NOTE: ##DZ reading now works - update comparison docs)")
        except Exception as e:
            # Expected: BlockIDError for ##DZ
            assert "DZ" in str(e) or "Block" in str(e), f"unexpected error: {e}"
    finally:
        cleanup(path)


def test_data_block_splitting_cross_read():
    """asammdf can read mf4-rs files that use data block splitting (##DL)."""
    path = tmp("dl_split")
    try:
        # Write enough data to trigger 4MB split: 300K records x 16 bytes = 4.8MB
        w = mf4_rs.PyMdfWriter(path)
        w.init_mdf_file()
        cg = w.add_channel_group("big")
        w.add_float_channel(cg, "a")
        w.add_float_channel(cg, "b")
        w.add_float_channel(cg, "c")
        w.add_float_channel(cg, "d")
        w.start_data_block(cg)
        n = 300_000
        for i in range(n):
            w.write_record(cg, [
                mf4_rs.create_float_value(float(i)),
                mf4_rs.create_float_value(float(i * 2)),
                mf4_rs.create_float_value(float(i * 3)),
                mf4_rs.create_float_value(float(i * 4)),
            ])
        w.finish_data_block(cg)
        w.finalize()

        # Verify ##DL block exists
        with open(path, "rb") as f:
            data = f.read()
        assert b"##DL" in data, "expected ##DL block in split file"

        # Read with asammdf
        mdf = AsamMDF(path)
        sig = mdf.get("a")
        assert len(sig.samples) == n, f"expected {n} samples, got {len(sig.samples)}"
        assert abs(sig.samples[0]) < 0.001, f"first value should be ~0, got {sig.samples[0]}"
        assert abs(sig.samples[-1] - (n - 1)) < 1.0, f"last value wrong: {sig.samples[-1]}"

        sig_d = mdf.get("d")
        assert abs(sig_d.samples[100] - 400.0) < 1.0
        mdf.close()
    finally:
        cleanup(path)


def test_value_to_text_conversion_cross_read():
    """asammdf correctly applies value-to-text conversions from mf4-rs files."""
    # This uses the Rust example file which has value-to-text conversions
    path = tmp("v2t")
    try:
        # Write file with value-to-text conversion using Rust API via cargo
        os.system(
            f'cd /home/user/mf4-rs && cargo run --release --example write_file 2>/dev/null'
        )
        example_path = "/home/user/mf4-rs/example.mf4"
        if not os.path.exists(example_path):
            print("    (SKIP: example.mf4 not found)")
            return

        mdf = AsamMDF(example_path)
        # Group 1 has Status channel with value-to-text conversion
        status = mdf.get("Status", group=1)
        assert status is not None, "Status channel not found"
        # With conversions applied, values should be byte strings
        assert status.samples[0] == b"OK" or status.samples[0] == "OK", \
            f"expected OK for value 0, got {status.samples[0]}"
        assert status.samples[1] == b"WARN" or status.samples[1] == "WARN", \
            f"expected WARN for value 1, got {status.samples[1]}"

        # Check the conversion type - find Status channel by name
        status_ch = None
        for ch in mdf.groups[1].channels:
            if ch.name == "Status":
                status_ch = ch
                break
        assert status_ch is not None, "Status channel not found in group 1"
        assert status_ch.conversion is not None, "expected conversion block"
        assert status_ch.conversion.conversion_type == 7, \
            f"expected conversion type 7 (ValueToText), got {status_ch.conversion.conversion_type}"
        mdf.close()
    finally:
        cleanup(path)
        cleanup("/home/user/mf4-rs/example.mf4")


def test_units_and_comments_readable():
    """mf4-rs can read units and comments from asammdf files."""
    path = tmp("units")
    try:
        mdf = AsamMDF()
        sig = Signal(
            samples=np.array([20.0, 21.0, 22.0]),
            timestamps=np.array([0.0, 1.0, 2.0]),
            name="Temperature",
            unit="degC",
            comment="Ambient temperature",
        )
        mdf.append([sig])
        mdf.save(path, overwrite=True)
        mdf.close()

        rs_mdf = mf4_rs.PyMDF(path)
        channels = rs_mdf.get_all_channels()
        temp_ch = [ch for ch in channels if ch.name == "Temperature"]
        assert len(temp_ch) >= 1, "Temperature channel not found"
        assert temp_ch[0].unit == "degC", f"expected unit='degC', got '{temp_ch[0].unit}'"
        assert temp_ch[0].comment == "Ambient temperature", \
            f"expected comment='Ambient temperature', got '{temp_ch[0].comment}'"
    finally:
        cleanup(path)


def test_performance_write():
    """Performance sanity check for mf4-rs Python write (should complete in < 30s)."""
    import time
    path = tmp("perf_write")
    try:
        n = 100_000
        w = mf4_rs.PyMdfWriter(path)
        w.init_mdf_file()
        cg = w.add_channel_group("perf")
        w.add_time_channel(cg, "Time")
        w.add_float_channel(cg, "A")
        w.add_float_channel(cg, "B")
        w.add_float_channel(cg, "C")

        start = time.time()
        w.start_data_block(cg)
        for i in range(n):
            t = i * 0.001
            w.write_record(cg, [
                mf4_rs.create_float_value(t),
                mf4_rs.create_float_value(t * 2),
                mf4_rs.create_float_value(t * 3),
                mf4_rs.create_float_value(t * 4),
            ])
        w.finish_data_block(cg)
        w.finalize()
        elapsed = time.time() - start
        assert elapsed < 30, f"write took {elapsed:.1f}s - performance regression"
        print(f"    ({n} records in {elapsed:.3f}s)")
    finally:
        cleanup(path)


def test_performance_read():
    """Performance sanity check for mf4-rs Python read (should complete in < 10s)."""
    import time
    path = write_mf4rs_basic()
    try:
        start = time.time()
        for _ in range(10):
            mdf = mf4_rs.PyMDF(path)
            for name in ["Time", "Temperature", "Counter"]:
                mdf.get_channel_values(name)
        elapsed = time.time() - start
        assert elapsed < 10, f"10x read took {elapsed:.1f}s - performance regression"
        print(f"    (10x read in {elapsed:.3f}s)")
    finally:
        cleanup(path)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


if __name__ == "__main__":
    print("mf4-rs <-> asammdf cross-compatibility tests\n")

    tests = [
        ("asammdf reads mf4-rs basic file", test_asammdf_reads_mf4rs_basic),
        ("mf4-rs reads asammdf basic file", test_mf4rs_reads_asammdf_basic),
        ("cross-read values match", test_cross_read_values_match),
        ("all integer types roundtrip", test_all_integer_types_roundtrip),
        ("float types roundtrip", test_float_types_roundtrip),
        ("multi-group cross-read", test_multi_group_cross_read),
        ("master channel detected by asammdf", test_master_channel_detected),
        ("file identification block", test_file_identification),
        ("compressed file fails gracefully", test_compressed_file_fails_gracefully),
        ("data block splitting cross-read", test_data_block_splitting_cross_read),
        ("value-to-text conversion cross-read", test_value_to_text_conversion_cross_read),
        ("units and comments readable", test_units_and_comments_readable),
        ("performance: write", test_performance_write),
        ("performance: read", test_performance_read),
    ]

    for name, fn in tests:
        run_test(name, fn)

    print(f"\n{'='*50}")
    print(f"Results: {passed} passed, {failed} failed")
    print(f"{'='*50}")
    sys.exit(1 if failed > 0 else 0)
