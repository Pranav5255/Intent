# Intent OS shell hook. Source this only from an interactive zsh session.
[[ -o interactive ]] || return
zmodload zsh/datetime 2>/dev/null
typeset -g _intent_os_command=""
typeset -g _intent_os_started=0

_intent_os_preexec() {
  _intent_os_command="$1"
  _intent_os_started=$EPOCHREALTIME
}

_intent_os_precmd() {
  local exit_code=$?
  [[ -n "$_intent_os_command" ]] || return
  local duration_ms=0
  if [[ -n "$_intent_os_started" && -n "$EPOCHREALTIME" ]]; then
    duration_ms=$(( (EPOCHREALTIME - _intent_os_started) * 1000 ))
  fi
  printf '%s\0%s\0%s\0%s\0' "$_intent_os_command" "$PWD" "$exit_code" "$duration_ms" |
    intent-os-shell-send >/dev/null 2>&1 &!
  _intent_os_command=""
}

preexec_functions+=(_intent_os_preexec)
precmd_functions+=(_intent_os_precmd)
