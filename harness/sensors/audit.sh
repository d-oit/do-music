#!/usr/bin/env bash
# Sensor: dependency CVE audit (advisory, repeated-cadence sensor).
# Skips cleanly when cargo-audit is not installed.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/../lib.sh"
. "$here/../config.sh"
cd "$here/../.." || exit 1

if ! cargo audit --version >/dev/null 2>&1; then
    emit audit skip 100 higher 0 "cargo-audit not installed (optional)" \
        "Install with 'cargo install cargo-audit' to enable CVE checks; CI can run it on a schedule."
    exit 0
fi

if out="$(cargo audit 2>&1)"; then
    emit audit pass 100 higher 0 "no known vulnerabilities in Cargo dependencies"
else
    emit audit warn 60 higher 0 "cargo audit reported findings" \
        "Review 'cargo audit' output: update affected crates or add an audited ignore entry with a reason and expiry."
fi
exit 0
