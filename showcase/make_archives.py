#!/usr/bin/env python3
"""
Build all archive showcase files for ii in-memory — no staging dir.
Formats: zip tar tgz(gz) tbz2(bz2) txz(xz) tzst(zst) jar whl
"""
import io, os, sys, tarfile, zipfile, time

HERE = os.path.dirname(os.path.abspath(__file__))

# ── Synthetic source tree to pack ──────────────────────────────────────
FILES: list[tuple[str, str]] = [
    ("ii-0.1.0/README.md", (
        "# ii\n"
        "Zero-overhead Sans-I/O terminal render engine -- Kitty Graphics Protocol\n"
        "\n"
        "Architecture:\n"
        "  SIMD memchr3 -> L0 wymix VRAM -> L1 64-slot ring -> L2 BLAKE3+LZ4 CAS -> KGP U+10EEEE\n"
        "\n"
        "Supported file formats:\n"
        "  .md .qmd .rmd .mdx .ipynb .tex .latex .typ\n"
        "  .mmd .mermaid .dot .gv .svg .svgz .csv .tsv\n"
        "  .png .vl .vegalite .plotly .ttf .otf .ttc\n"
        "  .zip .tar .tgz .tbz2 .txz .tzst .7z .rar\n"
        "  .iso .dmg .pkg .deb .rpm .whl .jar\n"
    )),
    ("ii-0.1.0/Cargo.toml", (
        "[package]\n"
        'name = "ii"\n'
        'version = "0.1.0"\n'
        'edition = "2024"\n'
        'rust-version = "1.96"\n'
        "\n"
        "[dependencies]\n"
        'memchr      = "2.8.3"\n'
        'blake3      = "1.8.7"\n'
        'lz4_flex    = { version = "0.14.0", default-features = false, features = ["std"] }\n'
        'resvg       = { version = "0.49.0", default-features = false, features = ["text","system-fonts"] }\n'
        'ratex-parser = "0.1.15"\n'
        'mermaid-svg  = "0.7.0"\n'
        'png          = { version = "0.18", default-features = false }\n'
    )),
    ("ii-0.1.0/src/lib.rs", (
        "//! Core Pipeline state machine, Scratchpad memory arena, VramEntry L0 residency table.\n"
        "//! All hot-path structs: Copy + #[repr(C)] + 16 bytes, compile-time size asserts.\n"
        "\n"
        "#[repr(C)]\n"
        "pub struct VramEntry {\n"
        "    pub tag:      u64,  // 64-bit wymix non-cryptographic hash\n"
        "    pub image_id: u32,  // Non-zero 24-bit KGP image ID\n"
        "    pub cols:     u16,  // Cell-snapped column count\n"
        "    pub rows:     u16,  // Cell-snapped row count\n"
        "}\n"
        "const _: () = assert!(core::mem::size_of::<VramEntry>() == 16);\n"
    )),
    ("ii-0.1.0/src/scan.rs", (
        "//! SIMD memchr3 single-pass lexer: Markdown, LaTeX math ($...$  $$...$$),\n"
        "//! fenced code blocks (mermaid dot svg csv), ipynb outputs.\n"
        "//! Span: 16 bytes, 4 spans per 64-byte cacheline. Zero heap alloc in hot path.\n"
        "\n"
        "#[repr(C)]\n"
        "pub struct Span {\n"
        "    pub outer_start:        u32,\n"
        "    pub outer_end:          u32,\n"
        "    pub payload_start:      u32,\n"
        "    pub payload_end_and_kind: u32, // 28-bit offset | 4-bit SpanKind\n"
        "}\n"
        "const _: () = assert!(core::mem::size_of::<Span>() == 16);\n"
    )),
    ("ii-0.1.0/src/kgp.rs", (
        "//! Kitty Graphics Protocol: APC/DCS command formatter, RFC 4648 Base64 codec,\n"
        "//! U+10EEEE Unicode Placeholder grid encoder.\n"
        "//!\n"
        "//! FIRST_CELL_8_LUT : 1024-entry static table => 8-byte register copy per cell.\n"
        "//! PLACEHOLDER_RUN  : 1024-byte SIMD memcpy for implicit column auto-increment.\n"
        "//!\n"
        "//! Image ID invariant: non-zero 24-bit ID from BLAKE3 key:\n"
        "//!   (b[0] << 16 | b[1] << 8 | b[2]).max(1)\n"
    )),
    ("ii-0.1.0/src/cache.rs", (
        "//! Three-tier cache hierarchy:\n"
        "//!   L0: 64-bit wymix non-cryptographic VRAM tagger  (compute_vram_tag)\n"
        "//!   L1: thread-local 64-way ring buffer              (L1_CACHE)\n"
        "//!   L2: BLAKE3-keyed, LZ4-compressed disk CAS       (PX01 magic, .px1 files)\n"
        "//!       openat on cached dir fd + atomic renameat writes\n"
    )),
    ("ii-0.1.0/src/ipc.rs", (
        "//! Zero-fork AF_UNIX daemon (IpcServer, run_daemon).\n"
        "//! IXPC v1 Header: 32 bytes -- 2 headers per 64-byte cacheline.\n"
        "//!\n"
        "//! Commands:\n"
        "//!   PING                  = 0x0001\n"
        "//!   CMD_COMPOSE_ANSI      = 0x0002\n"
        "//!   CMD_COMPOSE_EXTMARKS  = 0x0003\n"
        "//!   CMD_RENDER_FILE       = 0x0004\n"
        "//!   SHUTDOWN              = 0x0006\n"
        "//!\n"
        "//! DevSecOps socket hardening: dir 0700, socket 0600, CWE-59 symlink reject.\n"
    )),
    ("ii-0.1.0/src/raster.rs", (
        "//! Single-pass zero-allocation SVG color rewriter (rewrite_svg_colors),\n"
        "//! cell-snapped resvg/tiny-skia vector-to-RGBA rasterizer (rasterize_span),\n"
        "//! PNG decoder for notebook outputs (decode_png_into).\n"
    )),
    ("ii-0.1.0/src/math.rs", (
        "//! ratex (ratex-parser / ratex-layout / ratex-svg) TeX-to-SVG compiler.\n"
        "//! compile_math_svg: deterministic SVG with error badge fallback.\n"
    )),
    ("ii-0.1.0/benches/bench.rs", (
        "//! Criterion suite: scan_throughput, pipeline_warm, cache, frontends_zero_alloc.\n"
        "//! benches/daemon.rs: fork/exec vs zero-fork UDS tail-latency comparison.\n"
    )),
    ("ii-0.1.0/tests/snapshot.rs", (
        "//! tests/snapshot.rs : deterministic render regression suite.\n"
        "//! tests/daemon.rs   : IXPC UDS daemon end-to-end (just e2e).\n"
    )),
    ("ii-0.1.0/LICENSE-MIT", (
        "MIT License\n"
        "Copyright (c) 2026 zx0r\n"
        "\n"
        "Permission is hereby granted, free of charge, to any person obtaining a copy\n"
        "of this software and associated documentation files (the \"Software\"), to deal\n"
        "in the Software without restriction, including without limitation the rights\n"
        "to use, copy, modify, merge, publish, distribute, sublicense, and/or sell\n"
        "copies of the Software, and to permit persons to whom the Software is\n"
        "furnished to do so, subject to the following conditions:\n"
        "\n"
        "The above copyright notice and this permission notice shall be included in all\n"
        "copies or substantial portions of the Software.\n"
    )),
]

