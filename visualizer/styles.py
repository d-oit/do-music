"""The five generative styles. numpy only, fully deterministic per seed.

Every style implements: `__init__(width, height, seed)`, `.step(features,
t) -> HxWx3 uint8 frame`, and a `mirror` attribute (set by the caller;
when True the frame is horizontally mirrored inside `step`). Palettes
are shared 256-stop uint8 LUTs (zen/ink/abyss/ember/aurora).

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
        gain = float(getattr(self, "gain", 1.0) or 1.0)
        if gain != 1.0:
            field01 = np.clip(field01 * gain, 0.0, 1.0)
        idx = np.clip(field01 * 255.0, 0, 255).astype(np.uint8)
        return self.lut[idx]

    def step(self, features, t: float) -> np.ndarray:  # pragma: no cover
        raise NotImplementedError


class FlowField(_Style):
    """Particles advected by a smooth sine-noise angle field, leaving
    trails on a fading canvas. Band energies drive speed, trail length
    and deposit brightness (4000 particles, decay 0.93).

    Sound-fit v2 (2026-08-31):
    - bass → bulk speed + decay sustain (longer trails on swells)
    - mid  → directional turbulence (angle field wobble)
    - treble → sparkle deposit + respawn burst
    Per-band normalization makes all three axes audible even on calm piano.
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

        # Beat pulse: when raw bass attacks >0.15 above smoothed, boost 1 frame (web-audio-beat-detector style)
        # Use target vs smooth delta — cheap onset without extra FFT.
        target = float(np.asarray(features)[0])
        beat = 1.0 if target > self.smooth[0] + 0.12 else 0.0

        # Bass sustains trails (decay 0.88–0.95), mid adds subtle turbulent decay.
        self.canvas *= self.DECAY - 0.05 * bass + 0.025 * mid - 0.02 * beat

        # Sound-reactive speed: bass dominates, mid crossflow, treble jitter, beat adds burst (Codrops: low→time, high→amplitude)
        speed = 0.40 + 3.4 * bass + 1.3 * mid + 0.9 * treble + 1.8 * beat
        xn = self.pos[:, 0] / w
        yn = self.pos[:, 1] / h
        # Curl-noise inspired direction field (research: curl(newpos*freq)*amp for organic flow)
        # Base sine field + curl offset from perpendicular gradient of fbm-like noise
        curl = np.sin(xn * 4.0 - yn * 4.0 + t * 0.7) * np.cos(yn * 6.0 + xn * 3.0 - t * 0.5)
        ang = (
            np.sin(xn * 6.0 + 0.14 * t + yn * 3.0 + 1.0 * mid)
            + 0.6 * np.sin(yn * 9.0 - 0.34 * t + xn * 2.0 + 1.2 * mid)
            + 0.40 * np.sin((xn + yn) * 14.0 + 0.58 * t + 2.2 * treble)
            + 0.35 * treble * np.sin(xn * 22.0 + 1.9 * t)
            + 0.9 * curl * (0.5 + 0.8 * mid + 0.6 * treble)
        ) * np.pi
        self.pos[:, 0] = (self.pos[:, 0] + np.cos(ang) * speed) % w
        self.pos[:, 1] = (self.pos[:, 1] + np.sin(ang) * speed) % h

        # Treble-driven respawn bursts — sparkle tied to transient hits + beat
        respawn = self.rng.random(self.PARTICLES) > (0.9992 - 0.0028 * treble - 0.0008 * mid - 0.004 * beat)
        n = int(respawn.sum())
        if n:
            self.pos[respawn] = self.rng.random((n, 2)) * [w, h]

        # Deposit: 3x3 splat survives 0.93 decay on 2M-pixel canvas.
        gain = 0.14 + 2.2 * bass + 1.0 * mid + 2.0 * treble + 1.2 * beat
        xi = self.pos[:, 0].astype(np.int32)
        yi = self.pos[:, 1].astype(np.int32)
        for oy in (-1, 0, 1):
            for ox in (-1, 0, 1):
                wgt = 1.0 if ox == 0 and oy == 0 else 0.38
                np.add.at(
                    self.canvas,
                    (np.clip(yi + oy, 0, h - 1), np.clip(xi + ox, 0, w - 1)),
                    gain * wgt,
                )

        field = np.tanh(self.canvas * (1.2 + 3.8 * treble + 1.0 * mid + 0.6 * bass + 0.8 * beat))
        return self._finish(field)

