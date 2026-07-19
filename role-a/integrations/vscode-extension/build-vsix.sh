#!/usr/bin/env bash
set -euo pipefail

EXT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST="$EXT_DIR/dist/intent-os-vscode.vsix"

mkdir -p "$EXT_DIR/dist"
cd "$EXT_DIR"
rm -f "$DIST"

if test -x "$EXT_DIR/node_modules/.bin/vsce" && "$EXT_DIR/node_modules/.bin/vsce" --version >/dev/null 2>&1; then
  "$EXT_DIR/node_modules/.bin/vsce" package --out "$DIST"
else
  if ! command -v npm >/dev/null 2>&1; then
    echo "npm is required to build the Intent OS VSIX" >&2
    exit 1
  fi
  tmp="$(mktemp -d)"
  npm install --prefix "$tmp" @vscode/vsce@3.2.1
  "$tmp/node_modules/.bin/vsce" package --out "$DIST"
  rm -rf "$tmp"
fi

echo "Built $DIST"
