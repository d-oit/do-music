# do-music

A tiny, CLI-first, Suno-like music generator powered by GMI Cloud and MiniMax Music 3.0.

## Quick start

```bash
export GMI_API_KEY="..."

cargo run -- "calm ambient piano for studying"
cargo run -- "dreamy indie song about summer ending" --duration 4m
cargo run -- "peaceful ambient music for meditation" --duration 60m
cargo run -- improve "dark happy relaxing techno"
cargo run -- "cinematic ambient music for reading" --duration 60m --dry-run
```

`GMI_API_KEY` is read only from the environment. Never put it in source code or browser bundles.

## Design

- CLI-first; no server, database, auth, or web UI in v1.
- MiniMax text model is used as a fast prompt evaluator/compiler.
- MiniMax Music 3.0 generates individual tracks.
- 3–6 minute requests are single-track jobs.
- Long-form requests are planned as multiple coherent sections and assembled locally.
- FFmpeg is used for local audio assembly when available.
- `--dry-run` performs planning without spending a music-generation request.

## Configuration

```bash
export GMI_API_KEY="..."
export GMI_LLM_MODEL="MiniMaxAI/MiniMax-M2.7"
export GMI_MUSIC_MODEL="minimax-music-3.0"
export GMI_LLM_BASE_URL="https://api.gmi-serving.com/v1"
export GMI_MUSIC_URL="https://console.gmicloud.ai/api/v1/ie/requestqueue/apikey/requests"
```

The URLs remain configurable because provider endpoints can change.

## Commands

```text
do-music <prompt> [--duration 3m|4m|5m|6m|60m] [--instrumental] [--dry-run]
do-music improve <prompt>
do-music optimize <prompt>
do-music setup [--api-key ...] [--llm-model ...] [--music-model ...] [--video-model ...]
do-music providers
do-music template list|show <name>|new <name>
do-music video <prompt> [--duration 10m] [--scenes <list.json>] [--audio <track>] [--xfade <name>] [--no-i2v]
do-music visual <audio> [--style flow|bloom|plasma] [--palette zen|ink|abyss|ember] [--mirror] [--seed 42]
```

## Sample usage: "The River Above" (100% free pipeline)

A complete music-led music video - no Suno, no paid services. Music first,
then every shot cut from the track, transitions hidden inside the motion
(`fadewhite` = the light-flash cut), and the outro lands back on the intro's
composition so the video loops.

```bash
# 1. The track (MiniMax Music 3.0 - free during MiniMax Week).
#    The model decides the real length; a duration parameter does not exist.
do-music optimize '<section-tagged prompt with [Intro][Pulse][Build][Bloom][Drift][Outro]>'
do-music mix '<optimized prompt>' --duration 3m --instrumental -o do-music-output/river-above-3m.mp3

# 2. The video: one scene prompt per musical section (examples/river-above-scenes.json).
#    With --scenes, the music leads: the CLI probes the track and cuts the
#    scene chain to exactly its duration.
do-music video --scenes examples/river-above-scenes.json \
  --audio do-music-output/river-above-3m.mp3 --xfade fadewhite \
  -o do-music-output/river-above-3m.mp4
```

Verified live 2026-08-29: 76.2 s track, 76.0 s 1920x1080 HEVC video, six
scenes (drop-ripple → koi rises → village flyover → bloom → drift → closing
ripple ring), zero fades.

## Generative visuals (CPU-only, no API)


`do-music visual` renders audio-reactive generative video from any track:
flow-field particle trails (`flow`), Gray-Scott reaction-diffusion growth
(`bloom`), or domain-warped fbm clouds (`plasma`), colored by four palettes.
Frames are generated frame-by-frame in a numpy-only Python engine
(`visualizer/`, no pip installs beyond numpy) and piped into ffmpeg
(x265 1080p30); the audio is muxed back at the end. Deterministic per
`--seed`. Band energies (bass/mid/treble) drive motion speed, pattern
contrast, and palette position with a slow-release ambient mapping —
no beat flash.

