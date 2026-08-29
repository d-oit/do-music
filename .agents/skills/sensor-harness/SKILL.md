---
name: sensor-harness
description: Run and interpret the do-music sensor harness. Use before finishing a task or committing, when CI fails, or when assessing codebase health.
license: MIT
version: 0.1.0
metadata:
  author: d-music contributors <d-oit@users.noreply.github.com>
  tags: [maintainability, sensors, quality]
---

# Sensor Harness

Fast feedback loop between you and the codebase: one command runs every maintainability sensor and tells you what to fix.

Based on "Maintainability sensors for coding agents" (Böckeler): computational sensors catch common agent failure modes (unformatted code, warnings, broken tests, leaked secrets, hub files, oversized files) so issues self-correct before reaching a human.

## Purpose

Make codebase health machine-checked and self-correcting: run `harness/run.sh`, fix what the gating sensors report, and persist a baseline so regressions are visible — instead of relying on review luck or suppressing warnings.

## When to use

- **Before finishing any task** — run `harness/run.sh` and fix everything gated sensors report.
- **Before committing** — `.githooks/pre-commit` already runs the fast subset.
- **When CI fails** — reproduce locally with `harness/run.sh` first.
- When asked "is the codebase healthy?" → full run plus snapshot comparison.

## When NOT to use

- Don't run it after every single edit mid-task; batch it at task end.

## Prerequisites

- A Rust toolchain (`cargo`) for `fmt`/`clippy`/`tests`.
- A git working copy for the `secrets` sensor (it scans tracked files; it degrades to `warn` without one).
- Optional: `cargo-audit` (audit sensor), `cargo-mutants` (mutants sensor), FFmpeg not required.

## Instructions

1. Run `harness/run.sh` (or `--fast` for the gating subset).
2. Fix every **fail**. Do not move on while gating sensors are red.
3. For **warn** (advisory) findings, make a judgment call:
   - Refactor only if it genuinely reduces risk for the current task.
   - Suppressing is allowed but must stay visible: `#[allow(...)] // reason why`.
   - Raising a threshold in `harness/config.sh` is the absolute exception — prefer refactoring or suppression with a reason.
4. Never delete or weaken a test just to make `tests` green. A failing pre-existing test means either (a) you broke behavior — fix the code, or (b) the spec changed deliberately — update the test to the new specification. Decide explicitly.
5. Avoid over-engineering spirals: fix what the harness flags, nothing more.
6. When everything is green after real changes, persist the baseline: `harness/run.sh --snapshot`.

## Commands

| Command | What it does |
| --- | --- |
| `harness/run.sh` | All sensors; exit 1 if any gating sensor fails |
| `harness/run.sh --fast` | Gating subset only (`fmt clippy tests secrets`) |
| `harness/run.sh --sensor <name>` | One sensor |
| `harness/run.sh --snapshot` | Also persists the current state as the baseline in `harness/state/snapshot.json` |
| `harness/run.sh --json` | Machine-readable JSON on stdout |

Sensors: `fmt`, `clippy`, `tests`, `secrets` (gating) · `coupling`, `size` (advisory) · `audit`, `mutants` (opt-in, skip unless `cargo audit` / `cargo mutants` are installed).

## Reading the output

- Header shows the scouting rule from `harness/config.sh`.
- Table: sensor, status (`pass|warn|fail|skip|error`), score (higher is better), detail.
- Below the table: self-correction guidance only for non-pass sensors, and snapshot deltas ("regressed"/"improved") when a baseline exists.
- Every run appends one JSON line to `harness/state/history.jsonl` for trend analysis (which sensors fail often?).

## Examples

- Before finishing a feature: `harness/run.sh` → `fmt fail` → run `cargo fmt`, re-run → green → `harness/run.sh --snapshot`.
- CI shows `secrets fail` → the sensor found a hard-coded key pattern in a tracked file → scrub it, re-run, snapshot.
- `tests warn` on a degenerate branch → add a targeted test for the surviving path instead of silencing.

## Adding a sensor

1. Add `harness/sensors/<name>.sh` (executable). It must print exactly one line:
   `RESULT\t<name>\t<status>\t<score>\t<direction>\t<threshold>\t<detail>\t<guidance>`
   using the `emit` helper from `harness/lib.sh`.
2. If it must gate merges/CI, add its name to `GATE_SENSORS` in `harness/config.sh`; otherwise leave it advisory.
3. Document the tool it wraps in `harness/README.md`.

## Limitations

- Sensors are heuristic checks, not proofs: green harness ≠ bug-free code.
- `secrets` scans tracked files only; untracked or .gitignored files are out of scope.
- Snapshot deltas compare status only, not scores — a pass→pass score drop goes unnoticed.

## Troubleshooting

| Symptom | Cause | Solution |
| --- | --- | --- |
| Sensor reported as `error` | Script printed no valid RESULT line | Run `harness/run.sh --sensor <name>` directly; check stderr |
| `secrets` shows `warn` "git unavailable" | No git working copy | Run from a clone; the sensor degrades honestly rather than silently passing |
| `skip` status for audit/mutants | Optional tool not installed | `cargo install cargo-audit` / `cargo-mutants`, or ignore |
| Harness exits 1 in CI but passes locally | fmt/clippy toolchain drift | Re-run `--fast` locally on the same toolchain |
