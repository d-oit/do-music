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
- `src/main.rs` — CLI and orchestration only, plus the network transport (LLM evaluation and GMI music calls).
- `tests/planning.rs` — deterministic tests; must not call live APIs.
- `src/visual.rs` — `visual` command plumbing: embedded Python package
  materialization, `python3 -m visualizer` arg building, numpy check.
- `visualizer/` — numpy-only generative renderer (styles, palettes, FFT
  band analysis); embedded into the binary via `include_str!`.
- `tests/visual.rs` — offline visual-engine tests (python-dependent
  checks skip with a printed note when python3/numpy is absent).
- `harness/` — maintainability sensor harness; see `harness/README.md`.
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

1. **Before finishing a task**, run `./harness/run.sh` (`--fast` suffices mid-task) and fix every gated failure: `fmt clippy tests secrets`. Gate-green is not done while warnings remain: the bar is a fully clean sensor report, not just passing gates.
2. **A failing pre-existing test is a question, not an obstacle**: either you broke behavior (fix the code) or the spec deliberately changed (update the test). Decide explicitly — never weaken or delete tests just to go green.
3. **Suppressions are visible exceptions**: allow lint findings only via `#[allow(...)] // reason`. Raising a threshold in `harness/config.sh` is the absolute exception — prefer refactoring or suppression with a written reason.
4. **Address every warning, including pre-existing ones**: never wave a warning off because you didn't introduce it. Fix pre-existing warnings you encounter in the same task; keep unrelated pre-existing cleanups in their own commit so your functional change stays reviewable.
5. **After real fixes land green**, persist the baseline: `./harness/run.sh --snapshot` (comparison shows regressions/improvements next run).
6. **Advisory results are work items, not decoration**: when `size`/`coupling` report a hub (>8 importers), a >500-line file, a >6-param function, or unhandled unwraps, resolve them — split the module, shrink the signature, convert unwraps to `?` with context — regardless of when they appeared.

## Secrets hygiene
- `.env` stays untracked; only placeholders in `.env.example`.
- Never print or commit keys — not in logs, tests, docs, or error messages.
- If a secret leaks into history, rotate it first; then scrub. Silent deletion is not remediation.

## Harness housekeeping
- Enable the commit gate once: `git config core.hooksPath .githooks`
- New automation should become a sensor (`harness/sensors/`) rather than prose that agents forget to run.
- Skill docs under `.agents/skills/` must stay in sync with behavior; each carries frontmatter validated by NVIDIA SkillEvaluator CI (schema + quality ≥ 70).
