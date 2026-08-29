"""Determinism self-checks: palette anchors, feature smoothing EMA and
per-style frame invariants. The Rust test suite calls these so the
contract is checked against the interpreter that actually renders.
"""

from __future__ import annotations

import numpy as np

from . import analysis
from .styles import STYLES


def check_palette_anchors() -> None:
    """Anchors at 0.0/0.35/0.7/1.0 must appear at LUT indexes 0/89/178/255."""
    from .palette import PALETTES, palette_lut

    for name, anchors in PALETTES.items():
        lut = palette_lut(name)
        for stop, rgb in zip((0.0, 0.35, 0.7, 1.0), anchors):
            idx = min(255, int(round(stop * 255)))
            got = lut[idx].tolist()
            want = [min(255, round(v)) for v in rgb]
            if max(abs(a - b) for a, b in zip(got, want)) > 1:
                raise AssertionError(
                    f"palette {name}: LUT[{idx}]={got} diverges from anchor {want}"
                )
    del PALETTES


def check_smoothing() -> None:
    """EMA: attacks toward loud targets, releases slowly toward quiet."""
    from .styles import _Style

    s = _Style(4, 4, 1)
    loud = np.asarray([1.0, 0.5, 0.25])
    quiet = np.zeros(3)
    s._smooth_features(loud)
    if s.smooth[0] < 0.3:  # attack moved most of the way in one frame
        raise AssertionError(f"attack too slow: {s.smooth}")
    for _ in range(3):
        s._smooth_features(quiet)
    if s.smooth[0] > 0.5 * loud[0]:
        raise AssertionError(f"release too fast: {s.smooth}")


def check_style_determinism(name: str, width: int = 64, height: int = 36) -> None:
    """Same seed -> identical frames; different seed -> different frames."""
    cls = STYLES[name]
    a = cls(width, height, 42)
    b = cls(width, height, 42)
    c = cls(width, height, 43)
    feats = [0.6, 0.3, 0.1]
    fa1 = a.step(np.asarray(feats), 0.0)
    fa2 = a.step(np.asarray(feats), 1.0 / 30.0)
    fb1 = b.step(np.asarray(feats), 0.0)
    fc1 = c.step(np.asarray(feats), 0.0)
    if fa1.shape != (height, width, 3):
        raise AssertionError(f"{name}: frame shape {fa1.shape} != {(height, width, 3)}")
    if fa1.dtype != np.uint8:
        raise AssertionError(f"{name}: dtype {fa1.dtype} != uint8")
    if not np.array_equal(fa1, fb1):
        raise AssertionError(f"{name}: same seed produced different frames")
    if np.array_equal(fa1, fc1):
        raise AssertionError(f"{name}: different seed produced identical frame")
    if np.array_equal(fa1, fa2):
        raise AssertionError(f"{name}: identical consecutive frames (no evolution)")


def check_band_separation(wav_path: str) -> dict:
    """Bass-dominant vs treble-dominant synthetic tones must dominate
    their own band. Returns the feature matrix for the caller to assert.
    """
    feats = analysis.frame_features(wav_path)
    return {
        "bass_mean": float(feats[:, 0].mean()),
        "mid_mean": float(feats[:, 1].mean()),
        "treble_mean": float(feats[:, 2].mean()),
    }
