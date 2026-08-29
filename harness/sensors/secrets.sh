#!/usr/bin/env bash
# Sensor: secret hygiene (gate). Heuristic patterns; gitleaks is the heavier option.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/../lib.sh"
. "$here/../config.sh"
cd "$here/../.." || exit 1

fail_reasons=""
detail=""

# Degrade loudly instead of silently passing when git is unavailable.
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    emit secrets warn 60 higher 0 "git unavailable; tracked-file secret scan skipped" \
        "Cannot verify committed files. Run this sensor from a working copy so 'git ls-files' works."
    exit 0
fi

# .env must never be tracked.
if git ls-files --error-unmatch .env >/dev/null 2>&1; then
    fail_reasons="$fail_reasons tracked-.env;"
fi

# Common key/token shapes inside the repository. Placeholders like "" or example values are fine.
pattern='(GMI_API_KEY[[:space:]]*=[[:space:]]*"[^"][^"]"|(sk-|xoxb-|ghp_|github_pat_)[A-Za-z0-9_-]{16,}|AKIA[0-9A-Z]{16}|BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY)'
hits="$(git grep -InE "$pattern" -- . 2>/dev/null | grep -v '/\.env\.example' || true)"
if [ -n "$hits" ]; then
    n="$(printf '%s\n' "$hits" | wc -l | tr -d ' ')"
    files="$(printf '%s\n' "$hits" | cut -d: -f1 | sort -u | head -5 | tr '\n' ' ')"
    fail_reasons="$fail_reasons $n pattern hit(s) in: ${files%;}"
fi

if [ -z "$fail_reasons" ]; then
    emit secrets pass 100 higher 0 "no tracked .env, no secret-like strings in tracked files" \
        "Keep reading GMI_API_KEY from the environment only."
else
    fail_reasons="${fail_reasons# }"
    emit secrets fail 0 higher 0 "${fail_reasons%;}" \
        "Never commit secrets: read GMI_API_KEY from env, keep placeholders in .env.example, and if a real key leaked, rotate it and scrub history instead of quietly deleting it."
fi
exit 0
