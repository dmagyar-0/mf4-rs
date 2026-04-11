#!/usr/bin/env python3
"""
Write performance benchmark: mf4-rs Python bindings (old vs new API) vs asammdf.

Compares:
  1. mf4-rs record-at-a-time (write_record loop)
  2. mf4-rs columnar f64 (write_columns_f64 with numpy arrays)
  3. asammdf numpy vectorized (Signal + MDF.append)
"""
import time
import os
import sys
import tempfile
import numpy as np

TMPDIR = tempfile.gettempdir()

try:
    import mf4_rs
except ImportError:
    print("ERROR: mf4_rs not installed. Run: maturin develop --release")
    sys.exit(1)

HAS_ASAMMDF = True
try:
    import asammdf
    from asammdf import MDF as AsamMDF, Signal
except ImportError:
    HAS_ASAMMDF = False
    print("WARNING: asammdf not installed, skipping asammdf benchmarks")


def bench_mf4rs_record_at_a_time(path, n_records, n_channels=4, iterations=3):
    """Old API: write_record in a loop."""
    times = []
    for _ in range(iterations):
        w = mf4_rs.PyMdfWriter(path)
        w.init_mdf_file()
        cg = w.add_channel_group()
        w.add_time_channel(cg, "Time")
        for i in range(n_channels):
            w.add_float_channel(cg, f"ch_{i}")
        w.start_data_block(cg)

        start = time.perf_counter()
        for i in range(n_records):
            t = float(i) * 0.001
            values = [mf4_rs.PyDecodedValue.Float(value=t)]
            for j in range(n_channels):
                values.append(mf4_rs.PyDecodedValue.Float(value=t * (j + 2)))
            w.write_record(cg, values)
        elapsed = time.perf_counter() - start
        times.append(elapsed)

        w.finish_data_block(cg)
        w.finalize()
        os.remove(path)

    times.sort()
    return times[len(times) // 2]


def bench_mf4rs_columns_f64(path, n_records, n_channels=4, iterations=3):
    """New API: write_columns_f64 with numpy arrays."""
    # Pre-build numpy arrays
    timestamps = np.arange(n_records, dtype=np.float64) * 0.001
    cols = [timestamps]
    for i in range(n_channels):
        cols.append(timestamps * (i + 2))

    times = []
    for _ in range(iterations):
        w = mf4_rs.PyMdfWriter(path)
        w.init_mdf_file()
        cg = w.add_channel_group()
        w.add_time_channel(cg, "Time")
        for i in range(n_channels):
            w.add_float_channel(cg, f"ch_{i}")
        w.start_data_block(cg)

        start = time.perf_counter()
        w.write_columns_f64(cg, cols)
        elapsed = time.perf_counter() - start
        times.append(elapsed)

        w.finish_data_block(cg)
        w.finalize()
        os.remove(path)

    times.sort()
    return times[len(times) // 2]


def bench_asammdf_write(path, n_records, n_channels=4, iterations=3):
    """asammdf: Signal + MDF.append (numpy vectorized)."""
    timestamps = np.arange(n_records, dtype=np.float64) * 0.001
    signals = []
    for i in range(n_channels):
        data = timestamps * (i + 2)
        signals.append(Signal(samples=data, timestamps=timestamps, name=f"ch_{i}"))

    times = []
    for _ in range(iterations):
        start = time.perf_counter()
        mdf = AsamMDF()
        mdf.append(signals)
        mdf.save(path, overwrite=True, compression=0)
        mdf.close()
        elapsed = time.perf_counter() - start
        times.append(elapsed)
        os.remove(path)

    times.sort()
    return times[len(times) // 2]


def run_benchmark(label, n_records, n_channels=4):
    print(f"\n{'='*70}")
    print(f"  {label}: {n_records:,} records x {n_channels + 1} channels (time + {n_channels} data)")
    print(f"{'='*70}")

    total_values = n_records * (n_channels + 1)
    total_bytes = total_values * 8  # all f64

    path = os.path.join(TMPDIR, "bench_write_test.mf4")

    # mf4-rs record-at-a-time
    t_old = bench_mf4rs_record_at_a_time(path, n_records, n_channels)
    print(f"  mf4-rs write_record (loop):     {t_old:.4f}s  ({total_bytes/t_old/1e6:.0f} MB/s)")

    # mf4-rs columns
    t_new = bench_mf4rs_columns_f64(path, n_records, n_channels)
    print(f"  mf4-rs write_columns_f64:       {t_new:.4f}s  ({total_bytes/t_new/1e6:.0f} MB/s)")
    print(f"    -> {t_old/t_new:.1f}x faster than record-at-a-time")

    if HAS_ASAMMDF:
        t_asm = bench_asammdf_write(path, n_records, n_channels)
        print(f"  asammdf append+save:            {t_asm:.4f}s  ({total_bytes/t_asm/1e6:.0f} MB/s)")
        print(f"    -> mf4-rs columns is {t_asm/t_new:.1f}x {'faster' if t_new < t_asm else 'slower'} than asammdf")


def verify_columns_correctness():
    """Verify write_columns_f64 produces readable files."""
    print("\n  Verifying write_columns_f64 correctness...")
    path = os.path.join(TMPDIR, "verify_columns.mf4")

    n = 1000
    timestamps = np.arange(n, dtype=np.float64) * 0.001
    ch0 = timestamps * 2.0
    ch1 = timestamps * 3.0

    w = mf4_rs.PyMdfWriter(path)
    w.init_mdf_file()
    cg = w.add_channel_group()
    w.add_time_channel(cg, "Time")
    w.add_float_channel(cg, "ch_0")
    w.add_float_channel(cg, "ch_1")
    w.start_data_block(cg)
    w.write_columns_f64(cg, [timestamps, ch0, ch1])
    w.finish_data_block(cg)
    w.finalize()

    # Read back with mf4-rs
    reader = mf4_rs.PyMDF(path)
    vals = reader.get_channel_values("ch_0")
    assert vals is not None, "Channel ch_0 not found"
    assert len(vals) == n, f"Expected {n} values, got {len(vals)}"
    np.testing.assert_allclose(vals, ch0, rtol=1e-10)

    vals1 = reader.get_channel_values("ch_1")
    np.testing.assert_allclose(vals1, ch1, rtol=1e-10)

    print("  -> Correctness verified!")
    os.remove(path)


def main():
    print("MF4 Write Performance: mf4-rs new API vs old API vs asammdf")
    if HAS_ASAMMDF:
        print(f"  asammdf version: {asammdf.__version__}")
    print(f"  numpy version: {np.__version__}")

    verify_columns_correctness()
    run_benchmark("100K records", 100_000, 4)
    run_benchmark("1M records", 1_000_000, 4)

    print(f"\n{'='*70}")
    print("  DONE")
    print(f"{'='*70}")


if __name__ == "__main__":
    main()