```bash
do-music visual do-music-output/working-calm-10m.mp3 --style flow --palette zen
```


### Free-services notes (license-verified 2026-08-29)

- **Music: GMI MiniMax Music 3.0** - free during MiniMax Week (2026-08-24 → 09-06).
- **Stills: Pollinations FLUX** - free, deterministic seeded URLs; quality comes
  from the supersample at render, not the 1024x576 source.
- **Motion (optional): Wan 2.2 self-hosted** - Apache-2.0 open weights
  ([Wan-Video/Wan2.2](https://github.com/Wan-Video/Wan2.2)); TI2V-5B runs on a
  24 GB GPU. H3 clips via `do-music video` without `--no-i2v` work when
  credits exist.
- **Stems/beats (manual beat-cut route): Demucs** (MIT, [LICENSE](https://github.com/facebookresearch/demucs/blob/main/LICENSE))
  and **librosa** (ISC, [LICENSE](https://github.com/librosa/librosa/blob/main/LICENSE.md))
  both run locally for free.
- **Beware "Wan 2.7" downloads** - as of 2026-08, Wan 2.2 is the newest
  officially released open-weights version.
- **Generative visuals (2026-08-29 research): local diffusion video is not
  CPU-viable** — Wan 2.2 / LTX-2.5 / HunyuanVideo need 12–24 GB VRAM and CPU
  ports (ltx.cpp) are minutes-per-frame demos; free web I2V tiers (Kling,
  PixVerse, Hailuo, Pika, Vidu) are manual-UI tools with watermarks and no
  API; **Pollinations video is now key-gated** ("pollen" credits; the image
  `/models` endpoint lists only `sana`). Hence `do-music visual`: a
  CPU-only creative-coding engine (numpy + ffmpeg, no pip installs).

## Known issues

- **MiniMax-H3 video returns HTTP 402** ("Insufficient credits") on keys without video credits. `do-music video` tries H3 per scene, prints one warning on the first failure, then renders the remaining scenes with the supersampled ffmpeg fallback — the video never aborts. H3 starts working the moment credits exist.
- **LLM `MiniMaxAI/MiniMax-M2.7` is intermittently unavailable upstream** (HTTP 429/520/521 observed 2026-08-29). `MiniMaxAI/MiniMax-M3` answered `200 ok` throughout; select it with `GMI_LLM_MODEL=MiniMaxAI/MiniMax-M3` (or `do-music setup --llm-model`). `do-music providers` shows the live status.
- **MiniMax Week free access ends 2026-09-06.** After that date music generation starts returning 402; the CLI fails loudly with the provider error by design (no silent fallback to a paid model).
- **Pollinations free tier caps scene stills at 1024×576.** URLs are deterministic (seeded) and double as H3 `first_frame_image`; final quality comes from the 5760×3240 supersample + grain at render, not the source resolution.
- **Generative-visual bitrate varies by style** (HEVC CRF 19): the flow style
  lands around 2 Mbps for ambient tracks (particle trails are high-entropy),
  while low-motion styles compress far below the 1.5 Mbps target — expected
  for generative content, visually verified artifact-free.
- **TTS (`minimax-tts-speech-2.8-hd`) is out of scope** (user choice; upstream returned persistent 503 during probing).

## Maintainability harness

`harness/run.sh` runs the maintainability sensor suite (fmt, clippy, tests, secrets gated; coupling, size advisory; audit/mutants opt-in) and prints a status table plus self-correction guidance for humans and agents alike. Persist a baseline with `--snapshot`; enable the commit gate with `git config core.hooksPath .githooks`. See `harness/README.md`.

## Status
Implemented: prompt evaluation/planning, GMI request plumbing, dry-run mode,
local long-form assembly, `.env`-based setup with live provider probing,
generation templates, LLM prompt optimization, the H3/ffmpeg highlight
video pipeline, and the CPU-only generative visual engine (`do-music visual`).
Provider response parsing is deliberately defensive because GMI response
envelopes can evolve. Parse failures retry once before failing;
low scores (< 3) warn and continue. See *Known issues* for provider
availability caveats.
