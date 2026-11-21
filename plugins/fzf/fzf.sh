# ii — fzf preview (POSIX sh/bash/zsh). Usage: eval "$(ii init fzf)"
# The ii CLI talks to the daemon over AF_UNIX and spawns it on cold start.
export FZF_CTRL_T_OPTS="--preview 'ii --size \"\${FZF_PREVIEW_COLUMNS}x\${FZF_PREVIEW_LINES}\" -- {} 2>/dev/null || bat --color=always --line-range=:500 {}' --preview-window right:60%"
