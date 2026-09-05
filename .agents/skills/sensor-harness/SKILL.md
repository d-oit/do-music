---
name: sensor-harness
description: Run and interpret the do-harness sensor suite in do-music. Use before finishing a task or committing, when CI fails, or when assessing codebase health.
license: MIT
version: 0.2.0
metadata:
  author: d-music contributors <d-oit@users.noreply.github.com>
  tags: [maintainability, sensors, quality, do-harness]
---

# Sensor Harness (do-harness)

Fast feedback loop between you and the codebase: one command runs every maintainability sensor and tells you what to fix.

do-music uses [`d-o-hub/do-harness`](https://github.com/d-o-hub/do-harness) (vendored at `vendor/do-harness/`) as its dev harness. Sensors are `[[sensors]]` entries in `do-harness.toml`; the do-music-specific ones are bash scripts under `scripts/sensors/`. Gates fail the run; advisories (`allow_failure = true`) report findings that AGENTS.md still requires you to address.

## Purpose

Make codebase health machine-checked and self-correcting: run `do-harness verify`, fix everything gated sensors report, and treat advisory findings as work items — instead of relying on review luck or suppressing warnings.

## When to use

- **Before finishing any task** — run `do-harness verify` and fix everything the gates report.
- **Before committing** — the pre-commit hook (installed via `do-harness hook install`) already runs `fmt clippy test secrets`.
- **When CI fails** — reproduce locally with `do-harness verify --only <sensor>` first.
- When asked "is the codebase healthy?" → full run plus `do-harness metrics`.

## When NOT to use

- Don't run it after every single edit mid-task; batch it at task end.
- Don't use `task`/`trace`/`distill` workflow features unless the team opts in; this repo uses the sensor suite only.

## Prerequisites

- The CLI on PATH, installed from the vendored submodule (never build it into the repo's shared `target/`):
  `cargo install --path vendor/do-harness/crates/do-harness`
- A Rust toolchain (`cargo`) for `fmt`/`clippy`/`test`.
- A git working copy for the `secrets` sensor (it scans tracked files).
- Optional: `cargo-audit` (fills in the advisory `audit` sensor; CI runs it as a hard gate anyway).

## Instructions

1. Run `do-harness verify` (add `--format json` for machine-readable output).
2. Fix every gate failure (`fmt`, `clippy`, `test`, `secrets`). Do not move on while gates are red.
3. Treat advisory findings (`size`, `coupling`, `audit`) as work items per AGENTS.md: fix them — including pre-existing ones — in the same task, or in a separate cleanup commit if unrelated to your change.
4. Never delete or weaken a test just to make `test` green. A failing pre-existing test means either (a) you broke behavior — fix the code, or (b) the spec changed deliberately — update the test to the new specification. Decide explicitly.
5. Suppress only visibly: `#[allow(...)] // reason why`. Raising a threshold in `do-harness.toml` or a sensor script is the absolute exception.
6. Re-run `do-harness verify` end-to-end before committing; CI stores the evidence artifact (`.do-harness/evidence.json`).

## Commands

| Command | What it does |
| --- | --- |
| `do-harness verify` | All configured sensors; exit 1 if any gate fails |
| `do-harness verify --only <name>` | One sensor (repeatable) |
| `do-harness verify --format json --evidence .do-harness/evidence.json --strict` | CI form: JSON + evidence artifact; `--strict` also fails on weak evidence |
| `do-harness list` | Print configured sensor names |
| `do-harness doctor` | Check binary resolution, git hooks, state-DB migration skew |
| `do-harness hook install` | Install pre-commit/pre-push/commit-msg hooks |
| `do-harness metrics` | Sensor stats, strike counts, eval history |

Sensors: `fmt`, `clippy`, `test`, `secrets` (gating) · `size`, `coupling`, `audit` (advisory, `allow_failure = true`). Mutation testing stays opt-in: run `cargo mutants -j 4` locally; the `mutants` CI job reports survivors without blocking.

## Reading the output

- Text form prints one line per sensor with pass/fail and captured output; JSON form returns a `VerifyReport` (`ok`, `failed`, per-sensor `exit_code`, `duration_ms`, `allow_failure`, output).
- Advisory failures are recorded but never flip `ok` to false — the AGENTS.md warning policy is what makes them actionable.
- After three consecutive fail-fast strikes a sensor is halted; clear the signature with `do-harness errors clear --sensor <name>`.

## Examples

- Before finishing a feature: `do-harness verify` → `clippy` gate fails → fix the finding (or `#[allow(...)] // reason`) → re-run → green → commit (the hook re-checks).
- CI shows `secrets` failing → a hard-coded key pattern sits in a tracked file → scrub it, rotate the key if real, re-run.
- `coupling` warns about a hub → check whether the fan-in is a legitimate contract or a god-module; split along the CLI → logic → provider-adapter layering if it is the latter.

## Adding a sensor

1. Add the script under `scripts/sensors/<name>.sh` (executable), protocol: exit 0 = pass (or advisory warn), exit 1 = fail; print one human-readable finding line to stdout.
2. Register it in `do-harness.toml` as `[[sensors]]` with `name` + `argv`; add `allow_failure = true` only for advisory checks.
3. Gating sensors should also be listed under `[hooks] pre-commit` if they are fast enough for every commit.

## Limitations

- Sensors are heuristic checks, not proofs: green verify ≠ bug-free code.
- `secrets` scans tracked files only; untracked or .gitignored files are out of scope.
- The vendored submodule pins a commit; update deliberately with `git submodule update --remote vendor/do-harness` and re-run `do-harness doctor`.

## Troubleshooting

| Symptom | Cause | Solution |
| --- | --- | --- |
| `doctor` warns "state database: absent" | State DB never initialized | `do-harness init-db` (only needed for `--record`/`task`/`metrics` features) |
| Hook does nothing on commit | CLI not on PATH and no binary at the fallback locations | Reinstall: `cargo install --path vendor/do-harness/crates/do-harness`, then `do-harness hook install` |
| `audit` reports nothing locally but CI audit job fails | `cargo-audit` missing locally, so the advisory sensor skips | `cargo install cargo-audit` |
| Verify exits 2 | Evidence/strict-mode inconsistency (e.g. `--strict` with weak evidence) | Run `do-harness verify --format json --evidence .do-harness/evidence.json --strict` exactly as CI does |
