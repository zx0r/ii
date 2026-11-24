<p align="center">
  <strong><code>ii</code></strong><br>
  <sub>Terminal graphical coprocessor — math, diagrams, charts, and archives over the Kitty Graphics Protocol.</sub>
</p>

<p align="center">
  <a href="https://github.com/zx0r/ii/actions/workflows/ci.yml"><img src="https://github.com/zx0r/ii/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://codecov.io/gh/zx0r/ii"><img src="https://codecov.io/gh/zx0r/ii/branch/main/graph/badge.svg" alt="Coverage"></a>
  <a href="https://crates.io/crates/ii"><img src="https://img.shields.io/crates/v/ii.svg" alt="Crates.io"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg" alt="License"></a>
</p>

---

**ii** renders Markdown, LaTeX, Mermaid, Graphviz, Vega‑Lite/Plotly charts, Jupyter notebooks, SVG, CSV, HTML, Typst, MDX, and compressed archives as pixel‑perfect Kitty graphics directly inside any KGP‑capable terminal — Kitty, WezTerm, Ghostty — with native tmux passthrough.

It ships as a single statically‑linked binary with **zero runtime dependencies**, a background IXPC daemon for sub‑millisecond repeat renders, and first‑class plugins for **Yazi** (file‑manager previews) and **Neovim** (in‑buffer decoration and floating previews).

## Highlights

