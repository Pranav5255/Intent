# Intent OS shell hook. Source this only from an interactive bash session.
case $- in *i*) ;; *) return ;; esac

_intent_os_command=""
_intent_os_started=0

_intent_os_debug() {
  case "$BASH_COMMAND" in
    _intent_os_*|PROMPT_COMMAND*|intent-os-shell-send*) return ;;
  esac
  _intent_os_command="$BASH_COMMAND"
  _intent_os_started=$SECONDS
}

_intent_os_precmd() {
  local exit_code=$?
  [[ -n "$_intent_os_command" ]] || return
  local duration_ms=$(( (SECONDS - _intent_os_started) * 1000 ))
  printf '%s\0%s\0%s\0%s\0' "$_intent_os_command" "$PWD" "$exit_code" "$duration_ms" |
    intent-os-shell-send >/dev/null 2>&1 &
  _intent_os_command=""
}

trap '_intent_os_debug' DEBUG
PROMPT_COMMAND="_intent_os_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
