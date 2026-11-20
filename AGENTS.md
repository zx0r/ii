# `ii`

Zero-overhead terminal graphical coprocessor written in Rust 2024. Compiles documents, formulas,
diagrams, charts, tables, and archives into cell-snapped RGBA8 frames and delivers them over the
**Kitty Graphics Protocol (KGP)** using `U+10EEEE` virtual placeholders (CLI, Yazi, Neovim).

---

## Processing Pipeline

```text
Input (&[u8] / file / stdin / IXPC v1)
  │
  ▼  fmt::detect_kind -> fmt::dispatch_kind (closed enum, zero horizontal coupling)
Hourglass Scene IR (flat SoA: Rect / Path / GlyphRun / TableGrid / text_pool)
  │
  ├── L0 VRAM Table (4 160 B, 64×4 Tree-PLRU) ──► hit: emit_placeholder only
  ├── L1 RGBA Cache (16×4 Tree-PLRU)          ──► hit: stage_shm / stage_base64
  └── L2 Disk Cache (BLAKE3 + PX03/LZ4)       ──► hit: decompress_into -> KGP
  │
  ▼  miss: raster::rasterize_scene / raster::rasterize (resvg + tiny-skia + Q24 unpremultiply)
KGP Transport
  ├── local: POSIX SHM (t=s, mmap + RAII unlink)
  └── SSH/fallback: chunked Base64 (t=d, 4 096 B) + tmux DCS passthrough
```

---

## Supported Formats

Routing resolves by file extension (ASCII case-insensitive) with a zero-copy magic-byte sniffing
fallback (`fmt::detect_kind`).

| Category         | Extensions                                                       | Engine / Strategy                                                                                                  |
| :--------------- | :--------------------------------------------------------------- | :----------------------------------------------------------------------------------------------------------------- |
| Markup           | `.md`, `.qmd`, `.mdx`, `.rmd`, `.typ`                            | Single-pass SIMD scanner (`memchr`): frontmatter, fences, `$$`/`$`, pipe tables -> routed via `fmt::dispatch_kind` |
| Notebooks        | `.ipynb`                                                         | Zero-copy JSON walker (`MAX_DEPTH = 64`): markdown, code, stream/error, MIME (`svg`, `latex`, `markdown`, `plain`) |
| Math / Chemistry | `.tex`, `.latex`                                                 | `ratex-parser` / `ratex-layout` / `ratex-svg` + inline `mhchem` (`\ce{...}`) with a zero-copy fast-path            |
| Diagrams         | `.mmd`, `.mermaid`                                               | `mermaid-svg` -> palette adaptation -> `Scene`                                                                     |
| Graphs           | `.dot`, `.gv`                                                    | Stack tokenizer with `u32` offsets (`[Node; 128] <= 4 096 B`), cycle-safe layered DAG layout -> `Scene` / SVG      |
| Vector           | `.svg`                                                           | Zero-DOM color rewriting (`currentColor`, `#000`, `black`) + `viewBox` / `width` / `height` extraction             |
| Tables           | `.csv`, `.tsv`, pipe tables                                      | Zero-copy parser with quote unescaping -> `TableGrid` (`<= 64` columns, `<= 256` rows)                             |
| Charts           | `.vl`, `.plotly`                                                 | Zero-copy series extraction (bar / line / scatter, `<= 512` points) + `LinearScale` -> `Rect` / `Path`             |
| Archives         | `.zip`, `.jar`, `.whl`, `.tar`, `.tgz`, `.tbz2`, `.txz`, `.tzst` | Zero-dependency inspector for ZIP EOCD/CD, POSIX ustar TAR, and `gzip`/`bzip2`/`xz`/`zstd` headers -> `TableGrid`  |

---

## Architectural Invariants

### Data & Memory Layout

