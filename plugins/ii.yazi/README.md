# `ii.yazi` — Zero-Fork Graphical Coprocessor Plugin for Yazi

`ii.yazi` provides sub-millisecond graphical previews of scientific and technical documents (`.md`, `.markdown`, `.qmd`, `.ipynb`, `.tex`, `.typ`, `.mmd`, `.mermaid`, `.dot`, `.gv`, `.svg`, `.csv`, `.tsv`) inside [Yazi](https://github.com/sxyazi/yazi) via the **Kitty Graphics Protocol (KGP)** (`U+10EEEE` Unicode Placeholders) and the persistent `ii` Unix Domain Socket daemon (`CMD_RENDER_FILE = 0x0004`).

---

## Architecture

1. **Automatic Daemon Bootstrap**: On first invocation (`M:peek` or `M:preload`), `ii.yazi` ensures `ii --mode daemon` is running in the background (`state.ii_daemon_started`).
2. **Zero-Copy File & Viewport Dispatch**: Instead of loading system fonts (`fontdb`) on every preview or piping megabytes of file content, `ii` sends the file path, viewport dimensions (`--size <job.area.w>x<job.area.h>`), and scroll offset (`--skip <job.skip>`) over `$XDG_RUNTIME_DIR/ii.sock` via `CMD_RENDER_FILE`.
3. **Background Preloading (`M:preload`)**: Warms both the two-tier BLAKE3 CAS (`L1`/`L2`) and Kitty's terminal VRAM (`VramEntry`) before the cursor lands on the file.

---

## Installation

### 1. Generate or link `ii.yazi`
```sh
mkdir -p ~/.config/yazi/plugins/ii.yazi
ii init ii.yazi > ~/.config/yazi/plugins/ii.yazi/main.lua
```

### 2. Configure `~/.config/yazi/yazi.toml`
```toml
[plugin]
prepend_preloaders = [
  { name = "*.{md,markdown,qmd,ipynb,tex,typ,mmd,mermaid,dot,gv,svg,csv,tsv}", run = "ii" },
]

prepend_previewers = [
  { name = "*.{md,markdown,qmd,ipynb,tex,typ,mmd,mermaid,dot,gv,svg,csv,tsv}", run = "ii" },
]
```
Or append automatically via:
```sh
ii init yazi >> ~/.config/yazi/yazi.toml
```