MTIME = int(time.mktime((2026, 11, 12, 0, 0, 0, 0, 0, -1)))

# ── helpers ────────────────────────────────────────────────────────────

def make_tarinfo(name: str, data: bytes) -> tarfile.TarInfo:
    ti = tarfile.TarInfo(name=name)
    ti.size  = len(data)
    ti.mtime = MTIME
    ti.mode  = 0o644
    ti.type  = tarfile.REGTYPE
    return ti


def write_tar(path: str, mode: str) -> None:
    with tarfile.open(path, mode) as tf:
        for name, content in FILES:
            data = content.encode()
            tf.addfile(make_tarinfo(name, data), io.BytesIO(data))
    print(f"  {os.path.basename(path)}: {os.path.getsize(path):,} bytes, {len(FILES)} entries")


def write_zip(path: str, compression: int) -> None:
    with zipfile.ZipFile(path, "w", compression=compression, compresslevel=9) as zf:
        for name, content in FILES:
            zi = zipfile.ZipInfo(name)
            zi.date_time = (2026, 11, 12, 0, 0, 0)
            zi.compress_type = compression
            zf.writestr(zi, content)
    print(f"  {os.path.basename(path)}: {os.path.getsize(path):,} bytes, {len(FILES)} entries")


# ── generate ───────────────────────────────────────────────────────────
print("Generating showcase archives...")

