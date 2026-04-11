#!/usr/bin/env python3
"""
Benchmark: mf4-rs Python bindings vs asammdf
Compares read performance for various scenarios.
"""
import time
import os
import sys
import tempfile
import numpy as np

try:
    import mf4_rs
except ImportError:
    print("ERROR: mf4_rs not installed. Run: maturin develop --release")
    sys.exit(1)

try:
    import asammdf
    from asammdf import MDF as AsamMDF, Signal
except ImportError:
    print("ERROR: asammdf not installed. Run: pip install asammdf")
    sys.exit(1)

TMPDIR = tempfile.gettempdir()


def create_test_file_asammdf(path, n_records, n_float_channels=4):
    """Create an uncompressed test MDF file using asammdf writer."""
    mdf = AsamMDF()
    timestamps = np.arange(n_records, dtype=np.float64) * 0.001
    signals = []
    for i in range(n_float_channels):
        data = timestamps * (i + 2)
        signals.append(Signal(samples=data, timestamps=timestamps, name=f"ch_{i}"))
    mdf.append(signals)
    # Save without compression so mf4-rs can read it
    mdf.save(path, overwrite=True, compression=0)
    mdf.close()


def create_test_file_mf4rs(path, n_records, n_float_channels=4):
    """Create a test MDF file using mf4-rs writer."""
    w = mf4_rs.PyMdfWriter(path)
    w.init_mdf_file()
    cg = w.add_channel_group()
    time_ch = w.add_time_channel(cg, "Time")
    for i in range(n_float_channels):
        w.add_float_channel(cg, f"ch_{i}")

    w.start_data_block(cg)
    for i in range(n_records):
        t = float(i) * 0.001
        values = [mf4_rs.PyDecodedValue.Float(value=t)]
        for j in range(n_float_channels):
            values.append(mf4_rs.PyDecodedValue.Float(value=t * (j + 2)))
        w.write_record(cg, values)
    w.finish_data_block(cg)
    w.finalize()