class GrayScott(_Style):
    """Reaction-diffusion on a 480x270 torus, 2 steps/frame
    (F=0.055 k=0.062); the render loop upscales the tile.
    Bass scales pattern contrast; mid/treble nudge feed/kill within the
    safe band around the mitosis regime.

    v2: slightly larger F/K drift so music swells visibly morph the
    spots/bands (still safe), and bass-driven contrast range widens so
    calm passages don't wash out.
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

        # Drift widened vs v1 (0.003→0.005, 0.004→0.006) so mid/treble
        # swells visibly shift spot density without killing the pattern.
        F = self.F + 0.005 * mid + 0.002 * bass
        K = self.K - 0.006 * treble - 0.002 * mid
        F = float(np.clip(F, 0.040, 0.070))
        K = float(np.clip(K, 0.050, 0.070))
        for _ in range(2):
            uvv = self.U * self.V * self.V
            Lu = self._lap(self.U)
            Lv = self._lap(self.V)
            self.U += self.DU * Lu - uvv + F * (1.0 - self.U)
            self.V += self.DV * Lv + uvv - (K + F) * self.V
            np.clip(self.V, 0.0, 1.0, out=self.V)
            np.clip(self.U, 0.0, 1.0, out=self.U)

        # Auto-reseed if pattern dies (V mean <1e-4 after long calm passages).
        # Without this a 10 m calm track kills Gray-Scott by ~150 s, leaving
        # a flat field and failing the extreme-window sound-fit check.
        if self.V.mean() < 1e-3:
            rng = np.random.default_rng(self.seed + int(self.smooth.sum() * 1e6) % 100000)
            sh, sw = self.V.shape
            for _ in range(8):
                cy, cx = int(rng.integers(6, sh - 6)), int(rng.integers(6, sw - 6))
                r = int(rng.integers(3, 5))
                yy, xx = np.ogrid[-r : r + 1, -r : r + 1]
                mask = (yy * yy + xx * xx) <= r * r
                self.V[cy - r : cy + r + 1, cx - r : cx + r + 1][mask] = 0.9
                self.U[cy - r : cy + r + 1, cx - r : cx + r + 1][mask] = 0.1

        # V tops out around 0.3-0.4 in Gray-Scott; bass widens contrast so
        # quiet ambient still reads, loud swells bloom without washing out.
        field = np.tanh(self.V * (2.2 + 5.0 * bass + 1.2 * mid))
        return self._finish(field)


class Plasma(_Style):
    """Domain-warped fbm value-noise clouds (4 octaves, warp 3+2*treble).
    Noise grids are prebuilt once from the seed; time only shifts sample
    offsets, so frames are reproducible. Computed at half resolution (clouds
    tolerate it) then upscaled.

    v2: bass drives gamma/contrast (loud swells deepen clouds), mid adds
    lateral wind drift, treble warps domain more aggressively — the three
    bands stay separable on per-band normalized input.
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
        bass, mid, treble = self._smooth_features(features)
        gx = self.mesh_x
        gy = self.mesh_y
        # v2: separate drives — treble warps, mid winds, bass gammas.
        warp = 0.06 * (3.0 + 2.4 * treble + 0.8 * mid)
        wind_x = 0.015 * mid * t
        wind_y = -0.008 * mid * t
        w1 = self._fbm(1, gx + wind_x, gy + wind_y, t) - self.AMP_SUM / 2.0
        w2 = self._fbm(2, gx + wind_x, gy + wind_y, t) - self.AMP_SUM / 2.0
        field = self._fbm(0, gx + warp * w1, gy + warp * w2, t) / self.AMP_SUM
        field = np.clip(field, 0.0, 1.0) ** (1.0 - 0.55 * bass - 0.15 * mid)
        # Brightness lift tied to overall energy so quiet intro isn't washed out.
        bright = 0.06 * bass
        field = np.clip(field + bright, 0.0, 1.0)

        # Half-res -> full res: nearest doubling plus a small box blur.
        up = np.repeat(np.repeat(field, 2, axis=0), 2, axis=1)[: self.height, : self.width]
        up = (
            0.25 * np.roll(up, 1, 0) + 0.5 * up + 0.25 * np.roll(up, -1, 0)
            + 0.25 * np.roll(up, 1, 1) + 0.25 * np.roll(up, -1, 1)
        ) / 2.0
        return self._finish(np.clip(up, 0.0, 1.0))


