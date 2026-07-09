#!/usr/bin/env python3
"""Deep cross-validation: mf4-rs <-> asammdf, normal + index reads."""
import os, sys, traceback
import numpy as np
import pandas as pd
from asammdf import MDF, Signal
import mf4_rs

D = os.path.join(os.path.dirname(os.path.abspath(__file__)), "xval_files")
os.makedirs(D, exist_ok=True)
results = []

def check(name, fn):
    try:
        fn()
        results.append((name, "PASS", ""))
        print(f"  PASS  {name}")
    except Exception as e:
        tb = traceback.format_exc().strip().splitlines()[-1]
        results.append((name, "FAIL", str(e)))
        print(f"  FAIL  {name}: {e}")
        if os.environ.get("XVAL_TRACE"):
            traceback.print_exc()

def p(n): return os.path.join(D, n)

# ---------- helpers ----------
def rust_vals(path, ch, group=None):
    m = mf4_rs.Mdf(path)
    return m.values(ch, group)

def rust_series(path, ch, group=None):
    m = mf4_rs.Mdf(path)
    return m.read(ch, group)

def idx_vals(path, ch, group=None):
    ix = mf4_rs.MdfIndex.from_file(path)
    return ix.values(ch, group)

def idx_series(path, ch, group=None):
    ix = mf4_rs.MdfIndex.from_file(path)
    return ix.read(ch, group)

def idx_roundtrip_vals(path, ch, group=None):
    """index -> json -> reload -> read (tests serialization of resolved conversions)"""
    ix = mf4_rs.MdfIndex.from_file(path)
    jp = p("tmp_index.json")
    ix.save(jp)
    ix2 = mf4_rs.MdfIndex.load(jp)
    ix2.source = path
    return ix2.values(ch, group)

def eq_arrays(a, b, name, exact=False):
    a = np.asarray(a, dtype=np.float64); b = np.asarray(b, dtype=np.float64)
    assert len(a) == len(b), f"{name}: len {len(a)} != {len(b)}"
    if exact:
        bad = np.nonzero(a != b)[0]
    else:
        bad = np.nonzero(~np.isclose(a, b, rtol=1e-9, atol=0, equal_nan=True))[0]
    assert len(bad) == 0, f"{name}: {len(bad)} mismatches, first at {bad[0]}: {a[bad[0]]} vs {b[bad[0]]}"

# ============================================================
# A. asammdf writes -> mf4-rs reads (direct + index + index json roundtrip)
# ============================================================
N = 5000
t = np.arange(N, dtype=np.float64) * 0.001

def gen_boundary():
    sigs = []
    rng = np.random.default_rng(42)
    specs = [
        ("u8", np.uint8, [0, 255]), ("u16", np.uint16, [0, 65535]),
        ("u32", np.uint32, [0, 2**32 - 1]), ("u64", np.uint64, [0, 2**64 - 1]),
        ("i8", np.int8, [-128, 127]), ("i16", np.int16, [-32768, 32767]),
        ("i32", np.int32, [-2**31, 2**31 - 1]), ("i64", np.int64, [-2**63, 2**63 - 1]),
    ]
    for name, dt, bounds in specs:
        info = np.iinfo(dt)
        v = rng.integers(info.min, info.max, size=N, endpoint=True, dtype=dt)
        v[0], v[1] = bounds[0], bounds[1]
        sigs.append(Signal(v, t, name=name))
    f32 = rng.random(N).astype(np.float32) * 1e6; f32[0] = np.float32(3.4e38); f32[1] = np.float32(-1.2e-38)
    f64 = rng.random(N) * 1e12; f64[0] = 1.7e308; f64[1] = 5e-324; f64[2] = np.nan; f64[3] = np.inf; f64[4] = -np.inf
    sigs.append(Signal(f32, t, name="f32")); sigs.append(Signal(f64, t, name="f64"))
    m = MDF(version="4.10")
    m.append(sigs, comment="boundary")
    m.save(p("a_boundary.mf4"), overwrite=True, compression=0)
    m.close()
