# AGENTS.md

## Mission
Build `do-music` as a small, reliable CLI for GMI Cloud MiniMax Music 3.0.

## Rules
- Keep the core simple; no premature server/database/auth layers.
- Never hard-code API keys or secrets.
- Read `GMI_API_KEY` from the environment.
- Treat provider responses defensively and preserve raw responses for debugging without secrets.
- Use deterministic parsing/planning where possible; use the LLM only for interpretation and prompt improvement.
- A requested 60-minute result means multiple generated tracks plus local assembly; never pretend the provider accepts a duration parameter when it does not.
- Prefer small modules and keep files below ~500 LOC.
- Test prompt parsing, duration planning, API serialization, and assembly planning without requiring live API calls.
- Do not add dependencies without a concrete need.

## Codebase map
- `src/lib.rs` — module declarations only.
- `src/duration.rs` — `Duration` (3m–60m parsing).
- `src/plan.rs` — track planning, `MusicSpec`, section prompts.
- `src/assembly.rs` — copy/FFmpeg planning + execution.
- `src/api.rs` — request/response serialization + defensive parsing.
- `src/quality.rs` — render tiers (`fast`/`balanced`/`high`): supersample factor,
  scaler, grain, intermediate codec/preset, final preset. Pure data.
- `src/main.rs` — CLI and orchestration only, plus the network transport (LLM evaluation and GMI music calls).
- `tests/planning.rs` — deterministic tests; must not call live APIs.
- `src/visual.rs` — `visual` command plumbing: embedded Python package
  materialization, `python3 -m visualizer` arg building, numpy check.
- `visualizer/` — numpy-only generative renderer (styles, palettes, FFT
  band analysis); embedded into the binary via `include_str!`.
- `tests/visual.rs` — offline visual-engine tests (python-dependent
  checks skip with a printed note when python3/numpy is absent).
- `do-harness.toml` — the dev-harness sensor suite: gates `fmt clippy test
  secrets`, advisories `size coupling audit` (`allow_failure = true`).
- `scripts/sensors/` — the do-music-specific sensor scripts those entries run.
- `vendor/do-harness/` — vendored `d-o-hub/do-harness` CLI (git submodule,
  tracks latest main). Install with `cargo install --path
  vendor/do-harness/crates/do-harness`; see its README.
- `.agents/skills/` — guides for this repo: `prompt-evaluator`, `gmi-music`, `duration-planning`, `local-assembly`, `sensor-harness`.

## Structure & dependency rules (layers)
Enforced informally by the `coupling` sensor; keep them true as modules appear:

```
CLI entry / orchestration → pure logic (parsing, planning) → provider adapters (HTTP)
```

- Pure logic must not perform network I/O; provider adapters must not contain planning rules.
- Response-envelope parsing stays tolerant of shape drift but fails loudly (secret-free) when a known shape disappears.
- If you copy-paste a block a third time, extract it instead.

## Working style: sensors before vibes
The harness is part of your definition of done:

1. **Before finishing a task**, run `do-harness verify` and fix every gated failure: `fmt clippy test secrets`. Gate-green is not done while warnings remain: the bar is a fully clean sensor report, not just passing gates.
2. **A failing pre-existing test is a question, not an obstacle**: either you broke behavior (fix the code) or the spec deliberately changed (update the test). Decide explicitly — never weaken or delete tests just to go green.
3. **Suppressions are visible exceptions**: allow lint findings only via `#[allow(...)] // reason`. Raising a threshold in `do-harness.toml` or a sensor script is the absolute exception — prefer refactoring or suppression with a written reason.
4. **Address every warning, including pre-existing ones**: never wave a warning off because you didn't introduce it. Fix pre-existing warnings you encounter in the same task; keep unrelated pre-existing cleanups in their own commit so your functional change stays reviewable.
5. **After real fixes land green**, re-run `do-harness verify` end-to-end before committing; CI stores the evidence artifact (`.do-harness/evidence.json`).
6. **Advisory results are work items, not decoration**: the `size`/`coupling`/`audit` sensors run with `allow_failure = true` (findings cannot block a commit), but when they report a hub (>8 importers), a >500-line file, a >6-param function, unhandled unwraps, or a CVE, resolve it — split the module, shrink the signature, convert unwraps to `?` with context — regardless of when it appeared.

## Secrets hygiene
- `.env` stays untracked; only placeholders in `.env.example`.
- Never print or commit keys — not in logs, tests, docs, or error messages.
- If a secret leaks into history, rotate it first; then scrub. Silent deletion is not remediation.

## Harness housekeeping
- Install the CLI once: `cargo install --path vendor/do-harness/crates/do-harness`
- Enable the commit gate once: `do-harness hook install`
- Update the vendored harness to latest: `git submodule update --remote vendor/do-harness`
- New automation should become a sensor (`do-harness.toml` + `scripts/sensors/`) rather than prose that agents forget to run.
- Skill docs under `.agents/skills/` must stay in sync with behavior; each carries frontmatter validated by NVIDIA SkillEvaluator CI (schema + quality ≥ 70).
