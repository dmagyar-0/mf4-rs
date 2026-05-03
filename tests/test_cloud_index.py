"""Python integration test for cloud (HTTP) MDF indexing.

Builds a fixture MF4 file using ``mf4_rs.PyMdfWriter``, serves it from a
local HTTP server with single-range ``Range: bytes=A-B`` support, then
exercises ``PyMdfIndex.from_url`` and the ``*_from_url`` value-read
methods.

Exits 0 (skip) if the bindings are not importable so this can sit in CI
without forcing a maturin build on every runner.
"""

from __future__ import annotations

import http.server
import os
import re
import socketserver
import sys
import tempfile
import threading
import time
from pathlib import Path

try:
    import mf4_rs
except ImportError as e:
    print(f"SKIP: mf4_rs not importable ({e}); run `maturin develop --release` first")
    sys.exit(0)


GROUPS = 5
CHANNELS_PER_GROUP = 5  # 1 master + 4 data
RECORDS = 200


def build_fixture(path: str) -> None:
    w = mf4_rs.PyMdfWriter(path)
    w.init_mdf_file()
    cg_ids: list[str] = []
    cn_ids_per_group: list[list[str]] = []

    for g in range(GROUPS):
        cg = w.add_channel_group(f"Group {g}")
        w.set_channel_group_comment(cg, f"Comment for group {g}")
        cg_ids.append(cg)
        # Master channel (time, FloatLE 64-bit).
        t_id = w.add_time_channel(cg, f"t_{g}")
        cn_ids = [t_id]
        for j in range(1, CHANNELS_PER_GROUP):
            ch = w.add_float_channel(cg, f"ch_{g}_{j}")
            cn_ids.append(ch)
        cn_ids_per_group.append(cn_ids)

    for g, cg in enumerate(cg_ids):
        w.start_data_block(cg)
        for r in range(RECORDS):
            record = [mf4_rs.create_float_value(r * 0.01)]
            for j in range(1, CHANNELS_PER_GROUP):
                record.append(mf4_rs.create_float_value(float(g * 100 + r * j)))
            w.write_record(cg, record)
        w.finish_data_block(cg)

    w.finalize()


class RangeHandler(http.server.SimpleHTTPRequestHandler):
    """Honour single-range ``Range: bytes=A-B`` requests.

    Stays on HTTP/1.0 (the default) so each response closes the TCP
    connection. With HTTP/1.1 keep-alive, ureq 2.x can wedge waiting for
    bytes the server has no intention of sending.
    """

    def do_GET(self):  # noqa: N802
        path = self.translate_path(self.path)
        try:
            f = open(path, "rb")
        except OSError:
            self.send_error(404)
            return
        try:
            data = f.read()
        finally:
            f.close()
        total = len(data)
        rng = self.headers.get("Range")
        if rng:
            m = re.match(r"bytes=(\d+)-(\d*)", rng)
            if m:
                start = int(m.group(1))
                end = int(m.group(2)) if m.group(2) else total - 1
                end = min(end, total - 1)
                if start < total:
                    body = data[start : end + 1]
                    self.send_response(206)
                    self.send_header("Content-Type", "application/octet-stream")
                    self.send_header("Content-Length", str(len(body)))
                    self.send_header("Content-Range", f"bytes {start}-{end}/{total}")
                    self.send_header("Accept-Ranges", "bytes")
                    self.end_headers()
                    self.wfile.write(body)
                    return
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(total))
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()
        self.wfile.write(data)

    def do_HEAD(self):  # noqa: N802
        path = self.translate_path(self.path)
        try:
            size = os.path.getsize(path)
        except OSError:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(size))
        self.send_header("Accept-Ranges", "bytes")
        self.end_headers()

    def log_message(self, *_args, **_kwargs):  # silence noisy stdout
        return


class _Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True


def serve(directory: Path):
    os.chdir(directory)
    httpd = _Server(("127.0.0.1", 0), RangeHandler)
    port = httpd.server_address[1]
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    return httpd, port


def main() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        mf4_path = tmp_path / "fixture.mf4"
        build_fixture(str(mf4_path))
        size = mf4_path.stat().st_size
        print(f"fixture size: {size} bytes")

        httpd, port = serve(tmp_path)
        try:
            url = f"http://127.0.0.1:{port}/fixture.mf4"
            time.sleep(0.05)

            t0 = time.perf_counter()
            idx = mf4_rs.PyMdfIndex.from_url(url)
            print(f"index built in {(time.perf_counter() - t0)*1000:.1f} ms")

            groups = idx.list_channel_groups()
            assert len(groups) == GROUPS, f"got {len(groups)} groups, want {GROUPS}"
            for i, (gi, name, count) in enumerate(groups):
                assert gi == i
                assert name == f"Group {i}", f"group {i} name = {name!r}"
                assert count == CHANNELS_PER_GROUP

            # Read 5 channels via URL.
            t0 = time.perf_counter()
            targets = [
                ("Group 0", "t_0"),
                ("Group 1", "ch_1_2"),
                ("Group 2", "ch_2_4"),
                ("Group 3", "ch_3_1"),
                ("Group 4", "ch_4_3"),
            ]
            for gn, cn in targets:
                vals = idx.read_channel_values_by_group_and_name(gn, cn, str(mf4_path))
                vals_url = idx.read_channel_values_by_name_from_url(cn, url)
                assert len(vals) == RECORDS, f"{gn}/{cn} local len {len(vals)}"
                assert len(vals_url) == RECORDS, f"{gn}/{cn} url len {len(vals_url)}"
                # Spot-check one decoded value matches between local and URL paths.
                assert vals[10] == vals_url[10], (
                    f"{gn}/{cn}[10] differs: local={vals[10]!r} url={vals_url[10]!r}"
                )
            print(f"5 channel reads (local + url) in {(time.perf_counter() - t0)*1000:.1f} ms")

            # Round-trip via JSON to confirm the index is self-contained after
            # being built over HTTP.
            json_path = tmp_path / "fixture.idx.json"
            idx.save_to_file(str(json_path))
            idx2 = mf4_rs.PyMdfIndex.load_from_file(str(json_path))
            vals_a = idx.read_channel_values_by_name_from_url("ch_2_4", url)
            vals_b = idx2.read_channel_values_by_name_from_url("ch_2_4", url)
            assert vals_a == vals_b, "JSON round-trip changed decoded values"
            print(f"index json: {json_path.stat().st_size} bytes")

            print("OK")
            return 0
        finally:
            httpd.shutdown()


if __name__ == "__main__":
    sys.exit(main())
