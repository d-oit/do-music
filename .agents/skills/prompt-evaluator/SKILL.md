---
name: prompt-evaluator
description: Turn a music idea into MusicSpec JSON for do-music. Use for the improve command, evaluating or improving a prompt, or before duration planning.
license: MIT
version: 0.1.0
metadata:
  author: d-music contributors <d-oit@users.noreply.github.com>
  tags: [music, prompts, llm, spec]
---

# Prompt Evaluator

Turn a user's natural-language music idea into the compact `MusicSpec` that MiniMax Music 3.0 can render.

## Purpose

Normalize free-form music requests into one strict, provider-ready JSON shape (genre, mood, tempo, instruments, vocals, energy, arrangement, `style_prompt`, `lyrics`, readiness score) so the rest of the pipeline — duration planning, GMI calls, assembly — has a single deterministic input.

## When to use

- The `do-music improve <prompt>` command, or any "evaluate / analyze / improve this prompt" request.
- Before duration planning: every generation path needs a `MusicSpec` first.
- Instrumental requests (`--instrumental`), where vocals and lyrics are forced.

## When NOT to use

- Generating audio directly without an evaluation step.
- Anything unrelated to prompt → `MusicSpec` conversion (see `duration-planning`, `gmi-music`, `local-assembly` for the rest of the pipeline).

## Prerequisites

- `GMI_API_KEY` set in the environment (never hard-coded).
- Optional: `GMI_LLM_BASE_URL` (`https://api.gmi-serving.com/v1`) and `GMI_LLM_MODEL` (`MiniMaxAI/MiniMax-M2.7`) — defaults exist in code.

## Instructions

1. Read the user's prompt.
2. If `--instrumental`, set `vocals = "none"` and `lyrics = "[Inst]"` after evaluation.
3. POST `{GMI_LLM_BASE_URL}/chat/completions` with bearer auth, `temperature: 0.2`, `response_format: { "type": "json_object" }`, and the system prompt below.
4. Parse the returned content as `MusicSpec` via `api::parse_spec` (`src/api.rs`). On a parse error, retry once; if it fails again, fail with a clear error that includes the raw (secret-free) content for debugging.
5. Return the `MusicSpec` to the caller.

### System prompt (source of truth — keep in sync with `evaluate()` in `src/main.rs`)

> You are a concise music director. Convert the user's idea into strict JSON with fields: intent:string, genre:string[], mood:string[], tempo:string, instruments:string[], vocals:string, energy:string, arrangement:string, style_prompt:string, lyrics:string, score:number. Preserve intent. Resolve contradictions. For instrumental music lyrics must be [Inst]. style_prompt must be usable directly by MiniMax Music 3.0 and stay under 2000 characters. lyrics must stay under 3500 characters. Score generation readiness from 0 to 10. Return JSON only.

## Output schema

| Field          | Type     | Constraints                                  |
| -------------- | -------- | -------------------------------------------- |
| `intent`       | string   | Original user intent, preserved              |
| `genre`        | string[] | 1+ genres                                    |
| `mood`         | string[] | 1+ moods                                     |
| `tempo`        | string   | e.g. "slow", "90 BPM"                        |
| `instruments`  | string[] | instrumentation list                         |
| `vocals`       | string   | e.g. "female", "none" for instrumental       |
| `energy`       | string   | low/medium/high or descriptive               |
| `arrangement`  | string   | structure guidance                           |
| `style_prompt` | string   | ≤ 2000 chars, directly usable by the provider |
| `lyrics`       | string   | ≤ 3500 chars; `[Inst]` for instrumental       |
| `score`        | number   | 0–10 generation readiness                    |

## Examples

- `calm ambient piano for studying` → `score` ≥ 7, `genre: ["ambient"]`, `instruments: ["piano"]`, `vocals: "none"` (evaluator decides instrumental), `lyrics: "[Inst]"`.
- `dreamy indie song about summer ending --instrumental` → `vocals: "none"`, `lyrics: "[Inst]"`, `style_prompt` keeps the dreamy indie palette.
- `aggressive lullaby` (contradiction) → resolve conservatively (e.g. soft melody, bright tempo) and note the resolution in `intent`.

## Limitations

- The LLM is a black box: output quality varies; only the *parsing* is deterministic.
- `score` is the LLM's judgment, not a guarantee the provider will accept the spec.
- Long lyrics can exceed provider limits — the 3500-char cap is enforced by prompt, not by code.

## Troubleshooting

| Error | Cause | Solution |
| --- | --- | --- |
| "LLM returned invalid MusicSpec JSON twice" | Model returned prose or truncated JSON twice | Expected after the built-in retry; the raw (secret-free) content is included in the error for debugging |
| `GMI_API_KEY is not set` | Missing environment variable | Export `GMI_API_KEY` before running |
| `score < 3` warning | Under-specified prompt | The CLI warns and continues; improve the prompt (genre, mood, instruments, vocals detail) if the warning appears |

## Testing

- Unit tests run without live API calls (`tests/api.rs`): feed a fixture `LlmResponse` (chat-completion-style JSON) and assert the parsed `MusicSpec` via `parse_spec`.
- Cover instrumental forcing, score passthrough, and rejection of invalid/incomplete JSON.
