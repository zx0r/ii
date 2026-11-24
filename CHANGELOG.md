# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-11-24

### Added

- **Core Engine**: Ultra-fast terminal graphical coprocessor written in Rust 2024 for math, diagrams, charts, and notebooks over Kitty Graphics Protocol (KGP).
- **Format Synthesizers**: Single-pass zero-alloc O(1) format routing supporting Markdown, Quarto (`.qmd`), RMarkdown (`.rmd`), Jupyter Notebooks (`.ipynb`), LaTeX/ratex, Typst, Mermaid (`.mmd`), Graphviz (`.dot`), SVG/SVGZ, Vega-Lite (`.vl`), Plotly JSON, CSV/TSV tables, and archive inspection (`.zip`, `.tar`, `.tgz`, `.tbz2`, `.txz`, `.tzst`, `.jar`, `.whl`).
- **Chemistry Compilation**: Built-in chemical formula parser with `mhchem` (`\ce{...}`) macro support and reaction kinematics.
- **Graphic Coprocessor & VRAM Cache**: Cell-snapped resvg rasterizer with 4-way Tree-PLRU L0 tag table, 16×4 L1 RGBA tile cache, Q24 unpremultiply LUT, and KGP SHM/Base64 stream encoder.
- **IXPC v1 IPC Protocol**: High-throughput non-blocking Unix domain socket protocol (`/tmp/ii-$UID/ii.sock`) for seamless terminal and editor integrations.
- **Client Integrations**: Official adapters for Neovim (`ii.nvim` with ordered frame delivery and Conceal mode) and Yazi (`ii.yazi` with background preloader and thread-safe graphics sync).
- **Theme & Cyberpunk Palette**: Static zero-alloc 32-color palette system with straight sRGB, compile-time ANSI SGR sequences, and transparent SVG surface rendering.

## [0.0.0] - 2026-11-19

### Added

- **Project Genesis**: Initial baseline toolchain, lint configuration, and supply-chain hardening.
- **Sans-I/O Architecture**: Core foundation for flat `Error` enum, `Cfg`/`Viewport` primitives, and terminal cell geometry metrics (`TIOCGWINSZ`).
- **Scratch Arena & IR**: 16 B `Span`, 4-bit `Kind` LUT, and frame-local arena memory management.

[0.1.0]: https://github.com/zx0r/ii/releases/tag/v0.1.0
[0.0.0]: https://github.com/zx0r/ii/releases/tag/v0.0.0