write_zip(os.path.join(HERE, "20.zip"),  zipfile.ZIP_DEFLATED)
write_tar(os.path.join(HERE, "21.tar"),  "w:")
write_tar(os.path.join(HERE, "22.tgz"),  "w:gz")
write_tar(os.path.join(HERE, "23.tbz2"), "w:bz2")
write_tar(os.path.join(HERE, "24.txz"),  "w:xz")

# zstd: tarfile added native support in Python 3.14; use subprocess fallback for now
import subprocess, shutil
if shutil.which("zstd"):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:") as tf:
        for name, content in FILES:
            data = content.encode()
            tf.addfile(make_tarinfo(name, data), io.BytesIO(data))
    buf.seek(0)
    out_path = os.path.join(HERE, "25.tzst")
    result = subprocess.run(
        ["zstd", "-19", "-f", "-o", out_path],
        input=buf.read(), capture_output=True
    )
    if result.returncode == 0:
        print(f"  25.tzst: {os.path.getsize(out_path):,} bytes, {len(FILES)} entries")
    else:
        print(f"  25.tzst: zstd failed: {result.stderr.decode().strip()}")
else:
    print("  25.tzst: skipped (zstd not in PATH)")

# .jar = zip with manifest (Java archive)
jar_files = [("META-INF/MANIFEST.MF",
              "Manifest-Version: 1.0\nMain-Class: dev.ii.Main\nImplementation-Version: 0.1.0\n"),
             *FILES]
jar_path = os.path.join(HERE, "26.jar")
with zipfile.ZipFile(jar_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
    for name, content in jar_files:
        zi = zipfile.ZipInfo(name)
        zi.date_time = (2026, 11, 12, 0, 0, 0)
        zi.compress_type = zipfile.ZIP_DEFLATED
        zf.writestr(zi, content)
print(f"  26.jar: {os.path.getsize(jar_path):,} bytes, {len(jar_files)} entries")

# .whl = zip with dist-info (Python wheel)
whl_files = [
    ("ii-0.1.0.dist-info/WHEEL",
     "Wheel-Version: 1.0\nGenerator: ii-build 0.1.0\nRoot-Is-Purelib: false\nTag: py3-none-any\n"),
    ("ii-0.1.0.dist-info/METADATA",
     "Metadata-Version: 2.3\nName: ii\nVersion: 0.1.0\n"
     "Summary: Zero-overhead terminal render engine -- Kitty Graphics Protocol\n"
     "Author-email: zx0r <admin@xxxx.dev>\nLicense: MIT OR Apache-2.0\n"),
    ("ii-0.1.0.dist-info/RECORD", "ii-0.1.0.dist-info/WHEEL,,\nii-0.1.0.dist-info/METADATA,,\n"),
]
whl_path = os.path.join(HERE, "27.whl")
with zipfile.ZipFile(whl_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
    for name, content in whl_files:
        zi = zipfile.ZipInfo(name)
        zi.date_time = (2026, 11, 12, 0, 0, 0)
        zi.compress_type = zipfile.ZIP_DEFLATED
        zf.writestr(zi, content)
print(f"  27.whl: {os.path.getsize(whl_path):,} bytes, {len(whl_files)} entries")

print("\nDone. Summary:")
for f in sorted(os.listdir(HERE)):
    p = os.path.join(HERE, f)
    if os.path.isfile(p):
        ext = f.rsplit(".", 1)[-1] if "." in f else "—"
        print(f"  {f:<25} {os.path.getsize(p):>8,} bytes  .{ext}")