class Waves(_Style):
    """Band-driven undulating surfaces (spectrum-wave sheets, MuseGen
    "mids -> deformation/waves"). Layered directional sine surfaces at
    half resolution: bass lifts amplitude (tall slow swells), mid
    stretches the wavelength, treble adds a fine sparkle ripple. Time
    shifts phase only, so frames are reproducible per seed.
    """

    LAYERS = 3

    def __init__(self, width, height, seed):
        super().__init__(width, height, seed)
        rng = np.random.default_rng(seed)
        ang = rng.uniform(0.0, 2.0 * np.pi, (self.LAYERS,))
        self.dir = np.stack([np.cos(ang), np.sin(ang)], axis=1)
        self.phase = rng.uniform(0.0, 2.0 * np.pi, (self.LAYERS,))
        self.freq = rng.uniform(1.5, 4.5, (self.LAYERS,))
        ys = np.linspace(0.0, 1.0, height // 2, endpoint=False)
        xs = np.linspace(0.0, 1.0, width // 2, endpoint=False)
        self.mesh_x, self.mesh_y = np.meshgrid(xs, ys)

    def step(self, features, t: float) -> np.ndarray:
        bass, mid, treble = self._smooth_features(features)
        x, y = self.mesh_x, self.mesh_y
        field = np.zeros_like(x)
        for i in range(self.LAYERS):
            d = self.dir[i]
            lam = 1.0 + 0.7 * mid  # mid stretches the wavelength
            drift = t * (0.35 + 0.25 * mid)
            wave = np.sin(
                2.0 * np.pi * self.freq[i] / lam * (d[0] * x + d[1] * y)
                + drift
                + self.phase[i]
            )
            weight = 0.45 + 0.30 * bass + 0.20 * i / (self.LAYERS - 1)
            field += weight * wave
        # treble: fine sparkle ripple
        field += treble * 0.18 * np.sin(2.0 * np.pi * 9.0 * (x + y) + 3.2 * t)
        lo, hi = float(field.min()), float(field.max())
        rng = hi - lo
        field01 = (field - lo) / rng if rng > 1e-9 else np.zeros_like(field)
        field01 = np.clip(field01, 0.0, 1.0) ** (1.0 - 0.5 * bass)
        up = np.repeat(np.repeat(field01, 2, axis=0), 2, axis=1)[: self.height, : self.width]
        up = (
            0.25 * np.roll(up, 1, 0) + 0.5 * up + 0.25 * np.roll(up, -1, 0)
            + 0.25 * np.roll(up, 1, 1) + 0.25 * np.roll(up, -1, 1)
        ) / 2.0
        return self._finish(np.clip(up, 0.0, 1.0))


class Rings(_Style):
    """Radial spectrum pulse (MuseGen "bass -> scale/pulse"). Concentric
    rings breathe with the bass, a 4-spoke swirl rotates with mid, and
    treble sparkles on the ring edges. Polar coordinates about the frame
    center at half resolution, then upscaled.
    """

    def __init__(self, width, height, seed):
        super().__init__(width, height, seed)
        ys = np.linspace(0.0, 1.0, height // 2, endpoint=False)
        xs = np.linspace(0.0, 1.0, width // 2, endpoint=False)
        gx, gy = np.meshgrid(xs, ys)
        # aspect-correct normalized radius 0..1 and angle
        ex, ey = gx - 0.5, gy - 0.5
        self.r = np.sqrt((ex * width / height) ** 2 + ey**2)
        self.r /= float(self.r.max())
        self.ang = np.arctan2(ey, ex * width / height)
        # Seed-dependent character: initial phase and ring-spacing scale so
        # different seeds give visibly different ring layouts.
        rng = np.random.default_rng(self.seed)
        self.phase0 = float(rng.random()) * 6.283185307179586
        self.spacing_scale = 0.85 + 0.30 * float(rng.random())

    def step(self, features, t: float) -> np.ndarray:
        bass, mid, treble = self._smooth_features(features)
        spacing = (0.045 + 0.06 * bass + 0.02 * mid) * self.spacing_scale
        swirl = 0.22 * np.sin(4.0 * self.ang + 0.6 * t + 1.5 * mid + self.phase0)
        sparkle = treble * 0.15 * np.sin(2.0 * np.pi * self.r * 6.0 + 2.6 * t)
        field01 = 0.5 + 0.5 * np.sin(
            2.0 * np.pi * self.r / spacing - 1.8 * t + swirl + sparkle
        )
        field01 = np.clip(field01, 0.0, 1.0) ** (1.0 - 0.5 * bass)
        up = np.repeat(np.repeat(field01, 2, axis=0), 2, axis=1)[: self.height, : self.width]
        up = (
            0.25 * np.roll(up, 1, 0) + 0.5 * up + 0.25 * np.roll(up, -1, 0)
            + 0.25 * np.roll(up, 1, 1) + 0.25 * np.roll(up, -1, 1)
        ) / 2.0
        return self._finish(np.clip(up, 0.0, 1.0))


class Spectrum(_Style):
    """Classic bar-spectrum visualizer, modernized: frequency bars rendered
    as rounded glowing columns with a mirrored reflection and peak-cap dots
    that fall slowly (classic winamp-style peak hold). Uses the real FFT
    band matrix — bars map to log-spaced frequency bins, not just 3 bands.

    v2: driven by full-band feature matrix when available; falls back to
    3-band energies. Bass drives bar height gain, treble drives peak-cap
    snap speed, mid drives hue drift across the palette.
    """

    BARS = 48
    PEAK_FALL = 0.012

    def __init__(self, width, height, seed):
        super().__init__(width, height, seed)
        self.peaks = np.zeros(self.BARS)
        # Seed-dependent character: per-bar amplitude jitter (so different
        # seeds give visibly different bar profiles) and an initial hue phase.
        rng = np.random.default_rng(self.seed)
        self.bar_jitter = 0.85 + 0.30 * rng.random(self.BARS)
        self.phase = float(rng.random()) * 6.283185307179586

    def step(self, features, t: float) -> np.ndarray:
        f = np.asarray(features, dtype=np.float64).reshape(-1)
        bass, mid, treble = self._smooth_features(f[:3])
        # Log-spaced bar mapping over available feature bands.
        n_src = len(f)
        src = f if n_src > 3 else np.array([f[0]] * 8 + [f[1]] * 16 + [f[2]] * 24)
        n_src = len(src)
        log_pos = np.logspace(0, np.log10(n_src), self.BARS + 1) - 1
        bars = np.array([src[int(lo):max(int(lo) + 1, int(hi))].mean()
                         for lo, hi in zip(log_pos[:-1], log_pos[1:])])
        bars = np.clip(bars * (0.7 + 0.9 * bass) * self.bar_jitter, 0.0, 1.0)

        # Peak caps fall slowly (winamp-style peak hold); treble snaps faster.
        self.peaks = np.maximum(bars, self.peaks - self.PEAK_FALL * (1.0 + 2.0 * treble))

        self.phase += 0.02 + 0.05 * mid  # hue drift with mid energy
        W, H = self.width, self.height
        field = np.zeros((H, W))
        base_h = H * 0.72
        bar_w = W / self.BARS
        xs = np.arange(W)
        bar_idx = np.minimum((xs / bar_w).astype(int), self.BARS - 1)
        heights = bars[bar_idx] * base_h
        # Rounded glowing columns: vertical distance to bar top with soft edge.
        ys = np.arange(H)[:, None]
        top = (H - heights[None, :])
        core = np.clip((top - ys) / (H * 0.06) + 1.0, 0.0, 1.0)
        edge = 1.0 - np.abs(np.clip(xs[None, :] % bar_w - bar_w / 2) / (bar_w * 0.18))
        col = np.clip(core, 0, 1) * np.clip(edge, 0, 1)
        # Reflection below the bars (dimmed, faded).
        refl_y = ys - top
        refl = np.clip((top - refl_y) / (H * 0.10), 0.0, 1.0) * 0.35 * np.clip(col, 0, 1)
        field = np.maximum(col, refl)
        # Peak-cap dots.
        peak_y = (H - self.peaks[bar_idx] * base_h - H * 0.012).astype(int)
        dot = np.abs(ys - peak_y[None, :]) < H * 0.008
        field = np.maximum(field, dot * 0.95)
        # Floor glow under the bars.
        field += np.clip(1.0 - np.abs(ys - (H - 2)) / (H * 0.04), 0, 1) * (0.15 + 0.5 * bass)
        # Hue drift: rotate palette sampling with mid energy.
        shifted = (field01 := field)  # keep structure for hue drift below
        roll = int(self.phase * 255) % 256
        lut = np.roll(self.lut, roll, axis=0)
        self.lut = lut
        return self._finish(np.clip(field, 0.0, 1.0))


class Kaleido(_Style):
    """Kaleidoscope: radial symmetry (6-fold) over a bass-warped noise
    field, mid drives rotation speed, treble drives mirror pulse. Classic
    kaleidoscope construction: sample a wedge of the noise field and
    reflect it 6 ways around the center.
    """

    SEGMENTS = 6

    def __init__(self, width, height, seed):
        super().__init__(width, height, seed)
        ys = np.linspace(0.0, 1.0, height // 2, endpoint=False)
        xs = np.linspace(0.0, 1.0, width // 2, endpoint=False)
        self.gx, self.gy = np.meshgrid(xs, ys)
        rng = np.random.default_rng(self.seed)
        # Noise grids for the wedge source (toroidal, like Plasma).
        self.grids = []
        for octave, f in enumerate((3, 6, 12)):
            rng = np.random.default_rng(self.seed * 31 + octave * 7)
            self.grids.append((rng.random((f, f)), f))

    def _sample(self, grid, f, gx, gy):
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

    def step(self, features, t: float) -> np.ndarray:
        bass, mid, treble = self._smooth_features(features)
        x, y = self.gx - 0.5, self.gy - 0.5
        # Polar wedge coordinates; rotation speed driven by mid.
        rot = t * (0.15 + 0.5 * mid)
        r = np.sqrt(x * x + y * y)
        ang = np.arctan2(y, x) + rot
        # Bass warps the radius (breathing mandala).
        r_warp = r * (1.0 - 0.25 * bass * np.sin(2.0 * np.pi * r * 3.0 - 1.4 * t))
        # Sample the noise field in wedge space.
        gx = (r_warp * np.cos(ang) + 0.5) * 3.0
        gy = (r_warp * np.sin(ang) + 0.5) * 3.0
        field = np.zeros_like(r)
        amp = 1.0
        for grid, f in self.grids:
            field += amp * self._sample(grid, f, gx * f + t * 0.05, gy * f - t * 0.04)
            amp *= 0.5
        field /= 1.75
        # Fold into 6-fold symmetry: wedge angle maps to mirrored sectors.
        seg = np.pi / (self.SEGMENTS / 2.0)
        wedge = np.abs(((ang + rot) % (2.0 * seg)) - seg) / seg
        field01 = np.clip(field * (0.55 + 0.75 * wedge), 0.0, 1.0)
        # Treble pulse: brighten ring edges.
        field01 = np.clip(field01 + treble * 0.12 * np.sin(2.0 * np.pi * r * 5.0 - 2.2 * t), 0.0, 1.0)
        field01 = field01 ** (1.0 - 0.4 * bass)
        up = np.repeat(np.repeat(field01, 2, axis=0), 2, axis=1)[: self.height, : self.width]
        up = (
            0.25 * np.roll(up, 1, 0) + 0.5 * up + 0.25 * np.roll(up, -1, 0)
            + 0.25 * np.roll(up, 1, 1) + 0.25 * np.roll(up, -1, 1)
        ) / 2.0
        return self._finish(np.clip(up, 0.0, 1.0))


STYLES = {
    "flow": FlowField,
    "bloom": GrayScott,
    "plasma": Plasma,
    "waves": Waves,
    "rings": Rings,
    "spectrum": Spectrum,
    "kaleido": Kaleido,
}