#!/usr/bin/env bash
# do-music sensor-harness configuration.
# Thresholds here are policy knobs: raising one is the absolute exception —
# prefer refactoring or suppressing a specific finding with a written reason.

# Shown at the top of every summary (the sidecar's "scouting rule").
SCOUT_RULE="Keep changes small, focused and reversible. Fix failures instead of silencing them; suppressions need a written reason."

# Sensors whose failure fails the run and blocks commits (pre-commit) / merges (CI).
GATE_SENSORS="fmt clippy tests secrets"

# Advisory thresholds.
MAX_FILE_LINES=500 # AGENTS.md rule: files below ~500 LOC
LONG_FN_PARAMS=6   # functions with more params are an AI failure mode
HUB_FAN_IN=8       # local files referenced by >= this many files count as hubs

STATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/state"
SNAP_FILE="$STATE_DIR/snapshot.json"
HISTORY_FILE="$STATE_DIR/history.jsonl"
