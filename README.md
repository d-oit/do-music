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
do-music video <prompt> [--duration 10m] [--scenes <list.json>] [--audio <track>] [--xfade <name>] [--quality fast|balanced|high] [--jobs N] [--genre <name>] [--research] [--no-highlights] [--no-art-direction] [--no-autotune] [--no-i2v]
do-music visual <audio> [--style flow|bloom|plasma|waves|rings] [--palette zen|ink|abyss|ember|aurora] [--codec h264|hevc] [--mirror] [--seed 42]
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

The final `video` output is YouTube-ready by default: MP4 with fast-start,
progressive H.264 High Profile at 30 fps, BT.709 color, and stereo AAC-LC at
48 kHz. H3's short clips are loop-extended before the xfade chain, so scene
boundaries stay aligned to the music; a one-scene list is supported too. Use
`--codec hevc` only when smaller local files matter more than universal
playback. These choices follow YouTube's current upload guidance (H.264,
AAC-LC/48 kHz, 4:2:0, and MP4 fast-start).

### Music-driven art direction

By default the video is designed from the music rather than from a fixed
story. The LLM acts as art director and returns a visual brief — palette,
rendering style, negative prompt and one image prompt per scene forming a
beginning/development/resolution arc — which feeds the Pollinations stills
and the colour grade.

`--research` additionally searches the web for visual references for the
musical style and distils them into the brief. Both steps are best-effort:
a failure prints a note and falls back to the builtin monk arc, so a render
never dies because art direction was unavailable. `--no-art-direction`
restores the old fixed arc.

### Cinematic motion

Scenes no longer share one breathing zoom. Each gets a shot from a
vocabulary — `breathe`, `push-in`, `pull-out`, `drift`, `orbit`, `hold` —
picked by a deterministic rotation that opens and closes on the calmest
move. Pace and travel scale with the musical energy measured *under that
scene*, and each shot carries an atmosphere pass (vignette, S-curve grade,
energy-keyed warmth, optional grain). Transitions vary too: a peak musical
accent gets a light flash, a moderate one a soft wipe, the rest a dissolve.

### Self-tuning renders (`--no-autotune` to disable)

After each render do-music measures how much the finished picture actually
moves and compares it against the music's own energy, appending a record to
`do-music-output/render-memory.json` (capped at 50 entries, no database).
Renders that came out consistently flatter than their music raise the next
run's motion bias; busier ones lower it. The bias is damped and clamped to
0.6–1.5 so it converges instead of oscillating, and history is matched by
genre once there is enough of it — tag renders of the same kind of music
with `--genre <name>` (renders without it share the `general` bucket). A
missing or corrupt memory file is treated as empty.

### Highlight-aware scene cuts

Scene transitions snap to musical events instead of a metronome. The
embedded analyzer (`visualizer/highlights.py`, numpy-only) scores every
frame from three deterministic features:

- **onset strength** (spectral flux) — percussive entries;
- **energy lift** over a 2 s horizon — swells and section changes, the
  useful signal in ambient material with no transients;
- **brightness lift** (spectral centroid) — pads opening up, strings
  entering, changes that carry no extra energy.

Each ideal scene boundary then snaps to the strongest mark within
`--highlight-window` seconds (default 8), subject to every segment staying
long enough to crossfade. Boundaries with no nearby mark keep their even
position, and the durations always sum back to the exact track length, so
the video still lands on the music.

Scores are absolute rather than max-normalized: a featureless track yields
*no* marks rather than having its noise stretched into invented highlights.
Analysis is best-effort — if python3/numpy is missing or the analysis
fails, the render prints a note and falls back to even pacing.

```bash
do-music video --audio track.mp3 --scenes scenes.json      # snapping on (default)
do-music video --audio track.mp3 --no-highlights           # even pacing
do-music video --audio track.mp3 --highlight-window 15     # allow bigger moves
```

### Render tiers (`--quality`)

