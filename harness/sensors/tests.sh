#!/usr/bin/env bash
# Sensor: test suite as regression sensor (gate).
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/../lib.sh"
. "$here/../config.sh"
cd "$here/../.." || exit 1

out="$(cargo test --all-targets 2>&1)"
ok_total="$(awk '/test result: ok\./ {s+=$4} END{print s+0}' <<<"$out")"
bad="$(awk '/test result: FAILED/ {s+=$6} END{print s+0}' <<<"$out")"

if [ "$bad" -gt 0 ]; then
    emit tests fail 0 higher 0 "$bad failed, $ok_total passed" \
        "A failing pre-existing test = regression (fix the code) or deliberate spec change (update the test). Decide explicitly; never weaken tests just to go green."
elif grep -q '^error' <<<"$out"; then
    emit tests fail 0 higher 0 "compile error during tests" \
        "Fix compilation errors first (see 'cargo test' output); do not skip targets."
elif [ "$ok_total" -eq 0 ]; then
    emit tests warn 50 higher 0 "no tests found" \
        "Add deterministic tests for prompt parsing, duration planning, API serialization, and assembly planning."
else
    emit tests pass 100 higher 0 "$ok_total test(s) passed, 0 failed" \
        "Cover new pure functions here; keep everything free of live API calls."
fi
exit 0
