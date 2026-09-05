#!/usr/bin/env bash
# Sensor: secret hygiene (gate). Heuristic patterns; gitleaks is the heavier option.
# Protocol: exit 0 = pass, exit 1 = fail; one human-readable line on stdout.
set -uo pipefail
cd "$(git rev-parse --show-toplevel 2>/dev/null)" || exit 1

fail_reasons=""

# Degrade loudly instead of silently passing when git is unavailable.
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "secrets: git unavailable; tracked-file secret scan skipped"
    exit 1
fi

# .env must never be tracked.
if git ls-files --error-unmatch .env >/dev/null 2>&1; then
    fail_reasons="$fail_reasons tracked-.env;"
fi

# Common key/token shapes inside the repository. Placeholders like "<your-key>"
# or example values are fine.
pattern='(GMI_API_KEY[[:space:]]*=[[:space:]]*"[^"][^"]"|(sk-|xoxb-|ghp_|github_pat_)[A-Za-z0-9_-]{16,}|AKIA[0-9A-Z]{16}|BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY)'
hits="$(git grep -InE "$pattern" -- . 2>/dev/null | grep -v '/\.env\.example' || true)"
if [ -n "$hits" ]; then
    n="$(printf '%s\n' "$hits" | wc -l | tr -d ' ')"
    files="$(printf '%s\n' "$hits" | cut -d: -f1 | sort -u | head -5 | tr '\n' ' ')"
    fail_reasons="$fail_reasons $n pattern hit(s) in: ${files%;}"
fi

if [ -z "$fail_reasons" ]; then
    echo "secrets: no tracked .env, no secret-like strings in tracked files"
    exit 0
fi
fail_reasons="${fail_reasons# }"
echo "secrets: FAILED ${fail_reasons%;}"
echo "secrets: never commit secrets — read GMI_API_KEY from env, keep placeholders in .env.example; if a real key leaked, rotate it and scrub history"
exit 1
