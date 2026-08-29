#!/usr/bin/env bash
# Sensor: mutation testing (advisory, opt-in). Mutation runs are expensive, so
# they are triggered on demand: MUTANTS=1 harness/run.sh --sensor mutants
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/../lib.sh"
. "$here/../config.sh"
cd "$here/../.." || exit 1

if [ "${MUTANTS:-0}" != "1" ]; then
    emit mutants skip 100 higher 0 "opt-in; run with 'MUTANTS=1 harness/run.sh --sensor mutants'" \
        "Run incrementally on changed code to find assertion gaps that coverage hides."
    exit 0
fi

if ! cargo mutants --version >/dev/null 2>&1; then
    emit mutants skip 100 higher 0 "cargo-mutants not installed" \
        "Install with 'cargo install cargo-mutants' to measure test assertion strength."
    exit 0
fi

# Not --in-place: results land in mutants.out/ and the working tree stays
# untouched. Also, --in-place cannot be combined with --jobs on cargo-mutants
# >= 27, so this form is the portable one.
if out="$(cargo mutants -j 4 2>&1)"; then
    emit mutants pass 100 higher 0 "all generated mutants caught by the test suite"
else
    # grep -c prints 0 and exits 1 when nothing matches; keep the printed 0
    # instead of echoing a second one (which would break the arithmetic below).
    survivors="$(printf '%s\n' "$out" | grep -ciE 'survived|MISS_REQUIRED|NOT_COVERED' || true)"
    emit mutants warn $((100 - survivors * 5)) higher 0 "~${survivors} survivor signal(s); inspect mutants.out/" \
        "Coverage without assertions is false safety. Strengthen assertions or add targeted tests for surviving mutants."
fi
exit 0
