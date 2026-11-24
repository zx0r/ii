# ii for fzf

An optional Ctrl-T preview for fzf's shell integration. It uses ii for
Markdown, MDX, Quarto and R Markdown. Other files use bat when available.

From this directory, install the preview command somewhere on PATH:

```sh
mkdir -p "$HOME/.local/bin"
install -m 755 ii-fzf-preview "$HOME/.local/bin/ii-fzf-preview"
```

Source fzf.sh after loading fzf's shell integration. For a single invocation:

```sh
fzf --preview 'ii-fzf-preview {}'
```
