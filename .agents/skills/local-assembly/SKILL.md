---
name: local-assembly
description: Assemble generated tracks into one final audio file. Use for mix or multi-track output; single-track copies, multi-track uses FFmpeg concat.
license: MIT
version: 0.1.0
metadata:
  author: d-music contributors <d-oit@users.noreply.github.com>
  tags: [music, assembly, ffmpeg]
---

# Local Assembly

Turn the per-section tracks into one final audio file.

## Purpose

Produce the final deliverable (a single mp3) from whatever the generation step produced: copy for one track, FFmpeg concat for many — deterministically planned, with execution kept separate so the planning is testable offline.

## When to use

- Long-form generation (≥ 10 minutes) produced multiple track files.
- The `mix` command.
- Producing the final output path (`--output` or the default `song.mp3` / `mix-60m.mp3`).

## When NOT to use

- Generating a single track (a plain copy is enough).
- Provider request/response handling → `gmi-music`.

## Prerequisites

- FFmpeg on `PATH` for multi-track assembly. Single-track assembly needs no FFmpeg.
- The track files in order (production order is the assembly order).

## Instructions

1. **Plan first** (`plan_assembly` in `src/assembly.rs`): one file → `AssemblyStep::Copy`; many → `AssemblyStep::Concat` with inputs in order; zero → error.
2. **One file** → copy it to the output path and stop (`assemble` does this with `tokio::fs::copy`).
3. **Multiple files** → require FFmpeg (probe with `ffmpeg -version`).
4. Write a concat list (one `file '<path>'` per line, via `concat_list_content`) next to the output.
5. Run `ffmpeg -y -f concat -safe 0 -i <list> -c copy <output>`.
6. Delete the concat list (best-effort), even on failure.
7. Name the output with `default_output_name(total_seconds)`: ≥ 60 min → `mix-60m.mp3`, otherwise `song.mp3`.

## Examples

- `3m` request, one track `track-01.mp3` → copied to `song.mp3`.
- `60m` request, 15 tracks → `ffmpeg -f concat` over a list of 15 `file '…'` lines → `mix-60m.mp3`.

## Limitations

- `-c copy` requires all inputs to share the same codec/parameters; provider-consistent settings (fixed `sample_rate`/`bitrate`/`format`) are assumed.
- No crossfading or loudness normalization — concatenation is seam-level.
- FFmpeg must be pre-installed; the tool does not vendor it.

## Troubleshooting

| Error | Cause | Solution |
| --- | --- | --- |
| "FFmpeg is required for long-form assembly" | FFmpeg missing from `PATH` | Install FFmpeg; do not fake assembly |
| "FFmpeg assembly failed" | Non-zero ffmpeg exit (e.g. codec mismatch) | Check the ffmpeg error; the temp concat list is removed even on failure |
| Output keeps previous content | Stale file | The command is idempotent (`-y`); re-run `assemble` |

## Testing

- Assembly *planning* is what is tested deterministically (`tests/assembly.rs`): which `AssemblyStep` results, input order, and the exact concat-list content — without invoking FFmpeg.
- Cover the single-file copy path (offline) and the multi-file concat plan.
- The single-track `assemble` execution is tested with a real copy in a tempdir.
