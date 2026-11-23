# Rendering and scrolling stands

`scenarios/` contains 30 deterministic demonstration files. The incident and
telemetry data are synthetic demonstrations, not production measurements.

- `incident.*`: 96 checkpoints, wrapped multilingual prose, tasks, callouts,
  formulas and native tables; markers `SCROLL-BEGIN`, `ANCHOR-000`…`ANCHOR-095`,
  `SCROLL-END` make lost or repeated sections observable.
- `telemetry.csv` / `.tsv`: 1,500 records, quoted separators, embedded newline,
  quotes and multilingual fields; final marker `RECORD-1499`.
- `analysis.ipynb`: 32 cells, prose, code, stream output, SVG, PNG and a Vega-Lite
  MIME output. Saved outputs are rendered; Python is not executed.
- Mermaid, DOT, SVG, PNG, LaTeX and chemistry files exercise diagram labels,
  real glyphs, palette changes, equations and scrolling through image rows.
- Vega-Lite and Plotly files use the implemented chart subset. They are not
  conformance suites for the complete upstream specifications.
- ZIP/JAR/wheel/TAR files are valid containers; compressed variants carry real
  compressed payloads. The preview shows supported metadata, not an extraction UI.
- `queueing.typ` explicitly illustrates the current prose subset. Native Typst
  mathematical layout remains unsupported.

Rebuild inputs with `python3 showcase/build_stands.py` (Python standard library).

```sh
cargo +stable build
python3 showcase/checks/verify.py --report showcase/checks/latest.json
python3 showcase/checks/compare_scan.py --report showcase/checks/scan-comparison.csv
```

The verifier launches a real isolated daemon, sends actual file requests, checks
both themes and two widths, scrolls forward/back/to EOF, decodes Base64 RGBA/PNG KGP uploads
and validates RGBA dimensions, opacity and non-uniform pixels. It also compares
CLI framing with direct IPC and checks that the last CSV record remains reachable.
This verifies transport and basic image content, not subjective visual quality.

`compare_scan.py` reads the historical scanner directly from commit
`a2d84066f838f42364ecc8b7dd56573758dc4f2e` in `../ii_old`. Both scanners run in
one optimized executable on identical corpus bytes. Reports include span counts:
different counts must be explained before comparing throughput as equivalent work.
GiB/s here means source bytes scanned, not terminal rendering throughput.

For the editor test, run an isolated `ii --daemon --socket <private-path>` and:

```sh
II_ROOT="$PWD" II_SOCKET="<private-path>" nvim --headless -u NONE -l showcase/checks/nvim.lua
```

`checks/latest.json` records local timings and the binary path used for the run;
these are not universal performance promises. `checks/scan-comparison.csv` records an optimized scanner run.

## Yazi input and graphics regression

```sh
python3 showcase/checks/yazi.py --binary target/release/ii --report showcase/checks/yazi-latest.json
```

This runs real Yazi with private configuration in a controlling PTY, sends 40 arrow
keys and 40 scroll keys, answers capability queries and parses every output packet.
It checks that every graphic command suppresses responses, no UI redraw interrupts
an APC, no Base64 or repeated arrow letters appear as text, input remains raw, and
Yazi exits normally. The old competing CLI writer failed this test. The main-loop
publication passes. A real emulator is still needed to assess visual appearance.
