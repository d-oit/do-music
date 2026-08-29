#!/usr/bin/env bash
# Shared helpers for do-music harness sensor scripts.
#
# Sensor protocol: every sensor prints exactly ONE line on stdout:
#   RESULT<TAB>name<TAB>status<TAB>score<TAB>direction<TAB>threshold<TAB>detail<TAB>guidance
#   status:     pass | warn | fail | skip | error   (required)
#   score:      integer, higher is better           (required)
#   direction:  "higher" unless lower is better     (default: higher)
#   threshold:  integer target for consumers        (default: 0)
#   detail:     one-line human summary              (tabs/newlines stripped here)
#   guidance:   self-correction advice shown on non-pass results

emit() { # <name> <status> <score> [direction] [threshold] [detail] [guidance]
    local name="$1" status="$2" score="$3"
    local direction="${4:-higher}" threshold="${5:-0}" detail="${6:-}" guidance="${7:-}"
    detail=${detail//$'\t'/ }
    detail=${detail//$'\n'/ }
    guidance=${guidance//$'\n'/}
    printf 'RESULT\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$name" "$status" "$score" "$direction" "$threshold" "$detail" "$guidance"
}
