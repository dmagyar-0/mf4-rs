"""Python tests for ``mf4_rs.to_column_oriented``.

Builds multi-group source files with asammdf, converts them to the
column-oriented MDF 4.20 layout, and checks that

* asammdf reads the converted file back identically (an independent check of
  the MDF 4.2 remote-master encoding and shared time channels),
* ``mf4_rs.Mdf`` and ``mf4_rs.MdfIndex`` (file and HTTP sources) read the same
  values and timestamps, and
* a cold HTTP read of one signal needs only a handful of range requests.

Exits 0 (skip) if mf4_rs, asammdf or numpy are not importable.
"""

from __future__ import annotations

import http.server
import os
import re
import socketserver
import sys
import tempfile
import threading
import warnings

try:
    import numpy as np
    import mf4_rs
    from asammdf import MDF, Signal
except ImportError as e:  # pragma: no cover
    print(f"SKIP: dependency missing ({e})")
    sys.exit(0)

warnings.simplefilter("ignore")

N_FAST = 20_000
N_SLOW = 700


def build_source(path: str) -> None:
    t_fast = np.arange(N_FAST) * 0.01
    t_slow = np.arange(N_SLOW) * 0.1
    mdf = MDF(version="4.10")
    mdf.append(
        [
            Signal(np.sin(t_fast) * 100, t_fast, name="Speed", unit="km/h"),
            Signal((np.arange(N_FAST) % 6).astype(np.uint8), t_fast, name="Gear"),
            Signal(np.arange(N_FAST, dtype=np.int16) - 5000, t_fast, name="Torque",
                   conversion={"a": 0.5, "b": 10.0}),
            Signal(np.array([f"lbl-{i % 37}".encode() for i in range(N_FAST)]), t_fast,
                   name="Label", encoding="utf-8"),
        ],
        acq_name="Engine",
    )
    mdf.append([Signal(np.arange(N_SLOW, dtype=np.float32) * 1.5, t_slow, name="Pressure",
                       unit="bar")], acq_name="Slow")
    # Same time axis as Engine -> shares its time channel.
    mdf.append([Signal(np.cos(t_fast), t_fast, name="Lateral")], acq_name="Chassis")
    mdf.save(path, overwrite=True, compression=0)


NAMES = ["Speed", "Gear", "Torque", "Label", "Pressure", "Lateral"]


def as_list(values):
    return [v.decode() if isinstance(v, bytes) else v for v in values]


def check_asammdf_roundtrip(src: str, col: str) -> None:
    a, b = MDF(src), MDF(col)
    assert b.version == "4.20", b.version
    for name in NAMES:
        s, c = a.get(name), b.get(name)
        assert np.array_equal(s.timestamps, c.timestamps), f"{name} timestamps"
        if s.samples.dtype.kind in "SO":
            assert as_list(s.samples) == as_list(c.samples), f"{name} samples"
        else:
            assert np.array_equal(s.samples, c.samples), f"{name} samples"
            assert s.samples.dtype == c.samples.dtype, f"{name} dtype"
        assert s.unit == c.unit, f"{name} unit"
    # One channel per group plus the shared time groups.
    assert all(len(g.channels) == 1 for g in b.groups)


def check_mf4rs_reads(src: str, col: str) -> None:
    a = MDF(src)
    readers = {
        "Mdf": mf4_rs.Mdf(col),
        "MdfIndex": mf4_rs.MdfIndex.from_file(col),
    }
    for label, r in readers.items():
        for name in NAMES:
            ref = a.get(name)
            series = r.read(name)
            if ref.samples.dtype.kind in "SO":
                assert as_list(ref.samples) == list(series.values), f"{label} {name}"
            else:
                np.testing.assert_allclose(series.values.astype(float), ref.samples.astype(float),
                                           err_msg=f"{label} {name}")
                np.testing.assert_allclose(r.values(name), ref.samples.astype(float))
            assert len(series.index) == len(ref.timestamps), f"{label} {name} index"