- **16 format adapters** — single‑pass `O(N)` scanner, closed‑enum `Kind` dispatch, zero‑copy where possible.
- **Hourglass Scene IR** — flat SoA primitives (`GlyphRun` 20 B, `Rect` 20 B, `Path` 20 B, `CellSpan` 12 B, `SvgEmbed` 24 B`) with interned text pool.
- **4‑way set‑associative VRAM cache** — Tree‑PLRU eviction, `Tag` identity via `wymix`, 32 MiB soft budget.
- **L2 disk cache** — `PX03` envelope, BLAKE3 keying, raw LZ4 block, 256 MiB prune with race‑free `openat`/`renameat`.
- **IXPC v1 daemon** — non‑blocking `poll()` reactor, 32 clients, bounded render worker, viewport paging.
- **KGP dual transport** — POSIX SHM fast‑path (`t=s`, explicit `S=<bytes>` for macOS page rounding), Base64 fallback, 297 combining‑class‑230 diacritics for `U+10EEEE` placeholder grid.
- **Zero‑allocation hot paths** — no `String`/`Vec` realloc in inner loops; `const fn` palette LUTs, branchless SGR/hex formatting.
- **OSC 10/11 theme probing** — Rec. 709 luminance auto‑detection, Catppuccin Mocha (dark) / Latte (light).

## Quick Start

### Install from Source (Recommended)

```sh
# Requires Rust ≥ 1.100 (stable)
cargo install --locked --git https://github.com/zx0r/ii.git
```

### Homebrew (macOS / Linux)

```sh
brew install zx0r/tap/ii
```

### Shell Installer

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/zx0r/ii/releases/latest/download/ii-installer.sh | sh
```

### Prebuilt Binaries

Download platform archives from [GitHub Releases](https://github.com/zx0r/ii/releases):

| Target | Archive |
| --- | --- |
| `aarch64-apple-darwin` | `ii-aarch64-apple-darwin.tar.gz` |
| `x86_64-apple-darwin` | `ii-x86_64-apple-darwin.tar.gz` |
| `x86_64-unknown-linux-gnu` | `ii-x86_64-unknown-linux-gnu.tar.gz` |
| `x86_64-unknown-linux-musl` | `ii-x86_64-unknown-linux-musl.tar.gz` |
| `aarch64-unknown-linux-gnu` | `ii-aarch64-unknown-linux-gnu.tar.gz` |
| `aarch64-unknown-linux-musl` | `ii-aarch64-unknown-linux-musl.tar.gz` |

## Usage

```
ii [OPTIONS] [FILE]
ii doctor
```

### Direct Rendering

```sh
# Render Markdown with graphical blocks
ii README.md

# Render LaTeX equation
echo '\int_0^\infty e^{-x^2} dx = \frac{\sqrt{\pi}}{2}' | ii --size 80x24

# Render Mermaid diagram
ii diagram.mmd

# Render Jupyter notebook
ii analysis.ipynb

# Render from stdin (auto-detects format)
cat data.csv | ii

# Styled ANSI output on piped stdout
ii --ansi document.md | less -R
```

### Daemon Mode

The daemon caches rendered output and provides sub‑millisecond response for preview clients:

```sh
# Start background daemon (auto-starts on first preview request)
ii --daemon

# Diagnostic status
ii doctor

# Ping running daemon
ii --ping

# Shut down daemon
ii --shutdown
```

### Adapter for Yazi / Neovim

```sh
# Machine-framed output for adapter plugins
ii --machine --size 80x24 --cell-size 8x16 document.md
```

## Supported Formats

| Format | Extensions | Engine |
| --- | --- | --- |
| Markdown | `.md`, `.markdown`, `.qmd`, `.rmd` | `pulldown-cmark` → Scene IR |
| MDX | `.mdx` | JSX strip → Markdown pipeline |
| HTML | `.html`, `.htm` | Tag strip → Markdown pipeline |
| Typst | `.typ`, `.typst` | Normalize → Markdown pipeline |
| Jupyter | `.ipynb` | Cell extraction, embedded LaTeX/Mermaid |
| LaTeX | `.tex`, `.latex` | `ratex` (native Rust TeX engine) |
| Chemistry | `.chem`, `.mhchem` | `mhchem` → LaTeX transpiler |
| Mermaid | `.mmd`, `.mermaid` | `mermaid-svg` → resvg raster |
| Graphviz | `.dot`, `.gv` | Sugiyama layout → Scene IR |
| SVG | `.svg` | `resvg` raster, cell‑snapped |
| PNG | `.png` | Direct KGP passthrough |
| CSV / TSV | `.csv`, `.tsv` | Table grid with column auto‑width |
| Vega‑Lite | `.vl`, `.vega`, `.vega-lite` | JSON → SVG chart pipeline |
| Plotly | `.plotly` | JSON → SVG chart pipeline |
| Archives | `.zip`, `.tar`, `.tgz`, `.tbz2`, `.txz`, `.tzst`, `.jar`, `.whl` | Streamed TOC listing |

## Architecture

```
 ┌──────────────────────────────────────────────────────────────────────────┐
 │  CLI / stdin                 Yazi plugin              Neovim plugin     │
 │   ┌──────┐                 ┌──────────┐             ┌───────────┐      │
 │   │ main │                 │ ii.yazi  │             │  ii.nvim  │      │
 │   └──┬───┘                 └────┬─────┘             └─────┬─────┘      │
 │      │                         │                          │            │
 │      │    ┌────────────────────┴──────────────────────────┘            │
 │      ▼    ▼                                                            │
 │  ┌──────────┐   IXPC v1    ┌────────────┐                             │
 │  │  compose  │◄────────────►│   daemon   │  poll() reactor             │
 │  └────┬─────┘    UDS       └──────┬─────┘  32 clients                 │
 │       │                           │                                    │
 │       ▼                           ▼                                    │
 │  ┌────────┐  Kind dispatch  ┌───────────┐                              │
 │  │  fmt   │────────────────►│  Scene IR │  SoA: glyphs, rects, paths  │
 │  │ 16 fmt │  zero-copy      │  (flat)   │  text_pool, cells, svgs     │
 │  └────────┘                 └─────┬─────┘                              │
 │                                   │                                    │
 │       ┌──────────────┬────────────┼─────────────────┐                  │
 │       ▼              ▼            ▼                  ▼                  │
 │  ┌────────┐   ┌──────────┐  ┌─────────┐    ┌────────────┐             │
 │  │ layout │   │  raster  │  │  style  │    │  viewport  │             │
 │  │ engine │   │  resvg   │  │  ANSI   │    │  paging    │             │
 │  └────┬───┘   │  tiny-skia│  └────┬────┘    └──────┬─────┘             │
 │       │       └────┬─────┘       │                 │                   │
 │       ▼            ▼             ▼                 ▼                   │
 │  ┌──────────────────────────────────────────────────────────┐          │
 │  │                   kitty (KGP encoder)                    │          │
 │  │   SHM fast-path  ·  Base64 chunks  ·  U+10EEEE grid    │          │
 │  └────────────────────────┬─────────────────────────────────┘          │
 │                           │                                            │
 │       ┌───────────────────┼───────────────────┐                        │
 │       ▼                   ▼                   ▼                        │
 │  ┌─────────┐       ┌───────────┐       ┌───────────┐                  │
 │  │  vram   │       │  cache L2 │       │   ipc     │                  │
 │  │ 4-way   │       │  PX03/LZ4 │       │  IXPC v1  │                  │
 │  │ PLRU    │       │  BLAKE3   │       │  codec    │                  │
 │  └─────────┘       └───────────┘       └───────────┘                  │
 └──────────────────────────────────────────────────────────────────────────┘
```

### Module Map

| Module | Lines | Responsibility |
| --- | --- | --- |
| `main` | 464 | CLI parser, local pipeline, daemon control |
| `daemon` | 1172 | Non-blocking reactor, render worker, preview paging |
| `compose` | 644 | Ordered prose + graphic presentation, source anchors |
| `style` | 1179 | ANSI prose styler, semantic SGR emitter |
| `raster` | 894 | Cell-snapped resvg rasterizer, Q24 unpremultiply LUT |
| `kitty` | 806 | KGP SHM/Base64 encoder, 297 diacritics, placeholder grid |
| `scene` | 400 | Hourglass Scene IR, SoA primitives, text pool |
| `fmt/*` | 4306 | 16 format adapters (md, tex, ipynb, dot, chart, …) |
| `ipc` | 413 | IXPC v1 wire codec, header/command validation |
| `cache` | 369 | BLAKE3 L2 disk cache, PX03 + LZ4, atomic publish |
| `vram` | 179 | 4-way Tree-PLRU RGBA L0 cache |
| `theme` | 407 | Static palette tables, 1-byte `ColorToken`, SGR/hex LUTs |
| `tty` | 197 | `TIOCGWINSZ`, OSC 11 probe, Rec. 709 luminance |
| `viewport` | 325 | Row-index paging, skip/limit, bounded append |
| `layout` | 192 | Quantized grid layout, `StyleMetrics`, `BASE_STROKE` |
| `image` | 259 | Image context, dimension calculation |
| `err` | 75 | Flat ≤ 12 B error enum, zero heap payloads |
| `file` | 45 | Bounded file read |
| **Total** | **~12 700** | |

## Plugins

### Yazi — File Manager Preview

See [`plugins/ii.yazi/`](plugins/ii.yazi/) for the Lua plugin that integrates `ii` as a Yazi previewer.

### Neovim — Buffer Decoration & Floating Preview

See [`plugins/ii.nvim/`](plugins/ii.nvim/) for the Neovim plugin providing in-buffer graphical decoration and floating preview windows via extmarks.

### Additional Integrations

| Integration | Location | Description |
| --- | --- | --- |
| Kitty config snippet | [`plugins/kitty/`](plugins/kitty/) | Terminal configuration |
| fzf preview binding | [`plugins/fzf/`](plugins/fzf/) | `fzf --preview` integration |
| Yazi TOML config | [`plugins/yazi/`](plugins/yazi/) | Yazi previewer registration |

## Development

### Prerequisites

```sh
# Install complete developer toolchain
just setup-tools
```

This installs: `bacon`, `sccache`, `cargo-deny`, `cargo-llvm-cov`, `cargo-nextest`, `cargo-dist`, `cargo-mutants`, `cargo-binstall`, `lefthook`, `cocogitto`, `actionlint`, `zizmor`, `gitleaks`, `cargo-machete`, `cargo-semver-checks`, and the nightly Miri toolchain.

### Build & Test

```sh
# Full quality gate (fmt-check + pedantic/nursery clippy)
just qa

# Run all tests via nextest
just test

# Fast developer pass (fail-fast, no doctests)
just test-fast

# IXPC daemon end-to-end tests
just e2e
```

### Benchmarks

```sh
# Render pipeline micro-benchmarks (native vectorization)
just bench

# Daemon latency benchmark (fork/exec vs zero-fork UDS)
just bench-daemon

# Quick regression smoke pass (sample-size 10)
just bench-quick

# Save/compare Criterion baselines
just bench-save baseline-name
just bench-compare baseline-name
```

### Security & Supply Chain

```sh
# RustSec advisory + license + ban audit
just audit

# Secret leak scan
just audit-secrets

# Unused dependency scan
just machete

# Validate GitHub Actions workflows
just lint-workflows
```

### Advanced Verification

```sh
# Miri memory safety (Tree Borrows + strict provenance)
just miri

# SemVer API compatibility check
just semver

# Mutation testing
just mutants

# LLVM source-based coverage
just cov

# Full local CI gate
just check-all
```

### Release

```sh
# Build size-optimized binary (opt-level = "z")
just release-small

# Build full-throughput production binary (opt-level = 3, LTO fat)
just release

# Host-tuned build (NEON / AVX2)
just release-native

# Dry-run cargo-dist packaging
just dist-plan
```

Releases are published via [`cargo-dist`](https://github.com/axodotdev/cargo-dist):
- **GitHub Releases** — platform archives with SHA-256 digests, SLSA v1.1 Build Level 3 attestations, CycloneDX SBOM
- **Homebrew** — automated formula to `zx0r/homebrew-tap`
- **crates.io** — OIDC trusted publishing via GitHub Actions
- **Self-updater** — companion `ii-update` binary bundled in every archive

## CI Pipeline

The CI workflow runs on every push and PR:

| Gate | Checks |
| --- | --- |
| **Lint Workflows** | `actionlint`, `zizmor` offline audit |
| **Fast Checks** | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo deny check`, `gitleaks` |
| **Security Audit** | `cargo deny --all-features check` (advisories, bans, licenses, sources) |
| **Test & Coverage** | `cargo nextest` + doctests, LLVM coverage upload to Codecov |
| **Miri Verification** | Tree Borrows + strict provenance on `--workspace --lib` |
| **Terminal Previews** | Real Yazi + Neovim PTY integration tests on macOS arm64 |
| **ci-gate** | Branch protection aggregator — single required status check |

Dependency automation: [Renovate](https://github.com/renovatebot/renovate) with 72‑hour quarantine, SHA‑pinned Actions, and zero‑day security bypass.

## Project Structure

```
ii/
├── src/                    # ~12 700 lines of Rust
│   ├── main.rs             # CLI, daemon control, local pipeline
│   ├── lib.rs              # Public module re-exports
│   ├── fmt/                # 16 format adapters (md, tex, dot, chart, …)
│   ├── scene.rs            # Hourglass Scene IR (SoA primitives)
│   ├── compose.rs          # Prose + graphic composition
│   ├── style.rs            # ANSI prose styler
│   ├── raster.rs           # Cell-snapped resvg rasterizer
│   ├── kitty.rs            # KGP SHM/Base64 encoder
│   ├── daemon.rs           # IXPC reactor & render worker
│   ├── ipc.rs              # Wire codec
│   ├── cache.rs            # BLAKE3 L2 disk cache
│   ├── vram.rs             # 4-way Tree-PLRU RGBA cache
│   ├── theme.rs            # Palette tables & ColorToken
│   ├── tty.rs              # Terminal geometry & theme probing
│   ├── layout.rs           # Grid layout engine
│   ├── viewport.rs         # Row-index paging
│   ├── image.rs            # Image dimension context
│   ├── err.rs              # ≤12 B flat error enum
│   └── file.rs             # Bounded file read
├── tests/                  # Integration tests (core, daemon, fmt, io, render)
├── benches/                # Criterion + Divan benchmarks
├── plugins/
│   ├── ii.yazi/            # Yazi file-manager preview plugin (Lua)
│   ├── ii.nvim/            # Neovim buffer decoration plugin (Lua)
│   ├── kitty/              # Terminal config snippet
│   ├── fzf/                # fzf --preview binding
│   └── yazi/               # Yazi TOML registration
├── showcase/               # 32 sample documents + verification scripts
├── specs/                  # Architecture specs & research
├── .github/
│   ├── workflows/ci.yml    # 6-gate CI pipeline
│   ├── workflows/release.yml # cargo-dist release + SLSA attestation
│   ├── ISSUE_TEMPLATE/     # Bug report, feature request, question
│   └── renovate.json5      # Dependency automation
├── Justfile                # 30+ development recipes
├── Cargo.toml              # Edition 2024, resolver 3, Rust ≥ 1.100
├── deny.toml               # License + advisory + ban policy
├── cog.toml                # Conventional Commits config
├── dist-workspace.toml     # cargo-dist release packaging
├── lefthook.yml            # Git hook orchestrator
└── CONTRIBUTING.md         # Contribution guidelines
```

## Requirements

- **Rust** ≥ 1.100 (Edition 2024)
- **Terminal** with Kitty Graphics Protocol support (Kitty, WezTerm, Ghostty)
- **tmux** ≥ 3.3 for passthrough mode (optional)

## License

Licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development workflow, verification requirements, and commit conventions.
