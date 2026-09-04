"""Emit musical highlight marks as JSON for the Rust video planner.

The highlight video used to cut scenes on a metronome: every segment was
exactly `total/n` long regardless of what the music did, so transitions
landed mid-phrase as often as not. This module scores each analysis frame
and reports the timestamps worth cutting on, letting the planner snap scene
boundaries to the music.

Scoring combines three deterministic, numpy-only features already used by
the visualizer:

* **onset strength** — spectral flux, catches percussive entries;
* **energy lift** — rise in broadband energy over a ~2 s horizon, catches
  swells and section changes in ambient material that has no transients;
* **brightness lift** — rise in spectral centroid, catches timbral changes
  (pads opening up, strings entering) that carry no extra energy.

Ambient meditation tracks are the hard case: they can run for minutes with
no onset at all. The energy/brightness terms are what keep this useful
there, and the planner always has a uniform fallback.

Run as: `python3 -m visualizer.highlights track.wav -o marks.json`
"""

from __future__ import annotations

import argparse
import json
import sys

import numpy as np

from . import analysis

# Absolute score floor. Scores are *not* max-normalized before thresholding:
# dividing by the maximum would rescale the noise of a featureless track up
# to 1.0 and invent highlights in music that has none.
_MIN_SCORE = 0.08

# A mark must also stand this far above the track's own median score. A
# steady drone scores a flat ~0.13 everywhere (max/median ~1.25) and must
# yield no marks, while real structure is peaky (max/median often > 20).
_MEDIAN_RATIO = 2.0

# Minimum spacing between reported marks, in seconds. Two cuts closer than
# this are never both usable as scene boundaries, so the weaker one is
# dropped at detection time rather than bloating the JSON.
_MIN_SPACING = 3.0

# Horizon for the energy/brightness lift terms, in seconds.
_LIFT_SECONDS = 2.0


def _smooth(values: np.ndarray, width: int) -> np.ndarray:
    """Centered moving average; `width` in frames, odd-padded at the edges."""
    if width < 2 or len(values) < width:
        return values
    pad = width // 2
    padded = np.pad(values, pad, mode="edge")
    kernel = np.ones(width) / width
    return np.convolve(padded, kernel, mode="valid")[: len(values)]


def _lift(values: np.ndarray, span: int) -> np.ndarray:
    """Positive change of `values` over `span` frames, normalized to 0..1.

    A "lift" is deliberately one-sided: a swell into a new section is a cut
    point, the decay out of it is not.
    """
    if span < 1 or len(values) <= span:
        return np.zeros(len(values))
    past = np.concatenate((np.full(span, values[0]), values[:-span]))
    rise = np.maximum(values - past, 0.0)
    peak = float(rise.max())
    return rise / peak if peak > 1e-12 else rise


def highlight_scores(path: str) -> tuple[np.ndarray, float]:
    """Per-frame highlight score plus the feature frame rate.

    Scores are on an absolute scale (roughly 0..1 for real music) rather
    than normalized to the track maximum, so that a track with no musical
    events scores uniformly low instead of having its noise stretched to 1.

    Deterministic: the same WAV always yields the same scores.
    """
    features = analysis.frame_features(path)
    _samples, rate = analysis.load_wav_pcm(path)
    hop_rate = rate / analysis._HOP

    onset = analysis.onset_envelope(features)
    peak = float(onset.max())
    onset = onset / peak if peak > 1e-12 else onset

    span = max(1, int(round(_LIFT_SECONDS * hop_rate)))
    energy = _smooth(features.mean(axis=1), span)
    centroid = _smooth(analysis.spectral_centroid(path), span)

    score = 0.5 * onset + 0.35 * _lift(energy, span) + 0.15 * _lift(centroid, span)
    # Light smoothing so a single noisy frame cannot outrank a real swell.
    score = _smooth(score, max(1, span // 4))
    return score, hop_rate


def pick_marks(
    score: np.ndarray,
    hop_rate: float,
    min_spacing: float = _MIN_SPACING,
    min_score: float = _MIN_SCORE,
) -> list[dict[str, float]]:
    """Greedy peak-pick: strongest first, enforcing `min_spacing` seconds.

    Greedy-by-strength (rather than left-to-right) means that when the
    planner can only use a few of these, the ones it sees are the most
    prominent moments in the track.
    """
    if len(score) == 0:
        return []
    # Threshold on both an absolute floor and prominence over the track's
    # own median, so featureless audio yields nothing at all.
    threshold = max(min_score, _MEDIAN_RATIO * float(np.median(score)))
    gap = max(1, int(round(min_spacing * hop_rate)))
    order = np.argsort(score)[::-1]
    taken: list[int] = []
    for idx in order:
        if score[idx] < threshold:
            break
        if all(abs(int(idx) - t) >= gap for t in taken):
            taken.append(int(idx))
    taken.sort()
    # `strength` is reported relative to the strongest mark so the planner
    # can rank candidates; thresholding above already used absolute scores.
    top = float(score[taken].max()) if taken else 1.0
    top = top if top > 1e-12 else 1.0
    return [
        {"time": round(i / hop_rate, 3), "strength": round(float(score[i]) / top, 4)}
        for i in taken
    ]


def analyze(path: str) -> dict:
    """Full highlight document for `path` (a 22050 Hz mono PCM WAV)."""
    score, hop_rate = highlight_scores(path)
    samples, rate = analysis.load_wav_pcm(path)
    marks = pick_marks(score, hop_rate)
    # The lift terms only peak once a swell has finished rising, so a
    # detected mark trails the moment a listener hears the change by about
    # half the lift horizon. Cutting late is far more noticeable than
    # cutting a hair early, so compensate for that group delay.
    lag = _LIFT_SECONDS / 2.0
    for mark in marks:
        mark["time"] = round(max(0.0, mark["time"] - lag), 3)
    return {
        "duration": round(len(samples) / rate, 3),
        "hop_rate": round(hop_rate, 4),
        "marks": marks,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="do-music-highlights")
    parser.add_argument("audio", help="PCM WAV (22050 Hz mono) from the Rust side")
    parser.add_argument("-o", "--output", help="write JSON here (default: stdout)")
    args = parser.parse_args(argv)

    doc = analyze(args.audio)
    text = json.dumps(doc, indent=2)
    if args.output:
        with open(args.output, "w", encoding="utf-8") as f:
            f.write(text)
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
