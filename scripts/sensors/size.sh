#!/usr/bin/env bash
# Sensor: size & hygiene (advisory). Targets common AI failure modes:
# oversized files, too many function params, scattered unwraps/expects.
# Protocol: exit 0 = pass/warn (advisory), stdout carries the findings.
set -uo pipefail
cd "$(git rev-parse --show-toplevel 2>/dev/null)" || exit 1

MAX_FILE_LINES=500 # AGENTS.md rule: files below ~500 LOC
LONG_FN_PARAMS=6   # functions with more params are an AI failure mode

problems=""
n=0
for f in $(find src tests -type f -name '*.rs' 2>/dev/null | sort); do
    lines="$(wc -l <"$f" | tr -d ' ')"
    if [ "${lines:-0}" -gt "$MAX_FILE_LINES" ]; then
        problems="$problems $f:${lines}L>${MAX_FILE_LINES};"
        n=$((n + 1))
    fi
    long_fns="$(awk -v limit="$LONG_FN_PARAMS" '
        /fn [A-Za-z_][A-Za-z0-9_]*\(/ {
            sig=$0; sub(/^[^(]*\(/, "", sig);
            idx=index(sig, ")");
            if (idx > 1) {
                args=substr(sig, 1, idx-1); c=gsub(/,/, ",", args)+1;
                if (c > limit) print FILENAME ":" FNR " (" c " params)"
            }
        }' "$f")"
    if [ -n "$long_fns" ]; then
        problems="$problems ${long_fns// /, };"
        n=$((n + 1))
    fi
done
[ -z "$problems" ] && problems=" none"

unwraps="$(grep -rnE '\.(unwrap|expect)\(' src --include='*.rs' 2>/dev/null | wc -l | tr -d ' ')"
todos="$(grep -rnE 'TODO|FIXME|HACK' src tests 2>/dev/null | wc -l | tr -d ' ')"

if [ "$n" -gt 0 ]; then
    echo "size: WARN ${n} size issue(s):${problems} unwraps(src)=$unwraps todos=$todos"
    echo "size: split files over ${MAX_FILE_LINES} lines, shrink >${LONG_FN_PARAMS}-param signatures; prefer ? over unwrap in library paths"
    exit 0 # advisory: findings surface in verify output without failing the gate
fi
echo "size: 0 size issues; unwraps(src)=$unwraps todos=$todos"
exit 0
