#!/usr/bin/env python3
"""Exercise the real daemon, IXPC framing, pagination and decoded KGP RGBA pixels.
No mock renderer and no third-party Python packages. Run from any directory.
"""
import argparse
import base64
import json
import os
import re
import socket
import statistics
import struct
import subprocess
import tempfile
import time
import zlib
from pathlib import Path

from build_stands import build

ROOT = Path(__file__).resolve().parents[2]
HEADER = struct.Struct("<IHHIIIIHHHH")
ANSI = re.compile(rb"\x1b\[[0-?]*[ -/]*[@-~]")


def exact(stream, n):
    data = bytearray()
    while len(data) < n:
        chunk = stream.recv(n-len(data))
        if not chunk:
            raise AssertionError("truncated IXPC response")
        data.extend(chunk)
    return bytes(data)


def exchange(address, path, skip=0, cols=80, light=False):
    payload = struct.pack("<I", skip) + str(path.resolve()).encode()
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(10)
        stream.connect(address)
        stream.sendall(HEADER.pack(0x49585043, 1, 5, skip, 99, 4+int(light), len(payload), cols, 12, 9, 18) + payload)
        h = HEADER.unpack(exact(stream, 32))
        assert h[:5] == (0x49585043, 1, 0x8005, skip, 99), h
        assert h[5] <= h[6] <= 67108864, h
        body = exact(stream, h[6])
    return body[:h[5]], body[h[5]:]


def pixels(apc):
    pending, shape, count = bytearray(), None, 0
    for match in re.finditer(rb"\x1b_G(.*?)\x1b\\", apc, re.S):
        params, _, data = match.group(1).partition(b";")
        fields = dict(item.split(b"=", 1) for item in params.split(b",") if b"=" in item)
        if fields.get(b"a") == b"t":
            assert fields[b"t"] == b"d" and fields[b"f"] in (b"32", b"100")
            fmt = fields[b"f"]
            assert not pending
            shape = int(fields[b"s"]), int(fields[b"v"])
        if data:
            pending.extend(data)
        if fields.get(b"m") == b"0":
            rgba = base64.b64decode(pending, validate=True)
            if fmt == b"100":
                assert rgba.startswith(b"\x89PNG\r\n\x1a\n")
                chunks = bytearray()
                at = 8
                while at < len(rgba):
                    n = struct.unpack(">I", rgba[at:at+4])[0]
                    if rgba[at+4:at+8] == b"IHDR":
                        w,h,depth,color = struct.unpack(">IIBB", rgba[at+8:at+18])
                        assert (w,h) == shape and depth == 8 and color == 6
                    if rgba[at+4:at+8] == b"IDAT": chunks.extend(rgba[at+8:at+8+n])
                    at += 12+n
                raw = zlib.decompress(chunks)
                stride = shape[0]*4+1
                assert len(raw) == stride*shape[1]
                assert all(raw[row*stride] == 0 for row in range(shape[1])), "expected unfiltered PNG"
                rgba = b"".join(raw[row*stride+1:(row+1)*stride] for row in range(shape[1]))
            assert shape and len(rgba) == shape[0]*shape[1]*4
            assert any(rgba[3::4]), "fully transparent image"
            assert any(rgba[i:i+4] != rgba[:4] for i in range(4, len(rgba), 4)), "solid-colour image"
            pending.clear()
            count += 1
    assert not pending, "unfinished KGP image"
    return count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/ii")
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    results = []
    with tempfile.TemporaryDirectory(prefix="ii-verify-") as tmp:
        scenarios = Path(tmp) / "scenarios"
        build(scenarios)
        address = str(Path(tmp) / "ii.sock")
        run_env=dict(os.environ,XDG_CACHE_HOME=str(Path(tmp)/'cache'))
        daemon = subprocess.Popen([str(args.binary.resolve()), "--daemon", "--socket", address], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=run_env)
        try:
            for _ in range(200):
                if Path(address).exists(): break
                if daemon.poll() is not None: raise AssertionError(daemon.stderr.read().decode())
                time.sleep(.01)
            extensions = {p.suffix for p in scenarios.iterdir()}
            inputs = sorted(scenarios.iterdir()) + [
                p for p in sorted((ROOT / "showcase").iterdir())
                if p.name != "README.md" and p.is_file() and p.suffix in extensions]
            for path in inputs:
                if not path.is_file(): continue
                start = time.perf_counter()
                top = exchange(address, path)
                cold_ms = (time.perf_counter()-start)*1000
                image_count = pixels(top[0])
                assert top[1], path
                assert top != exchange(address, path, 12), (path, "scroll did not advance")
                assert exchange(address, path, 100000) == (b"", b""), (path, "EOF leaked data")
                samples = []
                for _ in range(7):
                    start = time.perf_counter()
                    assert exchange(address, path) == top, (path, "scroll back changed frame")
                    samples.append((time.perf_counter()-start)*1000)
                for cols, light in [(40, False), (80, True)]:
                    graphics, text = exchange(address, path, cols=cols, light=light)
                    pixels(graphics)
                    assert text and ANSI.sub(b"", text).count(b"\n") <= 12
                cli = subprocess.run([str(args.binary.resolve()), "--socket", address, "--machine", "--size", "80x12", "--cell-size", "9x18", "--theme", "dark", "--", str(path)], capture_output=True, check=True, env=run_env)
                n = struct.unpack("<I", cli.stdout[:4])[0]
                assert (cli.stdout[4:4+n], cli.stdout[4+n:]) == top, (path, "CLI/daemon mismatch")
                results.append(dict(file=str(Path("scenarios") / path.name if path.parent == scenarios else path.name), cold_ms=round(cold_ms,3), warm_median_ms=round(statistics.median(samples),3), apc_bytes=len(top[0]), text_bytes=len(top[1]), decoded_images=image_count))
                print(f"PASS {path.name}: {image_count} images, warm {statistics.median(samples):.3f} ms")
            # Assert the final dataset record is reachable through visual scrolling.
            csv = scenarios / "telemetry.csv"
            found = False
            for skip in range(0, 10000, 12):
                _, text = exchange(address, csv, skip)
                if not text: break
                if b"1499" in ANSI.sub(b"", text): found = True
            assert found, "last CSV record was lost"
        finally:
            daemon.terminate()
            daemon.wait(timeout=5)
    report = dict(binary=str(args.binary.resolve()), cases=len(results), results=results)
    if args.report: args.report.write_text(json.dumps(report, indent=2)+"\n")
    print(f"Verified {len(results)} real inputs, both themes, narrow geometry, EOF, scroll back and CLI parity.")


if __name__ == "__main__": main()
