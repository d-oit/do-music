#!/usr/bin/env bash
# do-music maintainability sensor harness.
#
# A pragmatic implementation of the "sensors for coding agents" sidecar
# (https://martinfowler.com/articles/sensors-for-coding-agents.html): runs the
# computational sensors, prints a status table enriched with self-correction
# guidance, compares against a persisted snapshot, and appends history.
#
# Usage:
#   harness/run.sh                 run all sensors
#   harness/run.sh --fast          gating subset only (fmt clippy tests secrets)
#   harness/run.sh --sensor NAME   one sensor only
#   harness/run.sh --snapshot      persist current state as the baseline
#   harness/run.sh --json          machine-readable JSON on stdout
#
# Exit codes: 0 = all gates green, 1 = a gate failed, 2 = usage error.

set -uo pipefail

HARNESS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HARNESS_DIR/.." && pwd)"
cd "$REPO_ROOT" || exit 2

. "$HARNESS_DIR/lib.sh"
. "$HARNESS_DIR/config.sh"

MODE="all"
SNAPSHOT=0
JSON_OUT=0
while [ $# -gt 0 ]; do
    case "$1" in
    --fast) MODE="fast"; shift ;;
    --snapshot) SNAPSHOT=1; shift ;;
    --json) JSON_OUT=1; shift ;;
    --sensor)
        MODE="one:${2:-}"
        shift 2
        ;;
    -h | --help)
        grep '^#' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    *)
        echo "unknown flag: $1 (see $0 --help)" >&2
        exit 2
        ;;
    esac
done

case "$MODE" in
fast)
    selected=""
    for g in $GATE_SENSORS; do [ -f "$HARNESS_DIR/sensors/$g.sh" ] && selected="$selected $g"; done
    ;;
one:*)
    name="${MODE#one:}"
    if [ -z "$name" ] || [ ! -f "$HARNESS_DIR/sensors/$name.sh" ]; then
        echo "unknown or missing sensor: '$name'" >&2
        exit 2
    fi
    selected="$name"
    ;;
