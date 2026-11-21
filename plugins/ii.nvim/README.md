# `ii.nvim` — Zero-Fork Binary UDS Coprocessor Plugin for Neovim

`ii.nvim` is a zero-dependency, zero-fork Neovim plugin that renders inline and display **LaTeX / `mhchem` mathematics**, **Mermaid**, **Graphviz DOT**, **SVG**, **CSV/TSV charts**, and **Quarto (`.qmd`)** directly inside Neovim buffers using the **Kitty Graphics Protocol (KGP)** (`U+10EEEE` Unicode Placeholders).

---

## Architecture

- **Zero Subprocess Forks**: Connects directly to `$XDG_RUNTIME_DIR/ii.sock` via Neovim's native `libuv` pipe (`vim.uv.new_pipe(false)`). `vim.fn.system` is never used.
- **Auto-Daemon Bootstrap**: If `ii.sock` is not listening, `ii.nvim` automatically spawns `ii --mode daemon` detached in the background.
- **Binary `IXPC` Wire Protocol (`CMD_COMPOSE_EXTMARKS = 0x0003`)**:
  - Sends a 32-byte `IXPC` binary header + buffer bytes over the persistent UDS stream.
  - Receives packed 16-byte [`ExtmarkRecord`](../../src/compose.rs) structures (`<I4I2I2I2I2I2I2`: `image_id: u32`, `start_row: u16`, `start_col: u16`, `end_row: u16`, `end_col: u16`, `cols: u16`, `rows: u16`) and unpacks them via LuaJIT `bit` operations.
- **O(1) Placeholder & Highlight Caching**:
  - Precomputes all 115 Kitty combining diacritics (`U+10EEEE`) at load time and memoizes placeholder rows (`M.row_cache`) and 24-bit image ID highlight groups (`IiImg_%06x` in `M.hl_cache`).
  - Inline math (`rows == 1`) is placed via `virt_text_pos = "inline"` with `conceal = ""`; multi-row formulas and diagrams are placed via `virt_lines`.

---

## Installation

### `lazy.nvim` (Local or Repository)
```lua
{
  dir = "/path/to/ii/plugins/ii.nvim",
  ft = { "markdown", "quarto", "tex", "typst", "mermaid", "dot", "svg", "csv", "tsv", "ipynb", "plotly", "json" },
  config = function()
    require("ii").setup({
      -- Optional custom UDS socket path (defaults to $XDG_RUNTIME_DIR/ii.sock)
      socket_path = nil,
    })
  end,
}
```

### Standalone CLI Generation
```sh
mkdir -p ~/.config/nvim/lua/ii
ii init ii.nvim > ~/.config/nvim/lua/ii/init.lua
```

---

## Commands & API

| Command | Lua API | Description |
| :--- | :--- | :--- |
| `:IiRender` | `require("ii").render(bufnr)` | Render KGP placeholders for the current buffer |
| `:IiToggle` | `require("ii").toggle()` | Toggle `ii.nvim` rendering on/off |
| `:IiClear` | `require("ii").clear(bufnr)` | Clear all `ii_kgp` extmarks in the buffer |
