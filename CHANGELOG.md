# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Commit types follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

## [Unreleased]

### Features

- **render:** Add HTML format adapter and separate heading palette ([350387b])
- **render:** Implement cell-snapped resvg rasterizer, Q24 unpremultiply LUT, and KGP SHM/Base64 encoder ([48a92a3])
- **core:** Restore native prose styler, stream healer, and Neovim presentation ([4ec75ad])
- **nvim:** Align plugin with 297 KGP diacritics and 4-byte UTF-8 encoding ([ea97f16])
- **arch:** Enforce stateless state relay, 297 KGP diacritics, and byte-bounded RSS ([b6a764c])
- **arch:** Implement pure stateless state relay with scaling and zero-overhead client plugins ([4cb6f0d])
- **layout:** Implement `StyleMetrics` with quantized grid layout and `BASE_STROKE` ([dccd79f])
- **orchestration:** `FORMAT_TABLE` O(1) dispatch, L0/L1/L2 compose, IXPC v1 codec, non-blocking poll daemon and CLI ([d0e080d])
- **io:** TTY OSC 10/11 probe, Rec. 709 luminance and PX02 BLAKE3/LZ4 L2 disk cache ([48a92a3])
- **vram:** 4-way Tree-PLRU L0 tag table, 16×4 L1 RGBA cache and wymix identity ([97c019b])
- **frontend:** Vega-Lite/Plotly chart, Sugiyama DOT, CSV/pipe table, ipynb, font, SVG rewriter and ANSI styler ([0b1f810])
- **parse:** Single-pass O(N) scanner, zero-alloc JSON nav, LaTeX healer and mhchem transpiler ([33a3745])
- **ir:** 16 B Span, 4-bit Kind LUT and frame-local Scratch arena ([e6030e7])
- **core:** `const` Token/Palette LUTs (Mocha/Latte), `Cfg.palette` ([d89358a])
- **core:** Flat `Error` enum, `Cfg`/`Viewport`, `tty::Metrics` with layout locks ([846ca88])

### Bug Fixes

- **render:** Close audit regressions ([caa4dc1])
- **yazi:** Reuse diagram uploads while scrolling ([fe369c7])
- **nvim:** Route HTML previews ([33c0697])
- **render:** Restore previews across Neovim and Yazi ([cbad71b])
- **engine:** Restore transparent previews and document architecture ([5e9abd1])
- **raster:** Overhaul styling pipeline, theme single source of truth, and in-memory vector icons ([e04d95e])
- **plugins:** Align Yazi Command chaining, Neovim loader, and daemon socket binding ([452475f])

### Performance

- **yazi:** Smooth notebook preview scrolling ([2739139])

### Refactoring

- **showcase:** Restructure stands and optimize preview scrolling ([6c7fd39])
- **theme:** Introduce `SemanticRole` tokens and straight sRGB palette ([9534a04])
- **arch:** Hourglass Scene IR, dedicated layout engine, and zero-alloc theme ([d56f8a3])
- Apply `TextDraw` extraction, fix local TTY KGP delivery, and configure benchmarks ([9786cbd])

### Build System

- Complete Sans-I/O pipeline, flat test topology, and Criterion benchmarks ([6934e8f])

### Miscellaneous

- Systems architecture specification ([01cdf7e])
- Baseline toolchain, lint, supply-chain and architecture spec ([626b174])

[Unreleased]: https://github.com/zx0r/ii/compare/v0.1.0...HEAD

<!-- Commit short-links (GitHub auto-resolves) -->
[6c7fd39]: https://github.com/zx0r/ii/commit/6c7fd39
[caa4dc1]: https://github.com/zx0r/ii/commit/caa4dc1
[fe369c7]: https://github.com/zx0r/ii/commit/fe369c7
[33c0697]: https://github.com/zx0r/ii/commit/33c0697
[350387b]: https://github.com/zx0r/ii/commit/350387b
[2739139]: https://github.com/zx0r/ii/commit/2739139
[cbad71b]: https://github.com/zx0r/ii/commit/cbad71b
[5e9abd1]: https://github.com/zx0r/ii/commit/5e9abd1
[4ec75ad]: https://github.com/zx0r/ii/commit/4ec75ad
[ea97f16]: https://github.com/zx0r/ii/commit/ea97f16
[b6a764c]: https://github.com/zx0r/ii/commit/b6a764c
[e04d95e]: https://github.com/zx0r/ii/commit/e04d95e
[dccd79f]: https://github.com/zx0r/ii/commit/dccd79f
[9534a04]: https://github.com/zx0r/ii/commit/9534a04
[452475f]: https://github.com/zx0r/ii/commit/452475f
[4cb6f0d]: https://github.com/zx0r/ii/commit/4cb6f0d
[9786cbd]: https://github.com/zx0r/ii/commit/9786cbd
[6934e8f]: https://github.com/zx0r/ii/commit/6934e8f
[01cdf7e]: https://github.com/zx0r/ii/commit/01cdf7e
[d56f8a3]: https://github.com/zx0r/ii/commit/d56f8a3
[d0e080d]: https://github.com/zx0r/ii/commit/d0e080d
[48a92a3]: https://github.com/zx0r/ii/commit/48a92a3
[97c019b]: https://github.com/zx0r/ii/commit/97c019b
[0b1f810]: https://github.com/zx0r/ii/commit/0b1f810
[33a3745]: https://github.com/zx0r/ii/commit/33a3745
[e6030e7]: https://github.com/zx0r/ii/commit/e6030e7
[d89358a]: https://github.com/zx0r/ii/commit/d89358a
[846ca88]: https://github.com/zx0r/ii/commit/846ca88
[626b174]: https://github.com/zx0r/ii/commit/626b174