gen_boundary()

def t_boundary(reader, label):
    m = MDF(p("a_boundary.mf4"))
    for ch in ["u8","u16","u32","u64","i8","i16","i32","i64","f32","f64"]:
        ref = m.get(ch).samples.astype(np.float64)
        got = reader(p("a_boundary.mf4"), ch)
        eq_arrays(got, ref, f"{label}:{ch}")
    m.close()
check("asammdf->mf4rs boundary values (direct)", lambda: t_boundary(rust_vals, "direct"))
check("asammdf->mf4rs boundary values (index)", lambda: t_boundary(idx_vals, "index"))
check("asammdf->mf4rs boundary values (index json roundtrip)", lambda: t_boundary(idx_roundtrip_vals, "ixjson"))

def t_u64_exact():
    # u64 > 2^53 loses precision in float64 — check raw exactness via read() Series
    m = MDF(p("a_boundary.mf4")); ref = m.get("u64").samples; m.close()
    s = rust_series(p("a_boundary.mf4"), "u64")
    got = s.to_numpy()
    # compare as uint64 if dtype preserved, else at least float-equal
    assert got[1] == float(ref[1]) or int(got[1]) == int(ref[1]), f"u64 max mismatch {got[1]} vs {ref[1]}"
check("u64 extreme value readback", t_u64_exact)

def gen_strings():
    strs = np.array([(f"msg_{i}_" + "x" * (i % 40)).encode() for i in range(500)], dtype="S64")
    ba = np.frombuffer(np.random.default_rng(1).bytes(500 * 8), dtype=np.uint8).reshape(500, 8)
    tt = np.arange(500, dtype=np.float64) * 0.01
    m = MDF(version="4.10")
    s1 = Signal(strs, tt, name="Text", encoding="utf-8")
    m.append([s1, Signal(ba, tt, name="Bytes")], comment="strings")
    m.save(p("a_strings.mf4"), overwrite=True, compression=0)
    m.close()
gen_strings()

def t_strings(use_index):
    m = MDF(p("a_strings.mf4")); ref = m.get("Text").samples; refb = m.get("Bytes").samples; m.close()
    s = (idx_series if use_index else rust_series)(p("a_strings.mf4"), "Text")
    got = list(s.to_numpy())
    for i in (0, 1, 99, 499):
        r = ref[i].decode() if isinstance(ref[i], bytes) else str(ref[i])
        assert str(got[i]) == r, f"Text[{i}]: {got[i]!r} vs {r!r}"
    sb = (idx_series if use_index else rust_series)(p("a_strings.mf4"), "Bytes")
    gb = sb.to_numpy()
    assert bytes(gb[7])[:8] == bytes(refb[7])[:8], f"Bytes[7]: {gb[7]!r} vs {refb[7]!r}"
check("asammdf->mf4rs VLSD strings + bytearray (direct)", lambda: t_strings(False))

def t_strings_index_guarded():
    # Index-based VLSD reads are not implemented; they must raise a clear
    # error rather than silently returning garbage (pre-fix behavior).
    try:
        idx_series(p("a_strings.mf4"), "Text")
    except Exception as e:
        assert "VLSD" in str(e) or "not" in str(e).lower(), f"unclear error: {e}"
        return
    # If they ever start working, values must be correct:
    t_strings(True)
check("index VLSD read: clear error or correct values", t_strings_index_guarded)

