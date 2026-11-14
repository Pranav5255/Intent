# Intent shell capture for zsh (Intent 2.x).
#
# Source it from ~/.zshrc:
#     [[ -f /usr/share/intent/shell/intent.zsh ]] && source /usr/share/intent/shell/intent.zsh
#
# * No Python and no network code here: each finished command is handed to
#   `intentctl send-shell`, backgrounded and disowned (&!), so the prompt never
#   waits. If intentctl is not installed the hook does nothing.
# * Uses add-zsh-hook, so existing preexec/precmd hooks keep working.
# * Duration comes from $EPOCHREALTIME (zsh/datetime).
# * Skipped: commands typed with a leading space (HIST_IGNORE_SPACE convention),
#   and everything while INTENT_PAUSED is set to a value other than 0.
# * Redaction/truncation of the command text is done by intentctl.

[[ -o interactive ]] || return 0
# Not exported: nested shells install their own hooks.
(( ${+_intent_zsh_loaded} )) && return 0
typeset -g _intent_zsh_loaded=1

zmodload zsh/datetime 2>/dev/null
autoload -Uz add-zsh-hook

typeset -g _intent_cmd=''
typeset -g _intent_start=''

_intent_preexec() {
  # $1 is the line as typed (empty if history is off); $3 is the expanded text.
  _intent_cmd=${1:-$3}
  _intent_start=${EPOCHREALTIME:-}
}

_intent_precmd() {
  local ec=$?
  [[ -n $_intent_cmd ]] || return $ec
  local cmd=$_intent_cmd
  _intent_cmd=''
  [[ $cmd == [[:space:]]* ]] && return $ec
  [[ -n ${INTENT_PAUSED:-} && $INTENT_PAUSED != 0 ]] && return $ec
  (( ${+commands[intentctl]} )) || return $ec
  local -i dur=0
  if [[ -n $_intent_start && -n ${EPOCHREALTIME:-} ]]; then
    dur=$(( (EPOCHREALTIME - _intent_start) * 1000 ))
  fi
  (( dur < 0 )) && dur=0
  intentctl send-shell --cmd="$cmd" --cwd="$PWD" --exit="$ec" --duration-ms="$dur" \
    </dev/null >/dev/null 2>&1 &!
  return $ec
}

add-zsh-hook preexec _intent_preexec
add-zsh-hook precmd _intent_precmd
