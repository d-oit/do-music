# Maintainability sensor harness

A small "sidecar" in the spirit of Birgitta Böckeler's
[Maintainability sensors for coding agents](https://martinfowler.com/articles/sensors-for-coding-agents.html).
Computational sensors run alongside the agent (and humans) to catch the common
failure modes early, so issues self-correct before a human review.

## Sensors

| Sensor    | Gate? | Type          | Watches for                                                     |
| --------- | ----- | ------------- | --------------------------------------------------------------- |
| `fmt`     | yes   | computational | code not rustfmt-clean                                          |
| `clippy`  | yes   | computational | warnings that CI would reject (`-D warnings`)                   |
| `tests`   | yes   | computational | failing or missing tests                                        |
| `secrets` | yes   | computational | tracked `.env`, hard-coded keys/tokens                          |
| `coupling`| no    | data          | hub files (fan-in), single-file drift                           |
| `size`    | no    | computational | files > 500 LOC, > 6-param functions, unwraps, TODOs            |
| `audit`   | no    | opt-in        | dependency CVEs (`cargo audit`; skipped if not installed)       |
| `mutants` | no    | opt-in        | assertion gaps (`MUTANTS=1 harness/run.sh --sensor mutants`)    |

## Usage

```bash
harness/run.sh                 # all sensors
harness/run.sh --fast          # gating subset only
harness/run.sh --sensor size   # one sensor
harness/run.sh --snapshot      # persist current state as baseline
harness/run.sh --json          # machine-readable JSON on stdout
```

Exit codes: `0` green, `1` gate failed, `2` usage error.

The summary is token-efficient by design: a scouting rule up top, one table,
guidance only for non-pass results, snapshot deltas ("regressed/improved"),
and a final GATE verdict. Every run appends a JSON line to
`state/history.jsonl` so trends are answerable later ("which sensors fail most?",
"do guides+models improve over time?").

## Sensor contract

Each `sensors/<name>.sh` prints exactly one line on stdout:

```
RESULT<TAB>name<TAB>status<TAB>score<TAB>direction<TAB>threshold<TAB>detail<TAB>guidance
```

- `status`: `pass | warn | fail | skip | error`
- `score`: integer, higher is better
- build the line with the `emit` helper from `lib.sh` — it strips tabs/newlines

A sensor without a RESULT line is reported as `error` by `run.sh`, not silently skipped.

To add a sensor: drop a script in `sensors/`, add it to `GATE_SENSORS` in
`config.sh` only if failures must block merges/CI, and document it above.

## Configuration policy knobs (`config.sh`)

Threshold changes are the **absolute exception**: prefer refactoring or a
targeted suppression with a written reason (`#[allow(...)] // why`). That keeps
constraints alive — a raised threshold still fires again when things get worse.

## Integration points

- **Guide**: skill `sensor-harness` tells agents when/how to run and interpret.
- **AGENTS.md** "Working style" section requires running before finishing tasks.
- **Pre-commit**: `.githooks/pre-commit` runs `--fast`. Enable with
  `git config core.hooksPath .githooks`.
- **CI**: the `test` job runs `./harness/run.sh --fast`.

State (`state/snapshot.json`, `state/history.jsonl`) is local and gitignored.
