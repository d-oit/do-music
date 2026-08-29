#!/usr/bin/env bash
# Sensor: clippy with warnings-as-errors, matching CI (gate).
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/../lib.sh"
. "$here/../config.sh"
cd "$here/../.." || exit 1

if cargo clippy --all-targets --all-features -- -D warnings >/dev/null 2>&1; then
    emit clippy pass 100 higher 0 "clippy clean (-D warnings)" \
        "New pure logic deserves a deterministic test in tests/planning.rs."
else
    emit clippy fail 0 higher 0 "clippy reported findings at warning level" \
        "Fix the findings. Suppress only with #[allow(...)] // reason; prefer fixing. Threshold changes are the absolute exception."
fi
exit 0