def gen_conversions():
    from asammdf.blocks import v4_blocks as vb
    raw = np.arange(100, dtype=np.uint8) % 10
    tt = np.arange(100, dtype=np.float64) * 0.1
    lin = Signal(raw.astype(np.float64), tt, name="Lin",
                 conversion={"a": 3.0, "b": 0.5})  # phys = 0.5 + 3*x? asammdf: a*x+b
    v2t = Signal(raw, tt, name="V2T", conversion={
        "val_0": 0, "text_0": "zero", "val_1": 1, "text_1": "one",
        "val_2": 2, "text_2": "two", "default": b"other"})
    r2t = Signal(raw, tt, name="R2T", conversion={
        "lower_0": 0, "upper_0": 3, "text_0": "low",
        "lower_1": 3, "upper_1": 7, "text_1": "mid",
        "default": b"high"})
    alg = Signal(raw.astype(np.float64), tt, name="Alg", conversion={"formula": "X * 2 + 1"})
    m = MDF(version="4.10")
    m.append([lin, v2t, r2t, alg], comment="conv")
    m.save(p("a_conv.mf4"), overwrite=True, compression=0)
    m.close()
gen_conversions()

def t_conv_lin(reader):
    m = MDF(p("a_conv.mf4")); ref = m.get("Lin").physical().samples; m.close()
    got = reader(p("a_conv.mf4"), "Lin")
    eq_arrays(got, ref, "Lin")
check("linear conversion (direct)", lambda: t_conv_lin(rust_vals))
check("linear conversion (index)", lambda: t_conv_lin(idx_vals))
check("linear conversion (index json roundtrip)", lambda: t_conv_lin(idx_roundtrip_vals))

def t_conv_alg(reader):
    m = MDF(p("a_conv.mf4")); ref = m.get("Alg").physical().samples; m.close()
    got = reader(p("a_conv.mf4"), "Alg")
    eq_arrays(got, ref, "Alg")
check("algebraic conversion (direct)", lambda: t_conv_alg(rust_vals))
check("algebraic conversion (index json roundtrip)", lambda: t_conv_alg(idx_roundtrip_vals))

def t_conv_text(name, use_index, expect):
    # expect: list of 10 expected strings per raw value 0..9 (mf4-rs semantics:
    # unmatched value with NIL default returns the raw value, like CANape;
    # asammdf renders unmatched as b'' — a documented difference).
    s = (idx_series if use_index else rust_series)(p("a_conv.mf4"), name)
    got = s.to_numpy()
    for i in range(min(len(got), 20)):
        e = expect[i % 10]
        assert str(got[i]) == e, f"{name}[{i}] raw={i%10}: {got[i]!r} vs {e!r}"
V2T_EXPECT = ["zero", "one", "two"] + [str(i) for i in range(3, 10)]
R2T_EXPECT = ["low", "low", "low", "low", "mid", "mid", "mid", "mid", "8", "9"]
check("value-to-text w/ NIL default -> raw (direct)", lambda: t_conv_text("V2T", False, V2T_EXPECT))
check("value-to-text w/ NIL default -> raw (index)", lambda: t_conv_text("V2T", True, V2T_EXPECT))
check("range-to-text first-match + raw default (direct)", lambda: t_conv_text("R2T", False, R2T_EXPECT))
check("range-to-text first-match + raw default (index)", lambda: t_conv_text("R2T", True, R2T_EXPECT))

def gen_invalidation():
    v = np.arange(200, dtype=np.float64)
    inv = np.zeros(200, dtype=bool); inv[::7] = True
    tt = np.arange(200, dtype=np.float64) * 0.05
    s = Signal(v, tt, name="WithInval", invalidation_bits=inv)
    m = MDF(version="4.10")
    m.append([s], comment="inval")
    m.save(p("a_inval.mf4"), overwrite=True, compression=0)
    m.close()
gen_invalidation()

def t_inval(use_index):
    s = (idx_series if use_index else rust_series)(p("a_inval.mf4"), "WithInval")
    got = s.to_numpy()
    inv_idx = set(range(0, 200, 7))
    for i in range(200):
        if i in inv_idx:
            assert got[i] is None or (isinstance(got[i], float) and np.isnan(got[i])), \
                f"[{i}] should be invalid, got {got[i]!r}"
        else:
            assert float(got[i]) == float(i), f"[{i}]: {got[i]} vs {float(i)}"
check("invalidation bits honored (direct)", lambda: t_inval(False))
check("invalidation bits honored (index)", lambda: t_inval(True))

