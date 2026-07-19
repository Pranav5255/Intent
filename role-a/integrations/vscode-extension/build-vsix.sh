#!/usr/bin/env bash
set -euo pipefail

EXT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST="$EXT_DIR/dist/intent-os-vscode.vsix"

mkdir -p "$EXT_DIR/dist"
cd "$EXT_DIR"

if command -v npx >/dev/null 2>&1; then
  npm install --no-save @vscode/vsce@3.2.1 >/dev/null 2>&1 || true
  npx --yes @vscode/vsce@3.2.1 package --out "$DIST"
else
  tmp="$(mktemp -d)"
  mkdir -p "$tmp/extension"
  cp package.json extension.js .vscodeignore "$tmp/extension/"
  (cd "$tmp" && zip -qr "$DIST" extension)
  rm -rf "$tmp"
fi

echo "Built $DIST"
