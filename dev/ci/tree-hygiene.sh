#!/usr/bin/env bash
# Repository hygiene: things that must never be committed.
#   dev/ci/tree-hygiene.sh            # whole tree (CI)
#   dev/ci/tree-hygiene.sh --staged   # staged changes only (pre-commit hook)
set -uo pipefail
cd "$(git rev-parse --show-toplevel)" || exit 1

if [[ "${1:-}" == --staged ]]; then
  files=$(git diff --cached --name-only --diff-filter=ACM)
  added() { git diff --cached -U0 | grep -E '^\+' | grep -vE '^\+\+\+ '; }
else
  files=$(git ls-files)
  added() { git ls-files -z | xargs -0 grep -IhE '' 2>/dev/null; }
fi

fail=0
bad() { echo "✗ $*" >&2; fail=1; }

# Env files and key material (examples are fine).
envs=$(printf '%s\n' "$files" | grep -E '(^|/)\.env($|\.)|\.(pem|key|p12|pfx)$|(^|/)id_(rsa|ed25519)' | grep -vE '\.example$' || true)
[[ -z $envs ]] || bad "env or key files: $envs"

# Build output and local state.
junk=$(printf '%s\n' "$files" | grep -E '(^|/)(target|node_modules|dist|__pycache__)/|\.(db|db-wal|db-shm|deb|xpi|vsix)$|(^|/)\.DS_Store$' || true)
[[ -z $junk ]] || bad "build output or local state: $junk"

# Large files (> 1 MB) belong in releases, not in git.
while IFS= read -r f; do
  [[ -f $f ]] || continue
  size=$(wc -c <"$f")
  ((size > 1048576)) && bad "large file ($((size / 1024)) KB): $f"
done <<<"$files"

# Credentials written into code. Matches assignments of long literal values to
# secret-looking names, and well-known token formats.
if added | grep -EIq \
  -e '(api[_-]?key|secret|password|passwd|access[_-]?key|private[_-]?key|auth[_-]?token)["'"'"']?[[:space:]]*[:=][[:space:]]*["'"'"'][A-Za-z0-9/_+=-]{16,}["'"'"']' \
  -e '(ghp|gho|ghu|ghs|github_pat)_[A-Za-z0-9_]{20,}' \
  -e 'AKIA[0-9A-Z]{16}' \
  -e 'sk-[A-Za-z0-9]{32,}' \
  -e 'AIza[0-9A-Za-z_-]{35}' \
  -e '-----BEGIN [A-Z ]*PRIVATE KEY-----'; then
  bad "possible hardcoded credential (inspect with: git grep -nE 'BEGIN .*PRIVATE KEY|ghp_|AKIA|sk-|AIza')"
fi

((fail == 0)) && echo "tree hygiene ok"
exit "$fail"