The video path spends nearly all of its wall time in the per-scene animation
render, so one knob controls the cost/fidelity trade:

| tier | supersample | grain | intermediates | final preset | use |
|---|---|---|---|---|---|
| `fast` | 1× (1920×1080) | off | x264 `ultrafast` | `veryfast` | previews, iterating on scene lists |
| `balanced` *(default)* | 2× (3840×2160) | 5 | x264 `veryfast` | `medium` | normal renders |
| `high` | 3× (5760×3240) | 6 | x265 `medium` | `slow` | archive / final upload |

`--jobs 0` (the default) renders one fallback segment per CPU core, capped at
8 so a long scene list does not thrash memory with many 4K scaler buffers.


Verified live 2026-08-29: 76.2 s track, 76.0 s 1920x1080 video, six scenes
(drop-ripple → koi rises → village flyover → bloom → drift → closing ripple
ring), zero fades. The live check used an older HEVC artifact; new renders use
the YouTube-compatible H.264 default.

## Generative visuals (CPU-only, no API)


`do-music visual` renders audio-reactive generative video from any track:
flow-field particle trails (`flow`), Gray-Scott reaction-diffusion growth
(`bloom`), or domain-warped fbm clouds (`plasma`), colored by five palettes.
Frames are generated frame-by-frame in a numpy-only Python engine
(`visualizer/`, no pip installs beyond numpy) and piped into ffmpeg
(H.264 1080p30 by default); the audio is muxed back at the end. Deterministic
per `--seed`. Band energies (bass/mid/treble) drive motion speed, pattern
contrast, and palette position with a slow-release ambient mapping —
no beat flash. Pass `--codec hevc` for a smaller local artifact.

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
- **Pollinations free tier caps scene stills at 1024×576.** URLs are deterministic (seeded) and double as H3 `first_frame_image`; final quality comes from the supersample + grain at render (3840×2160 on the default `balanced` tier, 5760×3240 on `high`), not the source resolution.
- **Generative-visual bitrate varies by style** (HEVC CRF 19): the flow style
  lands around 2 Mbps for ambient tracks (particle trails are high-entropy),
  while low-motion styles compress far below the 1.5 Mbps target — expected
  for generative content, visually verified artifact-free.
- **TTS (`minimax-tts-speech-2.8-hd`) is out of scope** (user choice; upstream returned persistent 503 during probing).

## Dev harness (do-harness)

The maintainability sensor suite runs on [`d-o-hub/do-harness`](https://github.com/d-o-hub/do-harness), vendored as a git submodule at `vendor/do-harness/`. Sensors are configured in `do-harness.toml`: **gates** `fmt`, `clippy`, `test`, `secrets` fail the run; **advisories** `size`, `coupling`, `audit` (`allow_failure = true`) report findings that AGENTS.md still requires fixing. The do-music-specific sensor scripts live in `scripts/sensors/`.

```bash
cargo install --path vendor/do-harness/crates/do-harness   # once: CLI on PATH
do-harness hook install                                    # once: commit gate
do-harness verify                                          # run the suite
do-harness verify --format json --evidence .do-harness/evidence.json --strict  # CI form
```

CI (`ci.yml`) runs the full suite via the CLI with submodule checkout, plus hard-gate `audit` (cargo-audit) and advisory `mutants` (cargo-mutants) jobs. Update the vendored harness with `git submodule update --remote vendor/do-harness`.

## Status
Implemented: prompt evaluation/planning, GMI request plumbing, dry-run mode,
local long-form assembly, `.env`-based setup with live provider probing,
generation templates, LLM prompt optimization, the H3/ffmpeg highlight
video pipeline, and the CPU-only generative visual engine (`do-music visual`).
Provider response parsing is deliberately defensive because GMI response
envelopes can evolve. Parse failures retry once before failing;
low scores (< 3) warn and continue. See *Known issues* for provider
availability caveats.
