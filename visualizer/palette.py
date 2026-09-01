"""Shared 256-stop uint8 palettes. Each is a hand-tuned ramp; values are
sRGB triplets. Anchors are 0.0 / 0.35 / 0.7 / 1.0 with linear blending.
"""

from __future__ import annotations

import numpy as np

_ANCHOR_STOPS = (0.0, 0.35, 0.7, 1.0)

PALETTES = {
    # Misty greens -> pale gold: calm meditation.
    "zen": (
        (8, 16, 24),
        (32, 72, 60),
        (110, 150, 120),
        (238, 226, 180),
    ),
    # Sumi-e monochrome with paper whites.
    "ink": (
        (12, 12, 14),
        (56, 58, 62),
        (140, 140, 138),
        (244, 242, 236),
    ),
    # Deep-sea teal into cold foam.
    "abyss": (
        (2, 6, 14),
        (8, 40, 66),
        (24, 110, 130),
        (198, 240, 235),
    ),
    # Charcoal through ember orange to hot ash.
    "ember": (
        (10, 8, 8),
        (96, 30, 18),
        (224, 120, 34),
        (252, 226, 168),
    ),
    # Dawn healing: deep indigo night, violet, teal, pale dawn gold.
    "aurora": (
        (12, 10, 34),
        (88, 42, 122),
        (34, 124, 158),
        (250, 212, 142),
    ),
    # Cyberpunk neon: near-black, deep purple, hot magenta, electric cyan.
    "neon": (
        (6, 4, 18),
        (74, 16, 120),
        (236, 32, 128),
        (52, 232, 222),
    ),
}


def palette_lut(name: str) -> np.ndarray:
    """256x3 uint8 LUT for the named palette (raises on unknown names)."""
    if name not in PALETTES:
        raise ValueError(f"unknown palette {name!r}; choices: {sorted(PALETTES)}")
    anchors = np.asarray(PALETTES[name], dtype=np.float64)
    xs = np.linspace(0.0, 1.0, 256)
    stops = np.asarray(_ANCHOR_STOPS, dtype=np.float64)
    lut = np.stack([np.interp(xs, stops, anchors[:, c]) for c in range(3)], axis=1)
    return np.clip(lut + 0.5, 0, 255).astype(np.uint8)
