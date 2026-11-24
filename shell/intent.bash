# shellcheck shell=bash
# Intent shell capture for bash (Intent 2.x).
#
# Source it near the END of ~/.bashrc (after anything else that edits
# PROMPT_COMMAND or the DEBUG trap):
#     [ -f /usr/share/intent/shell/intent.bash ] && . /usr/share/intent/shell/intent.bash
#
# * No Python and no network code here: each finished command is handed to
#   `intentctl send-shell` in a detached background process. If intentctl is not
#   installed the hook does nothing.
# * Never blocks the prompt and never changes $?, $_, an existing DEBUG trap or
#   PROMPT_COMMAND (string or bash 5.1+ array form) - it chains onto them.
# * bash-preexec compatible: if bash-preexec is loaded, registers in
#   preexec_functions / precmd_functions instead of installing its own hooks.
# * The full command line comes from `history 1` (read in the background child),
#   not $BASH_COMMAND, so pipelines and lists are captured whole.
# * Skipped: commands typed with a leading space (HISTCONTROL=ignorespace
#   convention), and everything while INTENT_PAUSED is set to a value other than 0.
# * intentd redacts the command text. intentctl only caps its length.

# Interactive shells only, and only once per shell. The guard is deliberately
# NOT exported: nested shells must install their own hooks (0.1 bug).
case $- in *i*) ;; *) return 0 ;; esac
[[ -n "${_intent_bash_loaded:-}" ]] && return 0
_intent_bash_loaded=1

_intent_pending=
_intent_armed=
_intent_start_us=0
_intent_now=0
_intent_first=
_intent_cmd=
_intent_histcmd=

# Microseconds since the epoch, without forking (EPOCHREALTIME is bash >= 5 and
# uses the locale's decimal separator). Falls back to whole seconds.
_intent_now_us() {
  if [[ -n "${EPOCHREALTIME:-}" ]]; then
    _intent_now=${EPOCHREALTIME//[.,]/}
  else
    _intent_now=$((SECONDS * 1000000))
  fi
}

_intent_enabled() {
  if [[ -n "${INTENT_PAUSED:-}" && "${INTENT_PAUSED}" != 0 ]]; then
    return 1
  fi
  command -v intentctl >/dev/null 2>&1
}

# Runs in a detached background subshell, so the history lookup and the
# intentctl start-up never delay the prompt.
#   $1 exit code  $2 duration ms  $3 1 if a new history entry was added
#   $4 first simple command as executed ($BASH_COMMAND)  $5 cwd  $6 command (bash-preexec mode)
_intent_send() {
  local cmd=$6
  if [[ -z "$cmd" ]]; then
    local HISTTIMEFORMAT='' line num
    line=$(builtin history 1) || return 0
    line=${line#"${line%%[![:space:]]*}"}
    num=${line%%[!0-9]*}
    [[ -n "$num" ]] || return 0
    # "NUM  cmd" or "NUM* cmd" (modified entry)
    cmd=${line:${#num}+2}
    if [[ "$3" != 1 && "$cmd" != "$4"* ]]; then
      # Nothing was added to history and the last entry is a different command:
      # the line was hidden by ignorespace (or history is off). Do not report it.
      # (A repeat dropped by ignoredups still matches and is reported.)
      return 0
    fi
  fi
  case $cmd in '' | [[:space:]]*) return 0 ;; esac
  exec intentctl send-shell --cmd="$cmd" --cwd="$5" --exit="$1" --duration-ms="$2" \
    </dev/null >/dev/null 2>&1
}

# First command of an interactive line (called from the DEBUG trap while armed).
_intent_preexec() {
  _intent_now_us
  _intent_start_us=$_intent_now
  _intent_first=$BASH_COMMAND
  _intent_cmd=
  _intent_pending=1
}

# bash-preexec entry point: $1 is the command line from history.
_intent_bp_preexec() {
  _intent_now_us
  _intent_start_us=$_intent_now
  _intent_first=
  _intent_cmd=$1
  _intent_pending=1
}

# Must run first in PROMPT_COMMAND so $? is the user's exit status.
_intent_precmd() {
  local ec=$?
  if [[ -n "$_intent_pending" ]]; then
    _intent_pending=
    if _intent_enabled; then
      _intent_now_us
      local dur=$(((_intent_now - _intent_start_us) / 1000)) new=0
      if ((dur < 0)); then dur=0; fi
      if [[ -n "$_intent_cmd" || "${HISTCMD:-}" != "$_intent_histcmd" ]]; then new=1; fi
      (_intent_send "$ec" "$dur" "$new" "$_intent_first" "$PWD" "$_intent_cmd" &)
    fi
    _intent_cmd=
  fi
  _intent_histcmd=${HISTCMD:-}
  return "$ec"
}

# Last in PROMPT_COMMAND: the next DEBUG trap belongs to the user's command.
_intent_arm() {
  _intent_armed=1
}

# $1: value to return (keeps $? intact for a chained trap), $2: the caller's $_
# (restored automatically because it is the last argument of this call).
_intent_debug() {
  if [[ -n "$_intent_armed" && -z "${COMP_LINE:-}" ]]; then
    _intent_armed=
    case $BASH_COMMAND in
      _intent_*) ;; # empty Enter: our own PROMPT_COMMAND hook
      *) _intent_preexec ;;
    esac
  fi
  return "$1"
}

_intent_pc_is_array() {
  ((BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1))) &&
    [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]
}

