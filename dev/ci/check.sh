#!/usr/bin/env bash
# The CI gates that run without a Linux desktop session, locally.
#   dev/ci/check.sh           # everything below
#   dev/ci/check.sh --fast    # format, hygiene and clippy only
# The .deb build, end-to-end runs and budgets need Ubuntu: use the Lima VM
# (dev/lima/e2e.sh, dev/lima/e2e-app.sh, dev/ci/budgets.sh) or let CI run them.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)" || exit 1

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
failed=()
run() {
  local name=$1; shift
  printf "%b==> %s%b\n" "$YELLOW" "$name" "$NC"
  if "$@"; then return 0; fi
  failed+=("$name")
}
have() { command -v "$1" >/dev/null; }

run "tree hygiene" dev/ci/tree-hygiene.sh
run "rustfmt" cargo fmt --all --check
run "rustfmt (app)" bash -c 'cd app/src-tauri && cargo fmt --check'
if have shellcheck; then
  run "shellcheck" bash -c "git ls-files -z '*.sh' shell/intent.bash 'packaging/debian/maintainer-scripts/*' '.githooks/*' | xargs -0 shellcheck -S warning"
else
  echo "(shellcheck not installed; CI runs it)"
fi
run "clippy" env RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets --locked -q

if [[ "${1:-}" != --fast ]]; then
  run "test" cargo test --workspace --locked -q
  run "shell hooks" bash shell/test_hooks.sh
  if have node; then
    run "browser companions" bash -c 'cd integrations/chrome-extension && npm test --silent'
  else
    echo "(node not installed; CI runs the browser companion tests)"
  fi
  if [[ -d app/node_modules ]]; then
    run "app typecheck" bash -c 'cd app && npm run -s typecheck'
  else
    echo "(app/node_modules missing; run npm ci in app/ to typecheck locally)"
  fi
  if have cargo-deny; then
    run "cargo deny" cargo deny --locked check
    run "cargo deny (app)" cargo deny --locked --manifest-path app/src-tauri/Cargo.toml check
  else
    echo "(cargo-deny not installed; CI runs it)"
  fi
fi

if ((${#failed[@]})); then
  printf "\n%b✗ failed: %s%b\n" "$RED" "${failed[*]}" "$NC"
  exit 1
fi
printf "\n%b✓ local gates passed%b\n" "$GREEN" "$NC"