- **Compile-time ABI assertions:** core types enforce layout via `#[repr(C)]` and
  `const _: () = assert!(...)`:
  - `Error`: `<= 12 B`, `Copy`, zero heap payloads.
  - `GlyphRun` (20 B), `Rect` (20 B), `Path` (20 B), `CellSpan` (12 B), `TableGrid` (24 B), `Span`
    (20 B).
  - `Palette`: 192 B, 64-byte aligned (3 cache lines); `SgrSeq` tables are generated at compile time
    (`const fn`).
- **Steady-state RSS ceiling:** `RenderScratch::reset()` clears buffers without deallocating, while
  shrinking outlier spikes above strict high-water marks (`MAX_RETAINED_RGBA = 16 MiB`,
  `MAX_RETAINED_OUT = 4 MiB`, `Cache::MAX_RETAINED_SLOT_CAP = 2 MiB`).
- **Render safety:** hard raster bound `MAX_DIM = 4096 px`, integer snapping to terminal cell grids
  (`cell_w × cell_h`), graceful degradation on malformed UTF-8, SVG, or LaTeX payloads.

### Three-Tier Caching

- **L0 (`vram::Table`):** 4-way set-associative, 64 sets, Tree-PLRU, 4 160 B (`align(64)`). Keyed by
  `Tag(NonZeroU64)` (`wymix`); `image_id` is a 24-bit `NonZeroU32`.
- **L1 (`vram::Cache`):** 4-way × 16 sets, Tree-PLRU, retaining ready RGBA8 replicas in process
  memory.
- **L2 (`cache`):** BLAKE3-keyed across cell geometry, `max_w`, block kind, `fg_rgb`, theme
  polarity, and payload. Blob format `PX03` (12-byte LE header + raw LZ4 block,
  `MAX_DECODE_SIZE = 64 MiB`). Published atomically via `openat(O_CREAT | O_EXCL | O_NOFOLLOW)` +
  `renameat(2)`.

### IXPC v1 Protocol & Daemon

- **Reactor (`daemon.rs`):** single-threaded non-blocking `poll(2)` Unix-socket reactor
  (`MAX_CLIENTS = 32`, `RECV_BUF_BYTES = 512 KiB`, 30 s idle timeout, 1 s graceful drain, zero
  `unsafe`).
- **Wire frame (`ipc.rs`):** 32-byte LE header (`MAGIC = 0x4958_5043`, `VERSION = 1`,
  `MAX_PAYLOAD = 64 MiB`). Clients dispatch header and payload in a single `writev(2)` call.
- **Commands:** `Ping` (`0x0001`), `ComposeAnsi` (`0x0002`), `ComposeExtmarks` (`0x0003`, 16-byte
  `Extmark` records), `RenderFile` (`0x0004`), `Shutdown` (`0x0006`).
- **OS boundary & security:**
  - Runtime directory opened with `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`, verified against
    `st_uid == getuid()`, and restricted via `fchmod(0700)`.
  - Single-instance exclusivity enforced via `flock(LOCK_EX | LOCK_NB)` on `ii.lock`; socket cleanup
    guaranteed by RAII `DaemonGuard`.
  - `/dev/tty` probing (`TIOCGWINSZ` + OSC 11 Rec. 709 luminance) restores `Termios` unconditionally
    via `RawModeGuard`.
  - The sole `unsafe` block in the codebase is isolated inside `kitty::ShmMap` (`mmap` /
    `copy_nonoverlapping` / `munmap`) under RAII `ShmGuard`.

---

## Build & Usage

```bash
cargo build --release
```

```text
ii [OPTIONS] [FILE]

Options:
  --daemon       Run background IXPC v1 coprocessor daemon
  --ping         Ping running daemon
  --shutdown     Gracefully shut down running daemon
  -h, --help     Show help message
  -V, --version  Show version
```

Examples:

```bash
ii document.md                  # local render to KGP + U+10EEEE placeholders
cat diagram.dot | ii            # read from stdin with automatic format detection
ii --daemon                     # start daemon ($XDG_RUNTIME_DIR/ii/ii.sock)
ii --ping && ii --shutdown      # health check and graceful shutdown
```

---

## Verification

