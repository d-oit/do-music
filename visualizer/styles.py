"""The three generative styles. numpy only, fully deterministic per seed.

Every style implements: `__init__(width, height, seed)`, `.step(features,
t) -> HxWx3 uint8 frame`, and a `mirror` attribute (set by the caller;
when True the frame is horizontally mirrored inside `step`). Palettes
are shared 256-stop uint8 LUTs (zen/ink/abyss/ember).

Feature smoothing is an ambient EMA: moderate attack, slow release, so
band energy swells and decays gently (no beat flash).
"""

from __future__ import annotations

import numpy as np

from .palette import palette_lut

_ATTACK = 0.35
_RELEASE = 0.06


class _Style:
    """Shared state: RNG, smoothed features, palette, mirror flag."""

    mirror = False

    def __init__(self, width: int, height: int, seed: int):
        self.width = width
        self.height = height
        self.seed = int(seed)
        self.rng = np.random.default_rng(self.seed)
        self.smooth = np.zeros(3)
        self.lut = palette_lut("zen")

    def set_palette(self, lut: np.ndarray) -> None:
        self.lut = lut

    def _smooth_features(self, features) -> np.ndarray:
        target = np.asarray(features, dtype=np.float64).reshape(-1)
        if target.shape != (3,):
            raise ValueError(f"features must have 3 bands, got {target.shape}")
        alpha = _ATTACK if target.sum() > self.smooth.sum() else _RELEASE
        self.smooth = alpha * target + (1.0 - alpha) * self.smooth
        return self.smooth

    def _finish(self, field01: np.ndarray) -> np.ndarray:
        if self.mirror:
            field01 = field01[:, ::-1]
        idx = np.clip(field01 * 255.0, 0, 255).astype(np.uint8)
        return self.lut[idx]

    def step(self, features, t: float) -> np.ndarray:  # pragma: no cover
        raise NotImplementedError


class FlowField(_Style):
    """Particles advected by a smooth sine-noise angle field, leaving
    trails on a fading canvas. Band energies drive speed, trail length
    and deposit brightness (plan Step 1.4: 4000 particles, decay 0.93).
    """

    PARTICLES = 4000
    DECAY = 0.93

    def __init__(self, width, height, seed):
        super().__init__(width, height, seed)
        self.pos = self.rng.random((self.PARTICLES, 2)) * [width, height]
        self.canvas = np.zeros((height, width), dtype=np.float64)

    def step(self, features, t: float) -> np.ndarray:
        bass, mid, treble = self._smooth_features(features)
        w, h = self.width, self.height

        self.canvas *= self.DECAY - 0.05 * bass

        speed = 0.6 + 2.2 * bass + 0.8 * mid
        xn = self.pos[:, 0] / w
        yn = self.pos[:, 1] / h
        ang = (
            np.sin(xn * 6.0 + 0.15 * t + yn * 3.0)
            + 0.6 * np.sin(yn * 9.0 - 0.35 * t + xn * 2.0)
            + 0.4 * np.sin((xn + yn) * 14.0 + 0.6 * t)
        ) * np.pi
        self.pos[:, 0] = (self.pos[:, 0] + np.cos(ang) * speed) % w
        self.pos[:, 1] = (self.pos[:, 1] + np.sin(ang) * speed) % h

        # Sparse stochastic respawn keeps coverage even without pops.
        respawn = self.rng.random(self.PARTICLES) > (0.9995 - 0.0015 * treble)
        n = int(respawn.sum())
        if n:
            self.pos[respawn] = self.rng.random((n, 2)) * [w, h]

        # Deposit: 3x3 splat (particle trails must survive 0.93 decay on
        # a 2M-pixel canvas), brightness follows band energy.
        gain = 0.12 + 1.8 * bass + 0.7 * mid + 1.6 * treble
        xi = self.pos[:, 0].astype(np.int32)
        yi = self.pos[:, 1].astype(np.int32)
        for oy in (-1, 0, 1):
            for ox in (-1, 0, 1):
                wgt = 1.0 if ox == 0 and oy == 0 else 0.45
                np.add.at(
                    self.canvas,
                    (np.clip(yi + oy, 0, h - 1), np.clip(xi + ox, 0, w - 1)),
                    gain * wgt,
                )

        field = np.tanh(self.canvas * (1.5 + 2.5 * treble))
        return self._finish(field)