def bench_read_mf4rs_values(path, channel_names, iterations=5):
    """Benchmark mf4-rs reading with get_channel_values (standard path)."""
    times = []
    total_values = 0
    for _ in range(iterations):
        start = time.perf_counter()
        reader = mf4_rs.PyMDF(path)
        for name in channel_names:
            vals = reader.get_channel_values(name)
            if vals is not None:
                total_values += len(vals)
        elapsed = time.perf_counter() - start
        times.append(elapsed)
    times.sort()
    return times[len(times) // 2], total_values // iterations


def bench_read_mf4rs_f64(path, channel_names, iterations=5):
    """Benchmark mf4-rs reading with get_channel_values_f64 (fast path)."""
    times = []
    total_values = 0
    for _ in range(iterations):
        start = time.perf_counter()
        reader = mf4_rs.PyMDF(path)
        for name in channel_names:
            vals = reader.get_channel_values_f64(name)
            if vals is not None:
                total_values += len(vals)
        elapsed = time.perf_counter() - start
        times.append(elapsed)
    times.sort()
    return times[len(times) // 2], total_values // iterations


def bench_read_mf4rs_numpy(path, channel_names, iterations=5):
    """Benchmark mf4-rs reading with get_channel_values_numpy (numpy path)."""
    times = []
    total_values = 0
    for _ in range(iterations):
        start = time.perf_counter()
        reader = mf4_rs.PyMDF(path)
        for name in channel_names:
            arr = reader.get_channel_values_numpy(name)
            if arr is not None:
                total_values += len(arr)
        elapsed = time.perf_counter() - start
        times.append(elapsed)
    times.sort()
    return times[len(times) // 2], total_values // iterations


def bench_read_asammdf(path, channel_names, iterations=5):
    """Benchmark asammdf reading."""
    times = []
    total_values = 0
    for _ in range(iterations):
        start = time.perf_counter()
        reader = AsamMDF(path)
        for name in channel_names:
            sig = reader.get(name)
            total_values += len(sig.samples)
        reader.close()
        elapsed = time.perf_counter() - start
        times.append(elapsed)
    times.sort()
    return times[len(times) // 2], total_values // iterations


def run_benchmark(label, n_records, n_channels=4):
    """Run a full benchmark comparison."""
    print(f"\n{'='*70}")
    print(f"  {label}: {n_records:,} records x {n_channels} float channels")
    print(f"{'='*70}")

    channel_names = [f"ch_{i}" for i in range(n_channels)]

    # Create test files with both libraries
    path_mf4rs = os.path.join(TMPDIR, f"bench_mf4rs_{n_records}.mf4")
    path_asammdf = os.path.join(TMPDIR, f"bench_asammdf_{n_records}.mf4")

    print(f"  Writing test files...")
    create_test_file_asammdf(path_asammdf, n_records, n_channels)
    create_test_file_mf4rs(path_mf4rs, n_records, n_channels)

    mf4rs_size = os.path.getsize(path_mf4rs)
    asammdf_size = os.path.getsize(path_asammdf)
    print(f"  File sizes: mf4-rs={mf4rs_size/1024:.0f}KB, asammdf={asammdf_size/1024:.0f}KB")

    results = {}

    # --- Benchmark reading mf4-rs-written file ---
    print(f"\n  --- Reading mf4-rs file ---")

    t, nv = bench_read_mf4rs_values(path_mf4rs, channel_names)
    tp = nv / t / 1e6
    print(f"  mf4-rs get_channel_values():       {t:.4f}s  ({tp:.1f}M vals/s)")
    results['mf4rs_values_own'] = t

    t, nv = bench_read_mf4rs_f64(path_mf4rs, channel_names)
    tp = nv / t / 1e6
    print(f"  mf4-rs get_channel_values_f64():   {t:.4f}s  ({tp:.1f}M vals/s)")
    results['mf4rs_f64_own'] = t

    t, nv = bench_read_mf4rs_numpy(path_mf4rs, channel_names)
    tp = nv / t / 1e6
    print(f"  mf4-rs get_channel_values_numpy(): {t:.4f}s  ({tp:.1f}M vals/s)")
    results['mf4rs_numpy_own'] = t

    t, nv = bench_read_asammdf(path_mf4rs, channel_names)
    tp = nv / t / 1e6
    print(f"  asammdf get():                     {t:.4f}s  ({tp:.1f}M vals/s)")
    results['asammdf_mf4rs_file'] = t

    # --- Benchmark reading asammdf-written file ---
    print(f"\n  --- Reading asammdf file ---")

    try:
        t, nv = bench_read_mf4rs_numpy(path_asammdf, channel_names)
        tp = nv / t / 1e6
        print(f"  mf4-rs get_channel_values_numpy(): {t:.4f}s  ({tp:.1f}M vals/s)")
        results['mf4rs_numpy_asammdf_file'] = t
    except Exception as e:
        print(f"  mf4-rs numpy: FAILED ({e})")
        results['mf4rs_numpy_asammdf_file'] = None

    t, nv = bench_read_asammdf(path_asammdf, channel_names)
    tp = nv / t / 1e6
    print(f"  asammdf get():                     {t:.4f}s  ({tp:.1f}M vals/s)")
    results['asammdf_own'] = t

    # --- Summary ---
    print(f"\n  --- Speedup Summary ---")
    speedup_vals = results['asammdf_mf4rs_file'] / results['mf4rs_values_own']
    speedup_f64 = results['asammdf_mf4rs_file'] / results['mf4rs_f64_own']
    speedup_np = results['asammdf_mf4rs_file'] / results['mf4rs_numpy_own']
    print(f"  mf4-rs values() vs asammdf (same file):     {speedup_vals:.2f}x")
    print(f"  mf4-rs values_f64() vs asammdf (same file): {speedup_f64:.2f}x")
    print(f"  mf4-rs numpy() vs asammdf (same file):      {speedup_np:.2f}x")

    if results.get('mf4rs_numpy_asammdf_file'):
        speedup_cross = results['asammdf_own'] / results['mf4rs_numpy_asammdf_file']
        print(f"  mf4-rs numpy vs asammdf (reading asammdf file): {speedup_cross:.2f}x")

    # Cleanup
    os.remove(path_mf4rs)
    os.remove(path_asammdf)

    return results


def main():
    print("MF4 Reading Performance Comparison: mf4-rs vs asammdf")
    print(f"asammdf version: {asammdf.__version__}")
    print(f"numpy version: {np.__version__}")

    all_results = {}
    all_results['100k'] = run_benchmark("Medium file", 100_000, 4)
    all_results['1m'] = run_benchmark("Large file", 1_000_000, 4)

    print(f"\n{'='*70}")
    print("  FINAL COMPARISON")
    print(f"{'='*70}")
    for label, r in all_results.items():
        a = r['asammdf_mf4rs_file']
        v = r['mf4rs_values_own']
        f = r['mf4rs_f64_own']
        n = r['mf4rs_numpy_own']
        print(f"\n  {label}:")
        print(f"    asammdf:              {a:.4f}s")
        print(f"    mf4-rs values():      {v:.4f}s  ({a/v:.2f}x vs asammdf)")
        print(f"    mf4-rs values_f64():  {f:.4f}s  ({a/f:.2f}x vs asammdf)")
        print(f"    mf4-rs numpy():       {n:.4f}s  ({a/n:.2f}x vs asammdf)")


if __name__ == "__main__":
    main()