Tests are partitioned into 5 orthogonal integration targets (zero `unwrap`/`expect`/`panic!`, zero
shared global state):

| Target            | Scope                                                                                                               | Gate                    |
| :---------------- | :------------------------------------------------------------------------------------------------------------------ | :---------------------- |
| `tests/core.rs`   | `scene`, `vram`, `layout`, `theme`, `err`, `kitty` (Base64/placeholder), `tty`, `ipc`, `unpremultiply` + `proptest` | `cargo test` + **Miri** |
| `tests/fmt.rs`    | `fmt` router, `font`, `json`, `csv`/`tsv`, `archive`, `svg`, `dot`, `chart`, `ipynb`, `md` + `proptest`             | `cargo test` + **Miri** |
| `tests/render.rs` | `ratex-*` (`tex`/`\ce{}`), `mermaid-svg`, end-to-end sweep of the `showcase/` corpus                                | `cargo test`            |
| `tests/io.rs`     | L2 `PX03` (`openat`/`renameat`/`O_NOFOLLOW`), live POSIX SHM, `resvg`, `decode_png`                                 | `cargo test` + **ASan** |
| `tests/daemon.rs` | Live `--daemon` / `--ping` / `--shutdown`, `flock`, raw IXPC v1 frames, malformed header handling                   | `cargo test`            |

Full verification suite:

```bash
# 1. Functional, integration, and property tests
cargo test --all-features

# 2. Strict static analysis across all targets
cargo clippy --all-targets --all-features -- -D warnings

# 3. Miri (Tree Borrows + Strict Provenance) on the Sans-I/O layer
cargo +nightly miri test --test core --test fmt -- \
  -Zmiri-tree-borrows -Zmiri-strict-provenance -Zmiri-symbolic-alignment-check

# 4. AddressSanitizer across POSIX SHM / FFI / resvg
RUSTFLAGS="-Zsanitizer=address" cargo +nightly test --test io --target aarch64-apple-darwin
```

---

## Repository Layout

```text
src/
├── main.rs          # CLI, local KGP pipeline, daemon control
├── lib.rs           # Crate module exports
├── err.rs           # Flat Copy error type (<= 12 B)
├── theme.rs         # DARK/LIGHT palettes (align 64), ColorToken, const SGR
├── scene.rs         # Hourglass Scene IR (SoA) and RenderScratch
├── layout.rs        # LinearScale and table column width solver
├── vram.rs          # L0 VRAM Table, L1 RGBA Cache, wymix Tag
├── cache.rs         # L2 BLAKE3 + PX03/LZ4 disk cache (openat/O_NOFOLLOW)
├── raster.rs        # resvg + tiny-skia rasterizer, PNG decoder, Q24 unpremultiply
├── kitty.rs         # KGP transport (POSIX SHM / Base64 / tmux DCS / U+10EEEE)
├── tty.rs           # TIOCGWINSZ, OSC 11 luminance probe, render_ansi
├── ipc.rs           # IXPC v1 wire codec, Extmark, writev client
├── daemon.rs        # Non-blocking poll(2) reactor (MAX_CLIENTS = 32)
└── fmt/
    ├── mod.rs       # Closed-enum Kind router + detect_kind + dispatch_kind
    ├── md.rs        # SIMD scanner for .md/.qmd/.mdx/.rmd/.typ
    ├── ipynb.rs     # Jupyter Notebook walker
    ├── tex.rs       # ratex-* adapter + inline mhchem (\ce{})
    ├── mermaid.rs   # mermaid-svg adapter
    ├── dot.rs       # DOT/GV tokenizer and layered DAG layout
    ├── svg.rs       # SVG palette rewriter and dimension extractor
    ├── csv.rs       # CSV / TSV / Markdown pipe-table parser
    ├── chart.rs     # Vega-Lite / Plotly JSON chart compiler
    ├── archive.rs   # ZIP/JAR/WHL/TAR and compressed container inspector
    ├── json.rs      # Zero-copy JSON navigator
    └── font.rs      # ASCII glyph advance-width LUT
```
