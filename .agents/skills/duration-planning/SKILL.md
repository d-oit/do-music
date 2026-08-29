---
name: duration-planning
description: "Plan how a requested duration maps to generated tracks. Use for --duration or long-form (>=10 min) requests: single vs multi-track, chunking, section prompts."
license: MIT
version: 0.1.0
metadata:
  author: d-music contributors <d-oit@users.noreply.github.com>
  tags: [music, planning, duration]
---

# Duration Planning

Decide how many tracks a requested duration becomes, and how those tracks stay coherent as one long-form piece.

## Purpose

Convert a requested total length (e.g. 60 minutes) into a concrete, deterministic generation plan: how many tracks, how long each is, and what each section's prompt should say — without ever pretending the provider accepts a duration parameter.

## When to use

- Any `run_generation` call with a `--duration` (default `4m`).
- Long-form requests: ≥ 10 minutes, especially the 60-minute case.
- Explaining *why* a 60-minute result is multiple tracks plus local assembly.

## When NOT to use

- Sending the request to the provider → `gmi-music`.
- Stitching the resulting files → `local-assembly`.

## Prerequisites

- A validated `MusicSpec` from `prompt-evaluator` (its `style_prompt` and `arrangement` feed the section prompts).
- No provider call happens here — this stage is pure planning.

## Core rules

- **The provider never accepts a duration parameter.** A requested 60-minute result means multiple generated tracks plus local assembly — never a single 60-minute request.
- Parse via `Duration::parse` (`src/duration.rs`):
  - `N m` where 3 ≤ N ≤ 60, or `N s` where 180 ≤ N ≤ 3600.
  - Anything else is an error: `duration must be 3m–60m`.
- **< 10 minutes → one track** of the full requested length.
- **≥ 10 minutes → multiple tracks** of at most 240 s each, assembled locally.

## Instructions

1. `Duration::parse(duration)` — fail loudly on anything outside 3m–60m; never guess.
2. Call `plan_tracks(duration)` (`src/plan.rs`):
   - `minutes < 10` → `[requested_seconds]`.
   - Otherwise chunk the total: repeatedly take `min(remaining, 240)` seconds until exhausted.
3. For each track index, build its prompt with `section_prompt(spec, index, total)`.
4. Hand the track lengths + prompts to the generation step; hand the track files to `local-assembly`.

## Examples

- `3m` → `[180]` — one track, no assembly.
- `10m` → `[240, 240, 120]` — three tracks.
- `60m` → `15 × [240]` — fifteen tracks, FFmpeg-concatenated.

## Section prompts

Each track gets a phase from the palette, in order: `opening, settling, deepening, flowing, reflection, release, closing` (wrap at the end). Each prompt must:

- Keep the same musical palette across the whole long-form work.
- Name its position: `section {i}/{total}` and phase.
- End in a way that crossfades cleanly into the next section.
- Reuse the evaluated `style_prompt` and `arrangement` (see `prompt-evaluator`).

## Limitations

- Chunking is length-based only; musical structure (intro/outro placement) is not modeled.
- The 240 s cap is a policy constant (`MAX_TRACK_SECONDS`), not a provider guarantee.
- Single-track results under 10 minutes can exceed 240 s — the cap applies only to the split path.

## Troubleshooting

| Error | Cause | Solution |
| --- | --- | --- |
| `duration must be 3m–60m` | Out-of-range or malformed `--duration` | Re-check the flag value; parse errors are intentionally strict |
| Track count surprising (e.g. 3 for 10m) | Chunk math | Verify: 10m → `[240, 240, 120]`; sums always equal the target |
| Prompts sound unrelated across sections | Phase/palette drift | Confirm `section_prompt` reuses `style_prompt` and `arrangement` verbatim |

## Testing

- `Duration::parse`: valid/invalid inputs (boundaries 3m, 60m, 180s, 3600s, 2m, 61m) in `tests/planning.rs`.
- `plan_tracks`: 3m → 1 track; 10m → 3 tracks; 60m → 15 tracks; all chunks ≤ 240 s and sum to the target.
- All deterministic, no live API calls.
