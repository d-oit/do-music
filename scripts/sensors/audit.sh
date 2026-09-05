#!/usr/bin/env bash
# Sensor: dependency CVE audit (advisory). Skips cleanly when cargo-audit
# is not installed; CI runs the same check as a hard gate.
# Protocol: exit 0 = pass/skip (advisory), stdout carries the findings.
set -uo pipefail
cd "$(git rev-parse --show-toplevel 2>/dev/null)" || exit 1

if ! cargo audit --version >/dev/null 2>&1; then
    echo "audit: cargo-audit not installed (optional) — install with 'cargo install cargo-audit'"
    exit 0
fi

if cargo audit 2>&1; then
    echo "audit: no known vulnerabilities in Cargo dependencies"
    exit 0
fi
echo "audit: WARN cargo audit reported findings"
echo "audit: review 'cargo audit' output — update affected crates or add an audited ignore entry with a reason and expiry"
exit 0
