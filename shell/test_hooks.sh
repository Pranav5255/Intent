#!/usr/bin/env bash
# Behavioural test for shell/intent.bash and shell/intent.zsh.
# Drives interactive shells with a fake `intentctl` that logs its arguments.
#   bash shell/test_hooks.sh            # needs bash; zsh part runs if zsh is installed
set -u
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"
cat >"$tmp/bin/intentctl" <<'EOF'
#!/bin/sh
# one write per invocation so concurrent senders do not interleave
line=""
for a in "$@"; do line="$line$a	"; done
printf '%s\n' "$line" >>"$INTENT_TEST_LOG"
EOF
chmod +x "$tmp/bin/intentctl"

fail=0
ok() { echo "ok   - $1"; }
bad() {
  echo "FAIL - $1"
  fail=1
}
has() { grep -qF -- "$2" "$1" && ok "$3" || bad "$3"; }
lacks() { grep -qF -- "$2" "$1" && bad "$3" || ok "$3"; }
count() {
  local n
  n=$(grep -cF -- "$2" "$1")
  [[ "$n" == "$3" ]] && ok "$4" || bad "$4 (got $n, want $3)"
}
settle() {
  # background senders are detached; give them a moment
  local i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    sleep 0.2
  done
}

# ---------------------------------------------------------------- bash
log="$tmp/bash.log"
: >"$log"
INTENT_TEST_LOG=$log PATH="$tmp/bin:$PATH" HISTFILE=/dev/null HISTCONTROL=ignoreboth \
  bash --norc --noprofile -i >/dev/null 2>&1 <<EOF
PROMPT_COMMAND='_user_pc=\$((\${_user_pc:-0}+1))'
trap '_user_trap=1' DEBUG
source "$here/intent.bash"
source "$here/intent.bash"
echo hello | cat
 echo secret-space
 echo hello | cat
false
true
true
INTENT_PAUSED=1
echo paused-cmd
unset INTENT_PAUSED
sleep 0.2
[[ \${_user_pc:-0} -gt 3 && \${_user_trap:-} == 1 ]] && : >"$tmp/bash-chain"
exit
EOF
settle
echo "# bash log:"
sed 's/^/#   /' "$log"
has "$log" $'--cmd=echo hello | cat\t' "bash: full pipeline captured from history"
has "$log" $'--cmd=false\t--cwd=' "bash: simple command captured"
has "$log" $'--cmd=false\t--cwd='"$PWD"$'\t--exit=1\t' "bash: exit status and cwd"
lacks "$log" "secret-space" "bash: leading-space command skipped"
lacks "$log" "paused-cmd" "bash: INTENT_PAUSED respected"
count "$log" $'--cmd=true\t' 2 "bash: ignoredups repeat still reported"
count "$log" $'--cmd=false\t' 1 "bash: double source installs once"
# A space-prefixed line identical to the previous saved command is indistinguishable
# from an ignoredups repeat; it is reported as that (already recorded) command.
count "$log" $'--cmd=echo hello | cat\t' 2 "bash: hidden repeat of last command reported as a repeat only"
has "$log" "--duration-ms=" "bash: duration passed"
[[ -e "$tmp/bash-chain" ]] && ok "bash: existing PROMPT_COMMAND and DEBUG trap still run" ||
  bad "bash: existing PROMPT_COMMAND and DEBUG trap still run"

# ---------------------------------------------------------------- zsh
if command -v zsh >/dev/null 2>&1; then
  log="$tmp/zsh.log"
  : >"$log"
  INTENT_TEST_LOG=$log PATH="$tmp/bin:$PATH" HISTFILE=/dev/null \
    zsh -f -i >/dev/null 2>&1 <<EOF
setopt HIST_IGNORE_SPACE
precmd() { _user_precmd=1 }
source "$here/intent.zsh"
echo hello | cat
 echo secret-space
false
sleep 0.3
INTENT_PAUSED=1
echo paused-cmd
unset INTENT_PAUSED
[[ \${_user_precmd:-} == 1 ]] && : >"$tmp/zsh-chain"
exit
EOF
  settle
  echo "# zsh log:"
  sed 's/^/#   /' "$log"
  has "$log" $'--cmd=echo hello | cat\t' "zsh: full command line captured"
  has "$log" $'--cmd=false\t--cwd='"$PWD"$'\t--exit=1\t' "zsh: exit status and cwd"
  lacks "$log" "secret-space" "zsh: leading-space command skipped"
  lacks "$log" "paused-cmd" "zsh: INTENT_PAUSED respected"
  if grep -F -- $'--cmd=sleep 0.3\t' "$log" | grep -qE -- $'--duration-ms=(2[5-9][0-9]|[3-9][0-9]{2}|[0-9]{4,})\t'; then
    ok "zsh: duration measured"
  else
    bad "zsh: duration measured"
  fi
  [[ -e "$tmp/zsh-chain" ]] && ok "zsh: existing precmd still runs" || bad "zsh: existing precmd still runs"
fi

exit "$fail"