def test_report_and_layout(tmp: str) -> None:
    src, col = os.path.join(tmp, "src.mf4"), os.path.join(tmp, "col.mf4")
    build_source(src)
    rep = mf4_rs.to_column_oriented(src, col, block_size=16 * 1024)
    assert rep.signal_groups == 6, rep
    assert rep.time_groups == 2, rep
    assert rep.shared_time_groups == 1, rep
    assert rep.skipped_channels == [], rep.skipped_channels
    assert rep.file_size == os.path.getsize(col)
    assert rep.metadata_size < 16 * 1024, rep
    # Every non-data block sits before metadata_size.
    layout = mf4_rs.file_layout_from_file(col)
    for b in layout.blocks:
        is_data = b.block_type in ("##DV", "##DT", "##SD")
        assert (b.offset >= rep.metadata_size) == is_data, (b.block_type, b.offset)
    check_asammdf_roundtrip(src, col)
    check_mf4rs_reads(src, col)


def test_options_and_errors(tmp: str) -> None:
    src = os.path.join(tmp, "src.mf4")
    build_source(src)
    col = os.path.join(tmp, "noshare.mf4")
    rep = mf4_rs.to_column_oriented(src, col, share_identical_time=False, memory_budget=1)
    assert rep.time_groups == 3 and rep.shared_time_groups == 0, rep
    check_asammdf_roundtrip(src, col)
    try:
        mf4_rs.to_column_oriented(src, src)
    except mf4_rs.MdfException:
        pass
    else:
        raise AssertionError("converting a file onto itself must fail")
    try:
        mf4_rs.to_column_oriented(os.path.join(tmp, "missing.mf4"), col)
    except mf4_rs.MdfException:
        pass
    else:
        raise AssertionError("missing input must fail")


class RangeHandler(http.server.SimpleHTTPRequestHandler):
    """Single-range ``Range: bytes=A-B`` server that counts requests."""

    requests = 0

    def do_GET(self):  # noqa: N802
        type(self).requests += 1
        with open(self.translate_path(self.path), "rb") as f:
            data = f.read()
        m = re.match(r"bytes=(\d+)-(\d*)", self.headers.get("Range", ""))
        if m:
            start = int(m.group(1))
            end = min(int(m.group(2)) if m.group(2) else len(data) - 1, len(data) - 1)
            body = data[start:end + 1]
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
        else:
            body = data
            self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def test_http_request_counts(tmp: str) -> None:
    src, col = os.path.join(tmp, "src.mf4"), os.path.join(tmp, "col.mf4")
    build_source(src)
    mf4_rs.to_column_oriented(src, col, block_size=16 * 1024)

    handler = lambda *a, **k: RangeHandler(*a, directory=tmp, **k)  # noqa: E731
    with socketserver.ThreadingTCPServer(("127.0.0.1", 0), handler) as srv:
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        url = f"http://127.0.0.1:{srv.server_address[1]}/col.mf4"
        RangeHandler.requests = 0
        idx = mf4_rs.MdfIndex.from_url(url)
        build = RangeHandler.requests
        ref = MDF(src)
        counts = {}
        for name in ["Speed", "Torque", "Pressure", "Lateral"]:
            before = RangeHandler.requests
            values = idx.values(name)
            series = idx.read(name)
            counts[name] = RangeHandler.requests - before
            np.testing.assert_allclose(values, ref.get(name).samples.astype(float))
            np.testing.assert_allclose(series.values, ref.get(name).samples.astype(float))
        srv.shutdown()
    print(f"    from_url build: {build} requests; values()+read() per signal: {counts}")
    assert build <= 2, build
    # values(): 1 (+1 for the lazily resolved conversion of Torque);
    # read(): values + shared time channel (+ conversion again for Torque).
    for name, n in counts.items():
        assert n <= (5 if name == "Torque" else 3), (name, n)


def main() -> int:
    tests = [test_report_and_layout, test_options_and_errors, test_http_request_counts]
    failed = 0
    for t in tests:
        with tempfile.TemporaryDirectory() as tmp:
            try:
                t(tmp)
                print(f"PASS {t.__name__}")
            except Exception as e:  # noqa: BLE001
                failed += 1
                print(f"FAIL {t.__name__}: {type(e).__name__}: {e}")
    print(f"{len(tests) - failed}/{len(tests)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
