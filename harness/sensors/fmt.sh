#!/usr/bin/env bash
# Sensor: rustfmt cleanliness (gate).
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/../lib.sh"
. "$here/../config.sh"
cd "$here/../.." || exit 1

if cargo fmt --all -- --check >/dev/null 2>&1; then
    emit fmt pass 100 higher 0 "cargo fmt clean" "Keep rustfmt untouched; formatting is deterministic."
else
    emit fmt fail 0 higher 0 "cargo fmt --check reported diffs" \
        "Run 'cargo fmt --all'. Never disable rustfmt or add rustfmt.skip to silence this."
fi
exit 0
