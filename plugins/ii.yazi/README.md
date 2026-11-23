# ii.yazi

Yazi preview adapter for `ii`. Requires the `ii` executable on `PATH` and a
terminal supporting Kitty Unicode image placeholders.

```sh
cargo +stable build --release
mkdir -p ~/.config/yazi/plugins/ii.yazi
cp plugins/ii.yazi/main.lua ~/.config/yazi/plugins/ii.yazi/main.lua
```

Add preview rules to your Yazi configuration:

```toml
[plugin]
prepend_previewers = [
  { url = "*.{md,markdown,mdx,qmd,rmd,ipynb,tex,latex,chem,mhchem,mmd,mermaid,dot,gv,svg,png,csv,tsv,vl,vega-lite,plotly,zip,jar,whl,tar,tgz,tbz2,txz,tzst}", run = "ii" },
]
```

The adapter starts one CLI process per preview. The CLI starts the daemon when
needed and sends `PreviewFile` (IXPC command 5): visual row offset, file path,
geometry and theme. A bounded prepared-document cache avoids recompiling on scroll.
The response includes only the selected visual rows and their referenced images.
The CLI uses `--machine`; the adapter commits graphics on Yazi's main loop via
`ya.sync`, rejecting superseded URLs and scroll positions. Writing graphics from
an independent child process races Yazi's screen redraws and is deliberately avoided.
Every KGP continuation carries `q=2` so terminal replies do not enter keyboard input.


`seek` uses Yazi's current preview offset and checks the hovered URL. Empty pages
beyond EOF request an upper bound. `preload` intentionally does no work: background
previews must not write graphics into another preview's terminal state.

Use `showcase/scenarios/incident.md`, `telemetry.csv` and `replicas.svg` to exercise
long prose, quoted multiline records and image-row scrolling. Full Typst compilation,
MDX execution and arbitrary Vega/Plotly specifications are not implemented.
