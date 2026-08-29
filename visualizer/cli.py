"""CLI: parse args, analyze audio, run the style loop, pipe raw frames
to ffmpeg. Kept dependency-free apart from numpy so the Rust side can
embed it with `PyO3` or run it via `python3 -m visualizer`.
"""

from __future__ import annotations

import argparse
import subprocess
import sys

import numpy as np

from . import analysis
from .palette import PALETTES
from .styles import STYLES

OUT_W, OUT_H = 1920, 1080
SIM_W, SIM_H = 960, 540
FPS = 30


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="do-music-visualizer")
    p.add_argument("audio", help="PCM WAV (22050 Hz mono) produced by the Rust side")
    p.add_argument("--style", choices=sorted(STYLES), default="flow")
    p.add_argument("--palette", choices=sorted(PALETTES), default="zen")
    p.add_argument("--mirror", action="store_true")
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--fps", type=int, default=FPS)
    p.add_argument("--preset", choices=("fast", "medium"), default="medium", help="x265 preset; fast for long tracks")
    p.add_argument("--sim-width", type=int, default=SIM_W)
    p.add_argument("--sim-height", type=int, default=SIM_H)
    p.add_argument("-o", "--output", required=True, help="output mp4 path")
    p.add_argument("--out-width", type=int, default=OUT_W)
    p.add_argument("--out-height", type=int, default=OUT_H)
    p.add_argument("--ffmpeg", default="ffmpeg")
    return p


def encoder_chain(args) -> list[str]:
    """Scale to output size, grade, and encode x265 at the output size.

    Preview/draft runs may override --fps; the chain stays the same.
    """
    return [
        "-vf",
        ",".join([
            f"scale={args.out_width}:{args.out_height}:flags=lanczos",
            "vignette=PI/6",
            "noise=alls=4:allf=t+u",
            "format=yuv420p",
        ]),
        "-c:v", "libx265",
        "-preset", args.preset,
        "-crf", "19",
        "-pix_fmt", "yuv420p",
    ]


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)

    feats = analysis.frame_features(args.audio)
    n_feat = feats.shape[0]
    # Feature frames advance _HOP samples at 22050 Hz; convert the real
    # audio duration to an exact video frame count at args.fps.
    duration = ((n_feat - 1) * analysis._HOP + analysis._WINDOW) / 22050.0
    n_frames = int(round(duration * args.fps))
    if n_frames != n_feat:
        src_x = np.linspace(0.0, n_feat - 1, n_frames)
        feats = np.stack(
            [np.interp(src_x, np.arange(n_feat), feats[:, b]) for b in range(3)],
            axis=1,
        )

    style = STYLES[args.style](args.sim_width, args.sim_height, args.seed)
    style.set_palette(_lut(args.palette))
    style.mirror = bool(args.mirror)
    chain = encoder_chain(args)

    ff = subprocess.Popen(
        [
            args.ffmpeg,
            "-y",
            # Input: raw RGB frames on stdin.
            "-f", "rawvideo",
            "-pix_fmt", "rgb24",
            "-s", f"{args.sim_width}x{args.sim_height}",
            "-r", str(args.fps),
            "-i", "-",
            # Output: upscale + grade + x265.
            "-an",
            *chain,
            args.output,
        ],
        stdin=subprocess.PIPE,
        stdout=sys.stderr,
        stderr=sys.stderr,
    )

    def log(msg: str) -> None:
        print(msg, file=sys.stderr, flush=True)

    log(f"visualizer: style={args.style} palette={args.palette} seed={args.seed} "
        f"frames={n_frames} sim={args.sim_width}x{args.sim_height}")

    for i in range(n_frames):
        t = i / args.fps
        frame = style.step(feats[i], t)
        if frame.shape[:2] != (args.sim_height, args.sim_width):
            raise RuntimeError(
                f"style produced {frame.shape[:2]}, expected "
                f"{(args.sim_height, args.sim_width)}"
            )
        ff.stdin.write(frame.tobytes())
        if i % (args.fps * 10) == 0:
            log(f"frame {i}/{n_frames}")

    ff.stdin.close()
    if ff.wait() != 0:
        raise SystemExit(f"ffmpeg exited {ff.returncode}")
    log("visualizer: done")
    return 0


def _lut(palette: str) -> np.ndarray:
    from .palette import palette_lut

    return palette_lut(palette)


if __name__ == "__main__":
    raise SystemExit(main())
