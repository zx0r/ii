# ii for fzf

Preview Markdown from fzf. Install [ii](https://github.com/zx0r/ii#installation) and make sure it is on PATH.

From this directory:

```sh
mkdir -p "$HOME/.local/bin"
install -m 755 ii-fzf-preview "$HOME/.local/bin/ii-fzf-preview"
fzf --preview 'ii-fzf-preview {}'
```

For Ctrl-T, source `fzf.sh` after fzf's shell integration.
