#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
set +e
output=$(env -u AMO_JWT_ISSUER -u AMO_JWT_SECRET INTENT_OS_ENV_FILE=/nonexistent bash "$ROOT/scripts/sign_firefox.sh" 2>&1)
status=$?
set -e
test "$status" -ne 0
printf '%s' "$output" | grep -q 'AMO_JWT_ISSUER'
echo "Signing credential guard OK"
