<p align="center">
  <a href="https://github.com/zx0r/xc/releases">
    <img src="https://img.shields.io/github/v/release/zx0r/xc?color=4be1ff&label=RELEASE&style=flat-square&logo=github&labelColor=1a1a1a" alt="Release" />
  </a>
  <a href="LICENSE-MIT">
    <img src="https://img.shields.io/badge/LICENSE-MIT%2FApache--2.0-9fef00?style=flat-square&logo=opensourceinitiative&logoColor=white&labelColor=1a1a1a" alt="License" />
  </a>
  <a href="https://www.rust-lang.org/">
    <img src="https://img.shields.io/badge/CORE-RUST_2024-orange?style=flat-square&logo=rust&logoColor=white&labelColor=1a1a1a" alt="Rust 2024" />
  </a>
  <a href="https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html">
    <img src="https://img.shields.io/badge/XDG-COMPLIANT-00FF00?style=flat-square&logo=linux&logoColor=white&labelColor=1a1a1a" alt="XDG Compliant" />
  </a>
</p>

# ii — Terminal Graphical Coprocessor

Terminal graphical coprocessor: renders math, diagrams, charts, and notebooks over the Kitty
Graphics Protocol.

---

### Integrations

#### Yazi (`ii.yazi`)

Install using the Yazi package manager:

```sh
ya pkg add zx0r/ii
```

Configure `~/.config/yazi/yazi.toml`:

```toml
[plugin]
prepend_preloaders = [
  { url = "*.{md,markdown,mdx,qmd,rmd,ipynb,mmd,mermaid,dot,gv,svg,tex,latex,typ,csv,tsv}", run = "ii" },
]
prepend_previewers = [
  { url = "*.{md,markdown,mdx,qmd,rmd}", run = "ii" },
  { url = "*.ipynb",                     run = "ii" },
  { url = "*.{mmd,mermaid,dot,gv,svg}",  run = "ii" },
  { url = "*.{csv,tsv}",                 run = "ii" },
  { url = "*.{tex,latex,typ}",           run = "ii" },
  { mime = "application/{*zip,tar,bzip2,7z*,rar,xz,zstd}", run = "ii" },
]
```

#### Neovim (`ii.nvim`)

Install and configure with `lazy.nvim`:

```lua
{
  "zx0r/ii.nvim",
  ft = {
    "markdown", "markdown.mdx", "md", "mdx", "quarto", "qmd", "rmd", "ipynb",
    "tex", "plaintex", "latex", "typst", "typ", "mermaid", "mmd", "dot", "gv",
    "svg", "png", "csv", "tsv", "plotly", "vl", "vega", "chem", "mhchem",
  },
  cmd = { "Ii", "IiRender", "IiToggle", "IiClear", "IiPreview" },
  keys = {
    { "<leader>pr", "<cmd>IiRender<cr>",  desc = "ii: Render buffer" },
    { "<leader>pt", "<cmd>IiToggle<cr>",  desc = "ii: Toggle KGP rendering" },
    { "<leader>pc", "<cmd>IiClear<cr>",   desc = "ii: Clear decorations" },
    { "<leader>pp", "<cmd>IiPreview<cr>", desc = "ii: Instant preview" },
  },
  opts = {
    -- 10ms debounce tuned for Apple ProMotion 120Hz displays (8.33ms frame window)
    debounce_ms = 10,
    -- IPC deadline timeout for complex multi-node vector charts
    timeout_ms = 3000,
  },
}
```

---

### Installation

```bash
# Cargo (crates.io)
cargo install --locked ii

# Homebrew (macOS / Linux)
brew install zx0r/tap/ii

# Build from source (LTO fat, stripped)
gh repo clone zx0r/ii
cd ii && just release
```

---

### CLI Reference

```bash
# Render document to Kitty terminal
ii README.md

# Pipe from stdin
cat diagram.mmd | ii

# Render specific size / cells
ii --size 80x24 document.ipynb

# Diagnostics
ii doctor

# Background daemon management
ii --daemon
ii --ping
ii --shutdown
```

---

### License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE).
