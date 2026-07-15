#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SOURCE="$ROOT/integrations/firefox-extension"
ARTIFACTS="$SOURCE/dist/signed"
OUTPUT="$SOURCE/dist/intent-os-firefox-signed.xpi"
ENV_FILE="${INTENT_OS_ENV_FILE:-$ROOT/.env}"

load_signing_environment() {
  [[ -f "$ENV_FILE" ]] || return 0
  while IFS= read -r raw_line || [[ -n "$raw_line" ]]; do
    line="${raw_line%$'\r'}"
    case "$line" in
      ""|\#*) ;;
      AMO_JWT_ISSUER=*)
        [[ -n "${AMO_JWT_ISSUER:-}" ]] || AMO_JWT_ISSUER="${line#AMO_JWT_ISSUER=}"
        ;;
      AMO_JWT_SECRET=*)
        [[ -n "${AMO_JWT_SECRET:-}" ]] || AMO_JWT_SECRET="${line#AMO_JWT_SECRET=}"
        ;;
      *)
        echo "Unsupported entry in $ENV_FILE; only AMO_JWT_ISSUER and AMO_JWT_SECRET are allowed" >&2
        exit 2
        ;;
    esac
  done < "$ENV_FILE"
}

load_signing_environment
: "${AMO_JWT_ISSUER:?Set AMO_JWT_ISSUER in the environment or $ENV_FILE}"
: "${AMO_JWT_SECRET:?Set AMO_JWT_SECRET in the environment or $ENV_FILE}"
export AMO_JWT_ISSUER AMO_JWT_SECRET

rm -rf "$ARTIFACTS"
mkdir -p "$ARTIFACTS"
npx --yes web-ext sign \
  --source-dir "$SOURCE" \
  --artifacts-dir "$ARTIFACTS" \
  --channel unlisted \
  --api-key "$AMO_JWT_ISSUER" \
  --api-secret "$AMO_JWT_SECRET"

SIGNED=$(find "$ARTIFACTS" -maxdepth 1 -type f -name '*.xpi' -print -quit)
[[ -n "$SIGNED" ]] || { echo "Mozilla signing completed without an XPI artifact" >&2; exit 1; }
install -m 0644 "$SIGNED" "$OUTPUT"
sha256sum "$OUTPUT" > "$OUTPUT.sha256"
echo "Signed XPI: $OUTPUT"
echo "Checksum: $OUTPUT.sha256"
