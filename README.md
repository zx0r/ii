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

Framed terminal file previewer:

```sh
# Clone plugin into Yazi plugins directory
git clone https://github.com/zx0r/ii.yazi.git ~/.config/yazi/plugins/ii.yazi
```

Add previewers to `~/.config/yazi/yazi.toml`:

```toml
[plugin]
prepend_previewers = [
  { name = "*.md",      run = "ii" },
  { name = "*.ipynb",   run = "ii" },
  { name = "*.tex",     run = "ii" },
  { name = "*.typ",     run = "ii" },
  { name = "*.mmd",     run = "ii" },
  { name = "*.mermaid", run = "ii" },
  { name = "*.dot",     run = "ii" },
  { name = "*.svg",     run = "ii" },
  { name = "*.csv",     run = "ii" },
]
```

#### Neovim (`ii.nvim`)

Inline rendering and floating window previews for Neovim (≥ 0.10):

```lua
-- lazy.nvim
{
  "zx0r/ii.nvim",
  ft = { "markdown", "quarto", "rmd", "tex", "typst" },
  opts = {},
  keys = {
    { "<leader>ip", "<cmd>IiPreview<cr>", desc = "Toggle ii floating preview" },
    { "<leader>ir", "<cmd>IiRender<cr>",  desc = "Render ii inline" },
    { "<leader>ic", "<cmd>IiClear<cr>",   desc = "Clear ii graphics" },
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
