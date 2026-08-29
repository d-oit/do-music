#!/usr/bin/env bash
# Sensor: size & hygiene (advisory). Targets common AI failure modes:
# oversized files, too many function params, scattered unwraps/expects.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/../lib.sh"
. "$here/../config.sh"
cd "$here/../.." || exit 1

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

detail="${n} size issue(s):${problems} unwraps(src)=$unwraps todos=$todos"

if [ "$n" -gt 0 ]; then
    emit size warn $((100 - n * 10)) higher 0 "$detail" \
        "Split files over ${MAX_FILE_LINES} lines and shrink >$LONG_FN_PARAMS-param signatures. Refactor only where it reduces risk now; convert src unwraps to ? with context."
else
    emit size pass 100 higher 0 "$detail" \
        "Keep functions under ~$LONG_FN_PARAMS params; prefer ? over unwrap in library paths."
fi
exit 0
