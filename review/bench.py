#!/usr/bin/env python3
"""Performance: mf4-rs Python bindings vs asammdf 8.8."""
import os, time, gc
import numpy as np
from asammdf import MDF, Signal
import mf4_rs

D = os.path.join(os.path.dirname(os.path.abspath(__file__)), "bench_files")
os.makedirs(D, exist_ok=True)
def p(n): return os.path.join(D, n)

N = 1_000_000
t = np.arange(N, dtype=np.float64) * 0.001
chans = {f"ch{i}": np.random.default_rng(i).random(N) for i in range(4)}

def timeit(fn, *a):
    gc.collect(); t0 = time.perf_counter(); r = fn(*a); dt = time.perf_counter() - t0
    return dt, r

# ---- WRITE ----
def w_mf4rs_loop(n=100_000):
    w = mf4_rs.MdfWriter(p("w_loop.mf4")); w.init_mdf_file()
    cg = w.add_channel_group(None)
    tc = w.add_time_channel(cg, "Time"); w.set_time_channel(tc)
    for name in chans: w.add_float_channel(cg, name)
    w.start_data_block(cg)
    cols = list(chans.values())
    fv = mf4_rs.create_float_value
    for i in range(n):
        w.write_record(cg, [fv(t[i]), fv(cols[0][i]), fv(cols[1][i]), fv(cols[2][i]), fv(cols[3][i])])
    w.finish_data_block(cg); w.finalize()

def w_mf4rs_cols():
    w = mf4_rs.MdfWriter(p("w_cols.mf4")); w.init_mdf_file()
    cg = w.add_channel_group(None)
    tc = w.add_time_channel(cg, "Time"); w.set_time_channel(tc)
    for name in chans: w.add_float_channel(cg, name)
    w.start_data_block(cg)
    w.write_columns_f64(cg, [t] + list(chans.values()))
    w.finish_data_block(cg); w.finalize()

def w_asammdf():
    m = MDF(version="4.10")
    m.append([Signal(v, t, name=k) for k, v in chans.items()])
    m.save(p("w_amdf.mf4"), overwrite=True, compression=0)
    m.close()

def w_asammdf_compressed():
    m = MDF(version="4.10")
    m.append([Signal(v, t, name=k) for k, v in chans.items()])
    m.save(p("w_amdf_dz.mf4"), overwrite=True, compression=2)
    m.close()

LOOP_N = 100_000
dt_loop, _ = timeit(w_mf4rs_loop)
dt_cols, _ = timeit(w_mf4rs_cols)
dt_amdf, _ = timeit(w_asammdf)
dt_amdz, _ = timeit(w_asammdf_compressed)
print(f"WRITE  mf4-rs write_record loop ({LOOP_N:>9,} rec): {dt_loop:7.3f}s  ({dt_loop/LOOP_N*1e6:.2f} us/rec)")
print(f"WRITE  mf4-rs write_columns_f64 ({N:>9,} rec): {dt_cols:7.3f}s  ({dt_cols/N*1e6:.2f} us/rec)")
print(f"WRITE  asammdf append+save      ({N:>9,} rec): {dt_amdf:7.3f}s  ({dt_amdf/N*1e6:.2f} us/rec)")
print(f"WRITE  asammdf save compressed  ({N:>9,} rec): {dt_amdz:7.3f}s")
print(f"SIZE   mf4-rs: {os.path.getsize(p('w_cols.mf4'))/1e6:.1f} MB   asammdf: {os.path.getsize(p('w_amdf.mf4'))/1e6:.1f} MB   asammdf-dz: {os.path.getsize(p('w_amdf_dz.mf4'))/1e6:.1f} MB")

# ---- READ (same mf4-rs-written file for both; then asammdf-written file) ----
for src_label, path in [("mf4-rs-file", p("w_cols.mf4")), ("asammdf-file", p("w_amdf.mf4"))]:
    def r_mf4rs_values():
        m = mf4_rs.Mdf(path)
        return [m.values(f"ch{i}") for i in range(4)]
    def r_mf4rs_read():
        m = mf4_rs.Mdf(path)
        return [m.read(f"ch{i}") for i in range(4)]
    def r_index_values():
        ix = mf4_rs.MdfIndex.from_file(path)
        return [ix.values(f"ch{i}") for i in range(4)]
    def r_asammdf():
        m = MDF(path)
        out = [m.get(f"ch{i}").samples for i in range(4)]
        m.close(); return out
    d1, v1 = timeit(r_mf4rs_values)
    d2, _ = timeit(r_mf4rs_read)
    d3, v3 = timeit(r_index_values)
    d4, v4 = timeit(r_asammdf)
    assert np.allclose(v1[0], v4[0]) and np.allclose(v3[0], v4[0])
    print(f"READ({src_label})  mf4-rs values: {d1:6.3f}s  read(Series): {d2:6.3f}s  index.values: {d3:6.3f}s  asammdf: {d4:6.3f}s")

# index build + reload cost
d5, _ = timeit(lambda: mf4_rs.MdfIndex.from_file(p("w_cols.mf4")))
ix = mf4_rs.MdfIndex.from_file(p("w_cols.mf4")); ix.save(p("ix.json"))
d6, _ = timeit(lambda: mf4_rs.MdfIndex.load(p("ix.json")))
print(f"INDEX  build: {d5:.4f}s   load-json: {d6:.4f}s   json size: {os.path.getsize(p('ix.json'))/1e3:.0f} kB")

# single-channel partial read (the index's design win)
def one_ch_index():
    ix2 = mf4_rs.MdfIndex.load(p("ix.json")); ix2.source = p("w_cols.mf4")
    return ix2.values("ch0")
def one_ch_asammdf():
    m = MDF(p("w_cols.mf4")); s = m.get("ch0").samples; m.close(); return s
d7, _ = timeit(one_ch_index)
d8, _ = timeit(one_ch_asammdf)
print(f"1-CHAN index(json)+read: {d7:.3f}s   asammdf open+get: {d8:.3f}s")