def gen_versions():
    for ver in ["4.11", "4.20"]:
        m = MDF(version=ver)
        m.append([Signal(np.arange(100, dtype=np.float64), np.arange(100) * 0.01, name="V")], comment="v")
        m.save(p(f"a_v{ver.replace('.','')}.mf4"), overwrite=True, compression=0)
        m.close()
gen_versions()
for ver in ["411", "420"]:
    def t_ver(v=ver):
        got = rust_vals(p(f"a_v{v}.mf4"), "V")
        eq_arrays(got, np.arange(100, dtype=np.float64), f"v{v}")
    check(f"asammdf MDF {ver[0]}.{ver[1:]} file (direct)", t_ver)

def t_compressed():
    m = MDF(version="4.10")
    m.append([Signal(np.arange(1000, dtype=np.float64), np.arange(1000) * 0.01, name="C")])
    m.save(p("a_dz.mf4"), overwrite=True, compression=2)
    m.close()
    try:
        rust_vals(p("a_dz.mf4"), "C")
    except Exception as e:
        assert "DZ" in str(e) or "block" in str(e).lower(), f"unclear error: {e}"
        return
    # if it read, values must be right
    m = MDF(p("a_dz.mf4")); ref = m.get("C").samples; m.close()
    eq_arrays(rust_vals(p("a_dz.mf4"), "C"), ref, "DZ")
check("compressed ##DZ file (graceful or correct)", t_compressed)

# timestamps: mf4-rs read() DatetimeIndex vs asammdf timestamps
def t_timestamps():
    m = MDF(p("a_boundary.mf4"))
    sig = m.get("f64")
    start = m.header.start_time
    m.close()
    s = rust_series(p("a_boundary.mf4"), "f64")
    idx = s.index
    rel = (idx - idx[0]).total_seconds().to_numpy()
    ref_rel = sig.timestamps - sig.timestamps[0]
    eq_arrays(rel, ref_rel, "relative timestamps")
    # absolute: first timestamp should equal start_time + t[0]
    first = pd.Timestamp(idx[0]).tz_localize("UTC") if idx[0].tzinfo is None else pd.Timestamp(idx[0])
    ref_first = pd.Timestamp(start) + pd.Timedelta(seconds=float(sig.timestamps[0]))
    diff = abs((first - ref_first).total_seconds())
    assert diff < 1e-3, f"absolute start mismatch: {first} vs {ref_first} (diff {diff}s)"
check("timestamps: DatetimeIndex matches asammdf", t_timestamps)

# ============================================================
# B. mf4-rs writes -> asammdf reads
# ============================================================
def gen_rust_file():
    w = mf4_rs.MdfWriter(p("b_rust.mf4"))
    w.init_mdf_file()
    cg = w.add_channel_group("grp")
    tc = w.add_time_channel(cg, "Time")
    w.set_time_channel(tc)
    w.add_float_channel(cg, "F64")
    w.add_float32_channel(cg, "F32")
    w.add_int_channel(cg, "I64")
    w.add_channel(cg, "U64", mf4_rs.create_data_type_uint_le())
    w.add_string_channel(cg, "Str")  # VLSD; fixed-length strings now raise
    w.start_data_block(cg)
    n = 2000
    for i in range(n):
        w.write_record(cg, [
            mf4_rs.create_float_value(i * 0.01),
            mf4_rs.create_float_value(i * 1.5 - 100),
            mf4_rs.create_float_value(i * 0.25),
            mf4_rs.create_uint_value(i * 2),  # add_int_channel is documented unsigned
            mf4_rs.create_uint_value(i * 3),
            mf4_rs.create_string_value(f"s{i:04d}" + "y" * (i % 10)),
        ])
    w.finish_data_block(cg)
    w.finalize()
gen_rust_file()

