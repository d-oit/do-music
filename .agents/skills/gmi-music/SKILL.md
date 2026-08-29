---
name: gmi-music
description: "Generate tracks via GMI MiniMax Music 3.0. Use when serializing requests, calling GMI_MUSIC_URL, or parsing responses. No duration parameter."
license: MIT
version: 0.1.0
metadata:
  author: d-music contributors <d-oit@users.noreply.github.com>
  tags: [music, provider, api, minimax]
---

# GMI Music

Talk to the GMI Cloud MiniMax Music 3.0 provider: build the request, send it, and parse the response defensively.

## Purpose

Define the provider contract — request envelope, env-var configuration, and defensive response parsing — so track generation is reliable even as GMI's response shapes evolve.

## When to use

- Building the `MusicRequest` payload for a track.
- POSTing to `GMI_MUSIC_URL` and handling the response.
- Locating the generated audio URL in a provider envelope.
- Any question about the provider contract (endpoints, models, envelope shapes).

## When NOT to use

- Planning how many tracks a duration needs → `duration-planning`.
- Stitching tracks into a final file → `local-assembly`.
- Evaluating a prompt into a spec → `prompt-evaluator`.

## Prerequisites

- `GMI_API_KEY` from the environment — never hard-coded, never logged.
- Optional env overrides: `GMI_MUSIC_URL` (default `https://console.gmicloud.ai/api/v1/ie/requestqueue/apikey/requests`) and `GMI_MUSIC_MODEL` (default `minimax-music-3.0`).

## Instructions

1. Use `api::music_request` (`src/api.rs`) to build the request envelope:
   - Set `model` from `GMI_MUSIC_MODEL` (default `minimax-music-3.0`).
   - Set `payload.lyrics` to the lyrics text (or `[Inst]` for instrumental).
   - Set `payload.prompt` to the section style prompt.
   - Keep the codec settings: `payload.sample_rate` 44100, `payload.bitrate` 256000, `payload.format` "mp3".
2. Send the request with `Authorization: Bearer <GMI_API_KEY>`.
3. Parse the response as `serde_json::Value`.
4. Locate the audio URL by **searching known keys recursively**: `audio_url`, `audioUrl`, `url`, `download_url`, `downloadUrl` (see `api::find_string` in `src/api.rs`). Do not assume a fixed envelope shape — GMI envelopes evolve.
5. Download the audio bytes and write them to the output path.

## Rules

- **Never send a `duration` parameter to the provider.** MiniMax Music 3.0 does not accept one; long-form is implemented as multiple generated tracks plus local assembly (see `duration-planning` and `local-assembly`).
- If the provider returns a duration, treat it as authoritative for bookkeeping.
- Preserve raw responses for debugging, but scrub secrets before storing or printing them.
- Keep errors secret-free: never echo the bearer key.

## Examples

- `do-music mix "ambient study hour" --duration 60m --dry-run` → plans 15 tracks; with `--dry-run` removed, 15 POSTs to `GMI_MUSIC_URL` each carry `sample_rate: 44100, bitrate: 256000, format: "mp3"` — never a duration field.
- Response with a nested envelope `{"result": {"audio": {"url": "https://…/track.mp3"}}}` → `find_string` returns the URL from the nested `url` key.

## Limitations

- The provider is a black box: envelope shapes and endpoints can change without notice — `find_string` is a mitigation, not a guarantee.
- No retry/backoff is built in; transient failures surface to the user.
- Rate limits and quotas are provider-side and not handled here.

## Troubleshooting

| Error | Cause | Solution |
| --- | --- | --- |
| "GMI response contained no audio URL: …" | Envelope shape changed or key renamed | Inspect the (secret-free) envelope in the error and extend the key list in `find_string` call sites |
| Non-2xx status | Bad key, quota, or endpoint | Check `GMI_API_KEY`, `GMI_MUSIC_URL`, and the scrubbed body |
| "FFmpeg is required for long-form assembly" | Multi-track output without FFmpeg | Install FFmpeg; single-track runs never need it |

## Testing

- Serialization tests without network (`tests/api.rs`): build a `MusicRequest` via `api::music_request` and assert the JSON shape, including that **no duration field exists**.
- Parsing tests: feed fixture envelopes (direct key, nested key, array, missing, non-string value) to `api::find_string` and assert the result.
- These must not require live API calls.
