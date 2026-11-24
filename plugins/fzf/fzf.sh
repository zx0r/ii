# Source after fzf's shell integration to set the Ctrl-T preview.
# fzf parses shell-style quotes in this variable.
# shellcheck disable=SC2089
FZF_CTRL_T_OPTS="${FZF_CTRL_T_OPTS:+$FZF_CTRL_T_OPTS }--preview 'ii-fzf-preview {}' --preview-window right:60%"
# shellcheck disable=SC2090
export FZF_CTRL_T_OPTS
