#!/usr/bin/env bash
# Sensor: coupling data (advisory). Fan-in/fan-out per module + hub detection.
# Raw numbers are noisy (see Böckeler): use them for review triage, not as truth.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/../lib.sh"
. "$here/../config.sh"
cd "$here/../.." || exit 1

file_list="$(find src tests -type f -name '*.rs' 2>/dev/null | sort)"
if [ -z "$file_list" ]; then
    emit coupling skip 100 higher 0 "no .rs sources found"
    exit 0
fi

detail=""
hub_list=""
hubs=0
count=0
for f in $file_list; do
    count=$((count + 1))
    fan_out="$(grep -cE '(^[[:space:]]*(pub )?mod [a-z_]|^[[:space:]]*use (crate|super)::)' "$f" 2>/dev/null)" || fan_out=0
    mod="$(basename "$f" .rs)"
    fin=0
    for g in $file_list; do
        [ "$g" = "$f" ] && continue
        grep -qw "$mod" "$g" 2>/dev/null && fin=$((fin + 1))
    done
    detail="$detail$(basename "$f")(in=$fin,out=$fan_out) "
    if [ "$fin" -ge "$HUB_FAN_IN" ]; then
        hubs=$((hubs + 1))
        hub_list="$hub_list $(basename "$f")(in=$fin)"
    fi
done
detail="${detail% }"

guidance_hub="Check each hub: a legitimate contract (e.g. shared spec types) is fine; a god-module is not - split along CLI -> logic -> provider adapter layering."
guidance_single="Fine for now; extract modules along CLI -> logic -> provider adapter when new areas appear."

if [ "$hubs" -gt 0 ]; then
    emit coupling warn 70 higher "$((HUB_FAN_IN * 10))" \
        "high-fan-in hub(s):${hub_list%;} in $count file(s); $detail" "$guidance_hub"
elif [ "$count" -le 1 ]; then
    lines="$(wc -l <src/main.rs 2>/dev/null || echo 0)"
    warn_at=$((MAX_FILE_LINES * 80 / 100))
    if [ "$lines" -ge "$warn_at" ]; then
        emit coupling warn 85 higher 0 "single-file crate near the ${MAX_FILE_LINES}-line limit ($lines lines)" \
            "Extract planning/parse, provider adapter, and assembly into modules before it grows past the limit."
    else
        emit coupling pass 90 higher 0 "single-file crate ($lines lines), no hubs; $detail" "$guidance_single"
    fi
else
    emit coupling pass 100 higher 0 "$count source file(s), no high-fan-in hubs; $detail" \
        "Keep provider adapters free of planning logic and pure logic free of I/O."
fi
exit 0