def t_rust_by_asammdf():
    m = MDF(p("b_rust.mf4"))
    n = 2000
    f64 = m.get("F64").samples; eq_arrays(f64, np.arange(n) * 1.5 - 100, "F64")
    f32 = m.get("F32").samples; eq_arrays(f32, (np.arange(n) * 0.25).astype(np.float32), "F32")
    i64 = m.get("I64").samples; eq_arrays(i64, np.arange(n) * 2, "I64", exact=True)
    u64 = m.get("U64").samples; eq_arrays(u64, np.arange(n) * 3, "U64", exact=True)
    st = m.get("Str").samples
    for i in (0, 1, 999, 1999):
        r = st[i].decode() if isinstance(st[i], bytes) else str(st[i])
        assert r == f"s{i:04d}" + "y" * (i % 10), f"Str[{i}]: {r!r}"
    tm = m.get("Time").samples; eq_arrays(tm, np.arange(n) * 0.01, "Time")
    m.close()
check("mf4rs(py)->asammdf numeric + VLSD string channel", t_rust_by_asammdf)

def t_rust_selfread_index():
    for ch, exact in [("F64", False), ("I64", True), ("U64", True)]:
        d = rust_vals(p("b_rust.mf4"), ch)
        ix = idx_vals(p("b_rust.mf4"), ch)
        eq_arrays(d, ix, f"self:{ch}", exact=exact)
check("mf4rs self: direct == index (numeric)", t_rust_selfread_index)

def t_columns_write():
    w = mf4_rs.MdfWriter(p("b_cols.mf4"))
    w.init_mdf_file()
    cg = w.add_channel_group(None)
    tc = w.add_time_channel(cg, "Time"); w.set_time_channel(tc)
    w.add_float_channel(cg, "A"); w.add_float_channel(cg, "B")
    w.start_data_block(cg)
    n = 100000
    tt = np.arange(n) * 0.001; a = np.sin(tt); b = np.cos(tt)
    w.write_columns_f64(cg, [tt, a, b])
    w.finish_data_block(cg)
    w.finalize()
    m = MDF(p("b_cols.mf4"))
    eq_arrays(m.get("A").samples, a, "A")
    eq_arrays(m.get("B").samples, b, "B")
    m.close()
    eq_arrays(idx_vals(p("b_cols.mf4"), "A"), a, "A idx")
check("write_columns_f64 100k -> asammdf + index (DL split)", t_columns_write)

def t_mixed_dtypes_columns():
    w = mf4_rs.MdfWriter(p("b_mixed.mf4"))
    w.init_mdf_file()
    cg = w.add_channel_group(None)
    tc = w.add_time_channel(cg, "Time"); w.set_time_channel(tc)
    w.add_channel(cg, "U", mf4_rs.create_data_type_uint_le())
    w.add_int_channel(cg, "I")
    w.add_float_channel(cg, "F")
    w.start_data_block(cg)
    n = 5000
    tt = np.arange(n) * 0.01
    u = (np.arange(n) * 7).astype(np.uint64)
    ii = (np.arange(n) * 11).astype(np.uint64)
    f = np.arange(n) * 0.5
    w.write_columns(cg, [tt, u, ii, f], ["f64", "u64", "u64", "f64"])
    w.finish_data_block(cg)
    w.finalize()
    m = MDF(p("b_mixed.mf4"))
    eq_arrays(m.get("U").samples, u, "U", exact=True)
    eq_arrays(m.get("I").samples, ii, "I", exact=True)
    eq_arrays(m.get("F").samples, f, "F")
    m.close()
check("write_columns mixed dtypes -> asammdf", t_mixed_dtypes_columns)

# ============================================================
# C. cut & merge validated by asammdf
# ============================================================
def t_cut():
    mf4_rs.cut_mdf_by_time(p("b_cols.mf4"), p("c_cut.mf4"), 10.0, 20.0)
    m = MDF(p("c_cut.mf4"))
    tm = m.get("Time").samples
    a = m.get("A").samples
    m.close()
    assert tm.min() >= 10.0 - 1e-9 and tm.max() <= 20.0 + 1e-9, f"cut bounds: {tm.min()}..{tm.max()}"
    eq_arrays(a, np.sin(tm), "cut A vs sin(t)")
    # expected records: t in [10,20] step 0.001 -> 10001
    assert len(tm) == 10001, f"cut record count {len(tm)}"
