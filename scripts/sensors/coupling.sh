#!/usr/bin/env bash
# Sensor: coupling data (advisory). Fan-in per module + hub detection.
# Fan-in counts *import edges* — module path references (`mod::`) and `mod`
# declarations, comment lines stripped — so prose and same-named fields do
# not inflate the numbers. Raw numbers are still noisy (see Böckeler): use
# them for review triage, not as truth.
# Protocol: exit 0 = pass/warn (advisory), stdout carries the findings.
set -uo pipefail
cd "$(git rev-parse --show-toplevel 2>/dev/null)" || exit 1

HUB_FAN_IN=8 # local files referenced by >= this many files count as hubs

file_list="$(find src tests -type f -name '*.rs' 2>/dev/null | sort)"
if [ -z "$file_list" ]; then
    echo "coupling: no .rs sources found"
    exit 0
fi

hub_list=""
hubs=0
count=0
detail=""
for f in $file_list; do
    count=$((count + 1))
    mod="$(basename "$f" .rs)"
    fin=0
    for g in $file_list; do
        [ "$g" = "$f" ] && continue
        # An import edge: any path through the module (`mod::`) or a `mod`
        # declaration in a parent file. Comment lines are stripped first:
        # a doc mention is documentation, not coupling.
        grep -vE '^[[:space:]]*//' "$g" 2>/dev/null |
            grep -qE "\b${mod}::|^[[:space:]]*(pub )?mod ${mod}[ ;]" && fin=$((fin + 1))
    done
    detail="$detail$(basename "$f")(in=$fin) "
    if [ "$fin" -ge "$HUB_FAN_IN" ]; then
        hubs=$((hubs + 1))
        hub_list="$hub_list $(basename "$f")(in=$fin)"
    fi
done
detail="${detail% }"

if [ "$hubs" -gt 0 ]; then
    echo "coupling: WARN high-fan-in hub(s):${hub_list%;} in $count file(s); $detail"
    echo "coupling: check each hub — a legitimate contract (shared spec types) is fine; a god-module is not. Split along CLI -> logic -> provider adapter layering."
    exit 0 # advisory
fi
echo "coupling: $count source file(s), no high-fan-in hubs; $detail"
exit 0