class GrayScott(_Style):
    """Reaction-diffusion on a 480x270 torus, 2 steps/frame (plan
    Step 1.4: F=0.055 k=0.062); the render loop upscales the tile.
    Bass scales pattern contrast; mid/treble nudge feed/kill within the
    safe band around the mitosis regime.
    """

    SIM_W, SIM_H = 480, 270
    F, K = 0.055, 0.062
    DU, DV = 0.2097, 0.105

    def __init__(self, width, height, seed):
        super().__init__(width, height, seed)
        # Sim at the requested size, but coarser than 2x the plan grid
        # buys nothing (patterns live at ~10px pitch); cap for speed.
        sh = max(32, min(height, self.SIM_H * 2))
        sw = max(1, min(width, self.SIM_W * 2))
        self.U = np.ones((sh, sw), dtype=np.float64)
        self.V = np.zeros((sh, sw), dtype=np.float64)
        rng = np.random.default_rng(self.seed + 1)
        for _ in range(120):
            cy, cx = int(rng.integers(6, sh - 6)), int(rng.integers(6, sw - 6))
            r = int(rng.integers(3, 7))
            yy, xx = np.ogrid[-r : r + 1, -r : r + 1]
            mask = (yy * yy + xx * xx) <= r * r
            self.V[cy - r : cy + r + 1, cx - r : cx + r + 1][mask] = 1.0
            self.U[cy - r : cy + r + 1, cx - r : cx + r + 1][mask] = 0.0

    def _lap(self, a: np.ndarray) -> np.ndarray:
        return (
            np.roll(a, 1, 0) + np.roll(a, -1, 0) + np.roll(a, 1, 1) + np.roll(a, -1, 1)
            - 4.0 * a
        )

    def step(self, features, t: float) -> np.ndarray:
        del t  # the simulation itself provides temporal evolution
        bass, mid, treble = self._smooth_features(features)

        # One-sided drift: band energy stays near 0 on calm tracks, so
        # centering at 0.5 would push k into the pattern-death zone.
        F = self.F + 0.003 * mid
        K = self.K - 0.004 * treble
        for _ in range(2):
            uvv = self.U * self.V * self.V
            Lu = self._lap(self.U)
            Lv = self._lap(self.V)
            self.U += self.DU * Lu - uvv + F * (1.0 - self.U)
            self.V += self.DV * Lv + uvv - (K + F) * self.V
            np.clip(self.V, 0.0, 1.0, out=self.V)
            np.clip(self.U, 0.0, 1.0, out=self.U)

        # V tops out around 0.3-0.4 in Gray-Scott; scale into palette range.
        field = np.tanh(self.V * (2.5 + 4.0 * bass))
        return self._finish(field)


class Plasma(_Style):
    """Domain-warped fbm value-noise clouds (plan Step 1.4: 4 octaves,
    warp 3 + 2*treble). Noise grids are prebuilt once from the seed;
    time only shifts sample offsets, so frames are reproducible.
    Computed at half resolution (clouds tolerate it) then upscaled.
    """

    OCTAVES = 4
    BASE_FREQ = {0: 3, 1: 4, 2: 5}  # channel -> base grid frequency
    AMP_SUM = 1.875  # 1 + 1/2 + 1/4 + 1/8

    def __init__(self, width, height, seed):
        super().__init__(width, height, seed)
        self.grids = {}
        for channel, base_f in self.BASE_FREQ.items():
            for octave in range(self.OCTAVES):
                f = base_f * (2**octave)
                rng = np.random.default_rng(self.seed * 1009 + channel * 97 + octave)
                self.grids[(channel, octave)] = (rng.random((f, f)), f)
        ys = np.linspace(0.0, 1.0, self.height // 2, endpoint=False)
        xs = np.linspace(0.0, 1.0, self.width // 2, endpoint=False)
        self.mesh_x, self.mesh_y = np.meshgrid(xs, ys)

    def _sample(self, grid: np.ndarray, f: int, gx: np.ndarray, gy: np.ndarray) -> np.ndarray:
        """Bilinear smoothstep lookup on a truly toroidal fxf grid:
        the wrapped edge column/row equals column/row 0, so tiles join
        seamlessly (no rectangular seams)."""
        x0 = np.floor(gx).astype(np.int64) % f
        y0 = np.floor(gy).astype(np.int64) % f
        tx = gx - np.floor(gx)
        ty = gy - np.floor(gy)
        tx = tx * tx * (3.0 - 2.0 * tx)
        ty = ty * ty * (3.0 - 2.0 * ty)
        g00 = grid[y0, x0]
        g01 = grid[y0, (x0 + 1) % f]
        g10 = grid[(y0 + 1) % f, x0]
        g11 = grid[(y0 + 1) % f, (x0 + 1) % f]
        top = g00 + (g01 - g00) * tx
        bot = g10 + (g11 - g10) * tx
        return top + (bot - top) * ty

    def _fbm(self, channel: int, gx: np.ndarray, gy: np.ndarray, t: float) -> np.ndarray:
        acc = np.zeros_like(gx)
        amp = 1.0
        for octave in range(self.OCTAVES):
            grid, f = self.grids[(channel, octave)]
            drift = t * (0.10 + 0.04 * octave)
            ox = drift * np.cos(0.7 * octave + 1.3)
            oy = drift * np.sin(0.9 * octave + 0.4)
            acc += amp * self._sample(grid, f, gx * f + ox, gy * f + oy)
            amp *= 0.5
        return acc

    def step(self, features, t: float) -> np.ndarray:
        bass, _mid, treble = self._smooth_features(features)
        gx = self.mesh_x
        gy = self.mesh_y
        warp = 0.06 * (3.0 + 2.0 * treble)
        w1 = self._fbm(1, gx, gy, t) - self.AMP_SUM / 2.0
        w2 = self._fbm(2, gx, gy, t) - self.AMP_SUM / 2.0
        field = self._fbm(0, gx + warp * w1, gy + warp * w2, t) / self.AMP_SUM
        field = np.clip(field, 0.0, 1.0) ** (1.0 - 0.5 * bass)

        # Half-res -> full res: nearest doubling plus a small box blur.
        up = np.repeat(np.repeat(field, 2, axis=0), 2, axis=1)[: self.height, : self.width]
        up = (
            0.25 * np.roll(up, 1, 0) + 0.5 * up + 0.25 * np.roll(up, -1, 0)
            + 0.25 * np.roll(up, 1, 1) + 0.25 * np.roll(up, -1, 1)
        ) / 2.0
        return self._finish(np.clip(up, 0.0, 1.0))


STYLES = {"flow": FlowField, "bloom": GrayScott, "plasma": Plasma}