*)
    selected=""
    for f in "$HARNESS_DIR"/sensors/*.sh; do
        [ -e "$f" ] || continue
        selected="$selected $(basename "$f" .sh)"
    done
    # Canonical display order: gates first, then advisories, then opt-ins.
    ordered=""
    for s in fmt clippy tests secrets coupling size audit mutants; do
        case " $selected " in
        *" $s "*) ordered="$ordered $s" ;;
        esac
    done
    for s in $selected; do
        case " $ordered " in
        *" $s "*) ;;
        *) ordered="$ordered $s" ;;
        esac
    done
    selected=$ordered
    ;;
esac

[ -z "${selected// /}" ] && {
    echo "no sensors found in $HARNESS_DIR/sensors" >&2
    exit 2
}

names=()
statuses=()
scores=()
directions=()
thresholds=()
details=()
guidances=()

add_result() {
    names+=("$1")
    statuses+=("$2")
    scores+=("$3")
    directions+=("$4")
    thresholds+=("$5")
    details+=("$6")
    guidances+=("${7:-}")
}

rank() { # pass/skip -> 0, warn -> 1, fail -> 2, error -> 3
    case "$1" in
    pass | skip) echo 0 ;;
    warn) echo 1 ;;
    fail) echo 2 ;;
    error) echo 3 ;;
    *) echo 0 ;;
    esac
}

snap_status() { # extract "name":"status" from flat JSON without jq
    [ -f "$SNAP_FILE" ] || return 0
    tr ',' '\n' <"$SNAP_FILE" | sed 's/[{}]//' |
        awk -F'"' -v n="$1" '{for(i=1;i<NF;i++) if($i==n){print $(i+2); exit}}'
}

valid_status() {
    case "$1" in
    pass | warn | fail | skip | error) return 0 ;;
    *) return 1 ;;
    esac
}
valid_score() {
    case "$1" in
    '' | *[!0-9]*) return 1 ;;
    *) return 0 ;;
    esac
}

run_one() { # <script-path> <expected-name>
    local script="$1" expected="$2" raw rc res tag nm st sc dr th det gd snippet
    rc=0
    raw="$($script 2>&1)" || rc=$?
    res="$(printf '%s\n' "$raw" | awk -F'\t' '$1=="RESULT"{last=$0} END{print last}')"
    IFS=$'\t' read -r tag nm st sc dr th det gd <<<"${res:-}"
    if [ "${res:-}" != "" ] && [ "$tag" = "RESULT" ] && valid_status "${st:-}" &&
        valid_score "${sc:-}" && [ "$nm" = "$expected" ]; then
        add_result "$nm" "$st" "$sc" "${dr:-higher}" "${th:-0}" "${det:-}" "${gd:-}"
    else
        snippet="$(printf '%s\n' "$raw" | head -c 160 | tr '\t\n' '  ')"
        add_result "$expected" "error" 0 higher 0 \
            "no RESULT line (rc=${rc:-?}) ${snippet}" \
            "The sensor crashed or emitted nothing. Run 'harness/run.sh --sensor $expected' directly to debug."
    fi
}

for s in $selected; do
    run_one "$HARNESS_DIR/sensors/$s.sh" "$s"
done

# ---- failed gates -----------------------------------------------------------
# Only hard failures (fail) and missing evidence (error) block gates; warn/skip
# stay visible above without failing the run.
failed_gates=""
for i in "${!names[@]}"; do
    st="${statuses[$i]}"
    case "$st" in
    pass | warn | skip) continue ;;
    esac
    case " $GATE_SENSORS " in
    *" ${names[$i]} "*) failed_gates="${failed_gates} ${names[$i]}" ;;
    esac
done
failed_gates="${failed_gates# }"

# ---- output -----------------------------------------------------------------
if [ "$JSON_OUT" -eq 1 ]; then
    printf '{"scout_rule":"%s","sensors":[' "$(printf '%s' "$SCOUT_RULE" | tr '"' "'")"
    first=1
    for i in "${!names[@]}"; do
        [ $first -eq 0 ] && printf ','
        printf '{"name":"%s","status":"%s","score":%s,"direction":"%s","threshold":%s,"detail":"%s","guidance":"%s"}' \
            "${names[$i]}" "${statuses[$i]}" "${scores[$i]}" "${directions[$i]}" \
            "${thresholds[$i]}" "$(printf '%s' "${details[$i]}" | tr '"' "'")" \
            "$(printf '%s' "${guidances[$i]}" | tr '"' "'")"
        first=0
    done
    printf '],"gates_failed":[%s]}\n' \
        "$(printf '%s' "$failed_gates" | awk 'NF{printf "\"%s\",", $1}' | sed 's/,$//')"
else
    width=6
    for n in "${names[@]}"; do [ "${#n}" -gt "$width" ] && width=${#n}; done
    echo "== do-music sensors =="
    echo "scout: $SCOUT_RULE"
    echo ""
    printf '%-*s  %-5s  %5s  %s\n' "$width" sensor status score detail
    printf '%-*s  %-5s  %5s  %s\n' "$width" ------- ----- ----- ------
    for i in "${!names[@]}"; do
        printf '%-*s  %-5s  %5s  %s\n' "$width" "${names[$i]}" "${statuses[$i]}" "${scores[$i]}" "${details[$i]}"
    done
fi

# Self-correction guidance, only for non-pass results (token-efficient).
non_pass=0
for i in "${!names[@]}"; do
    st="${statuses[$i]}"
    [ "$st" = "pass" ] || [ "$st" = "skip" ] || non_pass=$((non_pass + 1))
    if [ "$JSON_OUT" -eq 0 ] && [ "$st" != "pass" ] && [ "$st" != "skip" ] && [ -n "${guidances[$i]}" ]; then
        echo ""
        echo "[${st}] ${names[$i]}: ${guidances[$i]}"
    fi
done

# Snapshot delta + persistence.
if [ "$JSON_OUT" -eq 0 ]; then
    if [ -f "$SNAP_FILE" ]; then
        delta=""
        for i in "${!names[@]}"; do
            old="$(snap_status "${names[$i]}")"
            [ -z "$old" ] && continue
            # skip means "not measured" (opt-in sensor inactive), not a status
            # level — don't report it as an improvement or regression.
            [ "$old" = skip ] && continue
            [ "${statuses[$i]}" = skip ] && continue
            r_new="$(rank "${statuses[$i]}")"
            r_old="$(rank "$old")"
            if [ "$r_new" -gt "$r_old" ]; then
                delta="$delta ${names[$i]} regressed (${old} -> ${statuses[$i]})"
            elif [ "$r_new" -lt "$r_old" ]; then
                delta="$delta ${names[$i]} improved"
            fi
        done
        [ -z "$delta" ] && delta=" unchanged"
        echo ""
        echo "vs baseline:${delta}"
    fi
    if [ "$SNAPSHOT" -eq 1 ]; then
        mkdir -p "$STATE_DIR"
        out="{"
        first=1
        for i in "${!names[@]}"; do
            [ $first -eq 0 ] && out="$out,"
            out="$out\"${names[$i]}\":\"${statuses[$i]}\""
            first=0
        done
        echo "$out}" >"$SNAP_FILE"
        echo "baseline saved: $SNAP_FILE"
    fi
    echo ""
    if [ -n "$failed_gates" ]; then
        echo "GATE FAILED:$failed_gates"
    else
        ran_gates=""
        for i in "${!names[@]}"; do
            case " $GATE_SENSORS " in
            *" ${names[$i]} "*) ran_gates="$ran_gates ${names[$i]}" ;;
            esac
        done
        echo "GATE OK ($(echo ${ran_gates# } | wc -w | tr -d ' ') gate sensors ran)"
    fi
fi

# History for trend analysis (append one JSON line per run).
mkdir -p "$STATE_DIR"
ts="$(date +%Y-%m-%dT%H:%M:%S%z 2>/dev/null || date +%Y-%m-%dT%H:%M:%S)"
line="{\"ts\":\"$ts\",\"mode\":\"$([ "$MODE" = fast ] && echo fast || echo full)\",\"gates_failed\":\"$failed_gates\""
for i in "${!names[@]}"; do
    line="$line,\"${names[$i]}\":\"${statuses[$i]}\""
done
line="$line}"
echo "$line" >>"$HISTORY_FILE"

[ -n "$failed_gates" ] && exit 1
exit 0