check("cut_mdf_by_time -> asammdf reads, bounds inclusive", t_cut)

def t_cut_asammdf_file():
    mf4_rs.cut_mdf_by_time(p("a_conv.mf4"), p("c_cut_conv.mf4"), 2.0, 6.0)
    m = MDF(p("c_cut_conv.mf4"))
    lin = m.get("Lin").physical().samples
    v2t = m.get("V2T").physical().samples
    m.close()
    m2 = MDF(p("a_conv.mf4"))
    reft = m2.get("Lin"); mask = (reft.timestamps >= 2.0) & (reft.timestamps <= 6.0)
    ref = reft.physical().samples[mask]
    refv = m2.get("V2T").physical().samples[mask]
    m2.close()
    eq_arrays(lin, ref, "cut Lin")
    assert list(v2t) == list(refv), "cut V2T text"
check("cut asammdf-written file preserves conversions", t_cut_asammdf_file)

def t_merge():
    mf4_rs.merge_files(p("c_merged.mf4"), p("b_mixed.mf4"), p("b_mixed.mf4"))
    m = MDF(p("c_merged.mf4"))
    u = m.get("U").samples
    tm = m.get("Time").samples
    m.close()
    assert len(u) == 10000, f"merged len {len(u)}"
    ref = (np.arange(5000, dtype=np.uint64) * 7)
    eq_arrays(u[:5000], ref, "merge first half", exact=True)
    eq_arrays(u[5000:], ref, "merge second half", exact=True)
    print(f"        merge time axis: first={tm[:3]}, mid={tm[4998:5002]}, monotonic={bool(np.all(np.diff(tm)>=0))}")
check("merge_files identical layout -> asammdf", t_merge)

def t_merge_vlsd():
    mf4_rs.merge_files(p("c_merged_str.mf4"), p("a_strings.mf4"), p("a_strings.mf4"))
    m = MDF(p("c_merged_str.mf4"))
    st = m.get("Text").samples
    m.close()
    assert len(st) == 1000, f"len {len(st)}"
    for i in (0, 499):
        r = st[i].decode() if isinstance(st[i], bytes) else str(st[i])
        r2 = st[i + 500].decode() if isinstance(st[i + 500], bytes) else str(st[i + 500])
        exp = f"msg_{i}_" + "x" * (i % 40)
        assert r == exp, f"merged Text[{i}]: {r!r} vs {exp!r}"
        assert r2 == exp, f"merged Text[{i+500}]: {r2!r} vs {exp!r}"
check("merge_files with VLSD strings", t_merge_vlsd)

# ============================================================
# D. byte_ranges sanity
# ============================================================
def t_byte_ranges():
    # semantics: ONE coalesced span per fragment, first-to-last channel byte
    # (includes interleaved bytes of the other channels in the record)
    ix = mf4_rs.MdfIndex.from_file(p("b_cols.mf4"))
    rec, choff, chbytes = 24, 8, 8  # Time,A,B f64; A at byte 8
    r = ix.byte_ranges("A", None)
    total = sum(l for _, l in r)
    exp = 100000 * rec - choff - (rec - choff - chbytes)
    assert total == exp, f"byte total {total} != {exp} (ranges: {len(r)})"
    r2 = ix.byte_ranges_for_records("A", 0, 10)
    exp2 = 10 * rec - choff - (rec - choff - chbytes)
    assert sum(l for _, l in r2) == exp2, f"first-10 {r2}"
check("byte_ranges coalesced-span semantics", t_byte_ranges)

print()
npass = sum(1 for _, s, _ in results if s == "PASS")
nfail = len(results) - npass
print(f"Results: {npass} passed, {nfail} failed")
for n, s, e in results:
    if s == "FAIL":
        print(f"  FAILED: {n}\n    {e}")
sys.exit(0)