# The DEBUG trap cannot be chained from here: while a file is being sourced (or
# a function runs) bash hides the current DEBUG trap and restores it afterwards.
# Like bash-preexec, defer to the first prompt: a top-level PROMPT_COMMAND
# snippet captures and clears the existing trap, then _intent_install sets ours
# (no DEBUG trap is active on entry, so bash keeps it) and rewrites PROMPT_COMMAND.
_intent_install_string=$'_intent_old_trap=$(trap -p DEBUG)\ntrap - DEBUG\n_intent_install'

_intent_install() {
  # shellcheck disable=SC2154 # set by the PROMPT_COMMAND snippet above
  local old=$_intent_old_trap q="'\\''"
  unset _intent_old_trap
  old=${old#"trap -- '"}
  old=${old%"' DEBUG"}
  old=${old//"$q"/\'}
  if [[ -n "$old" ]]; then
    # shellcheck disable=SC2064 # expand the old trap now, on purpose
    trap "_intent_debug \"\$?\" \"\$_\"; $old" DEBUG
  else
    trap '_intent_debug 0 "$_"' DEBUG
  fi

  if _intent_pc_is_array; then
    local -a kept=()
    local item
    for item in "${PROMPT_COMMAND[@]}"; do
      [[ "$item" == "$_intent_install_string" ]] || kept+=("$item")
    done
    PROMPT_COMMAND=(_intent_precmd "${kept[@]}" _intent_arm)
  else
    local pc=${PROMPT_COMMAND//"$_intent_install_string"/}
    # shellcheck disable=SC2178 # PROMPT_COMMAND is a string in this branch
    PROMPT_COMMAND=$'_intent_precmd\n'"${pc}"$'\n_intent_arm'
  fi
  unset -f _intent_install _intent_pc_is_array
  unset _intent_install_string

  _intent_histcmd=${HISTCMD:-}
  _intent_arm # we run last in this prompt's PROMPT_COMMAND
}

if [[ -n "${bash_preexec_imported:-}${__bp_imported:-}" ]]; then
  preexec_functions+=(_intent_bp_preexec)
  precmd_functions+=(_intent_precmd)
  unset -f _intent_install _intent_pc_is_array
  unset _intent_install_string
elif _intent_pc_is_array; then
  PROMPT_COMMAND+=("$_intent_install_string")
elif [[ -n "${PROMPT_COMMAND:-}" ]]; then
  PROMPT_COMMAND="${PROMPT_COMMAND}"$'\n'"${_intent_install_string}"
else
  PROMPT_COMMAND=$_intent_install_string
fi
