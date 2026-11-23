# ii.nvim

Asynchronous Neovim 0.12+ adapter for graphical spans in Markdown, MDX, Quarto
and R Markdown buffers. Requires `ii` on `PATH` and a Kitty-compatible terminal.
It does not execute MDX/R/Python code or provide a standalone notebook editor.

```lua
vim.opt.runtimepath:append('/path/to/ii/plugins/ii.nvim')
require('ii').setup({
  executable = '/path/to/ii/target/release/ii',
  debounce_ms = 20,
  timeout_ms = 5000,
  -- socket_path = '/custom/private/directory/ii.sock',
})
```

The adapter sends the current buffer to IXPC command 3. Source anchors in the
response remain in buffer coordinates; graphics are shown using virtual lines.
One window owns one frame and at most one outstanding request. Changed source,
window geometry, toggles and closed windows invalidate pending results. A frame
becomes ready after it has been applied on Neovim's main loop.

Editor headings, callouts and tasks use the historical Catppuccin palettes and
markers, matching the daemon. Backgrounds are transparent. Terminal output uses
`nvim_ui_send`, ordered by Neovim's TUI; direct stdout writes are not used. Image highlight names are reused per window
and placement rather than allocated for every edited image identifier.

Commands: `IiRender`, `IiToggle`, `IiClear`. `require('ii').status(win)` reports
whether a request is pending, placement count, source change tick and render width.

The real-daemon headless test is `showcase/checks/nvim.lua`; it checks scrolling,
split widths, edits during requests, cancellation and toggling. It does not certify
visual appearance in every terminal. Inline formulas currently use virtual lines.
