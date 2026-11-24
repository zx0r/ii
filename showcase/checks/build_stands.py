#!/usr/bin/env python3
"""Rebuild deterministic demonstration inputs using only the Python standard library."""
import base64
import csv
import io
import json
import math
import struct
import tarfile
import zipfile
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE / "scenarios"


def write(name, text):
    (OUT / name).write_text(text, encoding="utf-8")


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def build():
    OUT.mkdir(exist_ok=True)
    sections = ["# Checkout service incident review\n\nSCROLL-BEGIN\n"]
    for minute in range(96):
        latency = 28 + 170 * math.exp(-((minute - 38) / 12) ** 2)
        sections.append(f"\n## Minute {minute:03d}\n\nANCHOR-{minute:03d}\n"
                        f"Checkout p95 latency was {latency:.1f} ms. Request volume was {1400 + minute * 7}/s. "
                        "Проверяем перенос русского текста, 日本語 и объединённый символ e\u0301. "
                        "This paragraph wraps in a narrow split while retaining its position relative to the measurements.\n\n"
                        f"- [{'x' if minute >= 44 else ' '}] Confirm replica health for minute {minute:03d}.\n")
        if minute % 12 == 0:
            sections.append("\n> [!WARNING]\n> Evaluate the burn rate before starting another rollout.\n\n"
                            "$$B = \\frac{r_{errors}}{1-SLO}$$\n\n"
                            "| Signal | Observation |\n|:--|--:|\n| p95 | 180 ms |\n| Burn rate | 6.5 |\n")
    sections.append("\nSCROLL-END\n")
    document = "".join(sections)
    for ext in ["md", "markdown", "mdx", "rmd"]:
        write("incident." + ext, document)
    write("incident.qmd", "---\ntitle: Checkout incident\nformat: html\n---\n\n" + document)
    write("kinetics.chem", r"\ce{2H2 + O2 -> 2H2O} \qquad k = A e^{-E_a/(RT)}" + "\n")
    write("equilibrium.mhchem", r"\ce{NH3 + H2O <=> NH4^+ + OH^-}" + "\n")
    latex = r"""\documentclass{article}
\begin{document}
\section{Queue saturation experiment}
An arrival rate $\lambda$ below the service rate $\mu$ is required for stability.
\[ L = \frac{\rho}{1-\rho}, \qquad W = \frac{1}{\mu-\lambda}. \]
\section{Uncertainty}
The measured response-time interval is
\[ \bar{x} \pm 1.96\frac{s}{\sqrt{n}}. \]
\section{Linear regression}
\[ \hat{\beta} = (X^T X)^{-1}X^T y. \]
The final observation is LATEX-END.
\end{document}
"""
    for ext in ["tex", "latex"]:
        write("queueing." + ext, latex)
    write("queueing.typ", "= Queue saturation experiment\n\n"
          "Typst headings, native mathematics and measurement tables share the document renderer.\n"
          "The following equation is compiled from Typst mathematical notation.\n"
          "$ L = frac(rho, 1 - rho) $\n\n== Measured observations\n"
          "At 75% utilisation, average queue depth was three requests.\n#table(columns: 3, [Utilisation], [Queue depth], [Status], [50%], [1], [healthy], [75%], [3], [watch], [90%], [9], [saturated])\n\nTYPST-END\n")
    mermaid = """flowchart TD
    Receive[Receive checkout] --> Validate[Validate request]
    Validate --> Reserve[Reserve inventory]
    Reserve --> Charge[Charge payment]
    Charge --> Commit[Commit order]
    Commit --> Notify[Publish confirmation]
    Charge --> Retry[Retry transient failure]
    Retry --> Charge
"""
    for ext in ["mmd", "mermaid"]:
        write("checkout." + ext, mermaid)
    dot = 'digraph rollout {\nrankdir=TB;\n'
    dot += '\n'.join(f'  n{i} [label="{label}"];' for i, label in enumerate(
        ["Build", "Integration", "Canary", "Observe", "Production", "Rollback"]))
    dot += '\n n0 -> n1; n1 -> n2; n2 -> n3; n3 -> n4; n3 -> n5; n5 -> n1;\n}\n'
    for ext in ["dot", "gv"]:
        write("rollout." + ext, dot)
    svg = ['<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="720" height="1200" viewBox="0 0 720 1200">']
    svg.append('<text x="24" y="36" font-family="sans-serif" font-size="24" fill="currentColor">Replication lag · Задержка</text>')
    for i in range(30):
        x, y = 24 + i % 3 * 232, 60 + i // 3 * 106
        svg.append(f'<rect x="{x}" y="{y}" width="210" height="76" rx="8" fill="none" stroke="currentColor"/>')
        svg.append(f'<text x="{x+10}" y="{y+28}" font-family="sans-serif" font-size="16" fill="currentColor">Replica {i:02d}</text>')
        svg.append(f'<text x="{x+10}" y="{y+55}" font-family="monospace" font-size="14" fill="currentColor">lag {4+i*3} ms</text>')
    svg.append('</svg>')
    write("replicas.svg", '\n'.join(svg))
    for ext, delim in [("csv", ","), ("tsv", "\t")]:
        output = io.StringIO(newline="")
        writer = csv.writer(output, delimiter=delim, lineterminator="\n")
        writer.writerow(["minute", "requests_per_second", "latency_p95_ms", "region", "operator_note"])
        for i in range(1500):
            note = 'Investigate "payment", retry budget\nKeep idempotency key' if i == 700 else f"RECORD-{i:04d}"
            writer.writerow([i, 1400 + i % 100 * 7, round(28 + 170 * math.exp(-((i % 96 - 38) / 12) ** 2), 2),
                             ["Москва", "東京", "Frankfurt"][i % 3], note])
        write("telemetry." + ext, output.getvalue())
    points = [{"minute": i, "latency": round(28 + 170 * math.exp(-((i - 38) / 12) ** 2), 2)} for i in range(96)]
    vega = {"$schema": "https://vega.github.io/schema/vega-lite/v6.json", "title": "Checkout latency",
            "mark": "line", "data": {"values": points}, "encoding": {
                "x": {"field": "minute", "type": "quantitative"},
                "y": {"field": "latency", "type": "quantitative"}}}
    for ext in ["vl", "vega-lite"]:
        write("latency." + ext, json.dumps(vega, indent=2) + "\n")
    plotly = {"data": [{"type": "scatter", "mode": "lines", "name": "p95",
                        "x": [p["minute"] for p in points], "y": [p["latency"] for p in points]}],
              "layout": {"title": {"text": "Checkout latency"}}}
    write("latency.plotly", json.dumps(plotly, indent=2) + "\n")
    # A real colour heatmap of the same bounded incident model, RGBA8 with transparent margin.
    w, h = 480, 288
    pixels = bytearray()
    for y in range(h):
        pixels.append(0)
        for x in range(w):
            t = math.exp(-((x / 5 - 38) / 12) ** 2) * (0.4 + 0.6 * y / h)
            pixels.extend((int(230*t), int(140*(1-t)), int(220*(1-t)), 255))
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    png += chunk(b"tEXt", b"Description\0Checkout incident heatmap: x=minute 0..95; y=replica cohort; red=high lag")
    png += chunk(b"IDAT", zlib.compress(pixels, 9)) + chunk(b"IEND", b"")
    (OUT / "incident-heatmap.png").write_bytes(png)
    cells = []
    for i in range(16):
        cells.append({"cell_type": "markdown", "metadata": {}, "source": [
            f"## Checkpoint {i:02d}\n", f"NOTEBOOK-{i:02d}\n", "Observed queue length follows $L=\\rho/(1-\\rho)$.\n"]})
        outputs = [{"output_type": "stream", "name": "stdout", "text": [f"checkpoint={i:02d} requests={1400+i*7}\n"]}]
        if i == 4:
            outputs.append({"output_type": "display_data", "metadata": {}, "data": {
                "image/svg+xml": ['<svg xmlns="http://www.w3.org/2000/svg" width="280" height="80"><text x="8" y="40" font-size="22" fill="currentColor">Recovery checkpoint</text></svg>'],
                "text/plain": ["Recovery checkpoint"]}})
        if i == 8:
            outputs.append({"output_type": "display_data", "metadata": {}, "data": {
                "image/png": base64.b64encode(png).decode("ascii"), "text/plain": ["Latency heatmap"]}})
        if i == 10:
            outputs.append({"output_type": "display_data", "metadata": {}, "data": {
                "application/vnd.vegalite.v6+json": vega, "text/plain": ["Checkout latency"]}})
        cells.append({"cell_type": "code", "metadata": {}, "execution_count": i + 1,
                      "source": [f"requests = {1400+i*7}\n", "print(requests)\n"], "outputs": outputs})
    notebook = {"nbformat": 4, "nbformat_minor": 5, "metadata": {"kernelspec": {"display_name": "Python 3", "language": "python", "name": "python3"}}, "cells": cells}
    write("analysis.ipynb", json.dumps(notebook, ensure_ascii=False, indent=2) + "\n")
    payloads = {"incident/report.md": document.encode(), "incident/telemetry.csv": (OUT / "telemetry.csv").read_bytes(),
                "incident/latency.vl": json.dumps(vega).encode(), "incident/checksums.txt": b"Capture collected at 2026-11-23T00:00:00Z\n"}
    for ext in ["zip", "jar"]:
        with zipfile.ZipFile(OUT / ("evidence." + ext), "w", zipfile.ZIP_DEFLATED) as archive:
            contents = dict(payloads)
            if ext == "jar":
                contents["META-INF/MANIFEST.MF"] = b"Manifest-Version: 1.0\r\nImplementation-Title: Incident evidence\r\n\r\n"
            for name, data in contents.items():
                info = zipfile.ZipInfo(name, (2026, 11, 23, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(info, data)
    import hashlib
    wheel = {"incident_evidence/__init__.py": b'"""Recorded incident evidence."""\n',
             "incident_evidence/report.md": document.encode(),
             "incident_evidence-1.0.0.dist-info/METADATA": b"Metadata-Version: 2.1\nName: incident-evidence\nVersion: 1.0.0\n",
             "incident_evidence-1.0.0.dist-info/WHEEL": b"Wheel-Version: 1.0\nGenerator: ii-showcase\nRoot-Is-Purelib: true\nTag: py3-none-any\n"}
    record = io.StringIO(newline="")
    rows = csv.writer(record, lineterminator="\n")
    for name, value in wheel.items():
        digest = base64.urlsafe_b64encode(hashlib.sha256(value).digest()).rstrip(b"=").decode()
        rows.writerow([name, "sha256=" + digest, len(value)])
    rows.writerow(["incident_evidence-1.0.0.dist-info/RECORD", "", ""])
    wheel["incident_evidence-1.0.0.dist-info/RECORD"] = record.getvalue().encode()
    with zipfile.ZipFile(OUT / "incident_evidence-1.0.0-py3-none-any.whl", "w") as archive:
        for name, value in wheel.items():
            archive.writestr(zipfile.ZipInfo(name, (2026, 11, 23, 0, 0, 0)), value)
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w") as archive:
        for name, data in payloads.items():
            info = tarfile.TarInfo(name)
            info.size, info.mtime, info.mode = len(data), 1795392000, 0o644
            archive.addfile(info, io.BytesIO(data))
    (OUT / "evidence.tar").write_bytes(raw.getvalue())
    import gzip
    import bz2
    import lzma
    for ext, data in [("tgz", gzip.compress(raw.getvalue(), mtime=0)), ("tbz2", bz2.compress(raw.getvalue())),
                      ("txz", lzma.compress(raw.getvalue()))]:
        (OUT / ("evidence." + ext)).write_bytes(data)
    # A Zstandard frame containing uncompressed raw blocks, per the frame specification.
    data = raw.getvalue()
    frame = b"\x28\xb5\x2f\xfd\xa0" + struct.pack("<I", len(data))
    for offset in range(0, len(data), 131072):
        block = data[offset:offset+131072]
        header = len(block) << 3 | int(offset + len(block) == len(data))
        frame += header.to_bytes(3, "little") + block
    (OUT / "evidence.tzst").write_bytes(frame)
    print(f"Built {len(list(OUT.iterdir()))} demonstration inputs in {OUT}")


if __name__ == "__main__":
    build()
