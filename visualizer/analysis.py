"""Deterministic audio analysis: band energies from WAV PCM via numpy FFT.

The Rust side materializes the audio as a 22050 Hz mono WAV first
(ffmpeg), so this module only ever sees PCM floats — no mp3/ogg decode
dependencies.
"""

from __future__ import annotations

import numpy as np

# Analysis hop size (samples at 22050 Hz) — 512 samples ≈ 23.2 ms.
_HOP = 512

# FFT window size; 1024 samples ≈ 46.4 ms resolution at 22050 Hz.
_WINDOW = 1024

# Band edges in Hz, matching GMI/MusicSpec band language.
BANDS = ("bass", "mid", "treble")
BAND_EDGES = {"bass": (20.0, 250.0), "mid": (250.0, 2000.0), "treble": (2000.0, 11025.0)}


def load_wav_pcm(path: str) -> tuple[np.ndarray, int]:
    """Read a PCM WAV file as float64 in [-1, 1]; returns (samples, rate).

    Walks RIFF chunks (ffmpeg emits a LIST/INFO chunk between fmt and
    data), so metadata never shifts the data offset.
    """
    with open(path, "rb") as f:
        riff = f.read(12)
    if riff[:4] != b"RIFF" or riff[8:12] != b"WAVE":
        raise ValueError(f"{path}: not a RIFF/WAVE file")

    import struct

    audio_format = channels = rate = bits = None
    raw = None
    with open(path, "rb") as f:
        f.seek(12)
        while True:
            head = f.read(8)
            if len(head) < 8:
                break
            chunk_id, size = head[:4], int.from_bytes(head[4:8], "little")
            if chunk_id == b"fmt ":
                fmt = f.read(size)
                audio_format, channels, rate, _byte_rate, _align, bits = struct.unpack(
                    "<HHIIHH", fmt[:16]
                )
            elif chunk_id == b"data":
                raw = f.read(size)
            else:  # LIST, fact, ...
                f.seek(size, 1)
            if size % 2:
                f.seek(1, 1)  # chunks are word-aligned

    if audio_format != 1:  # PCM
        raise WAVNotPCMError(
            f"{path}: only PCM WAV is supported, got format {audio_format}"
        )
    if bits not in (16, 32):
        raise WAVNotPCMError(f"{path}: {bits}-bit WAV not supported (need 16 or 32)")
    if raw is None:
        raise WAVNotPCMError(f"{path}: no data chunk found")

    dtype = np.dtype("<i2") if bits == 16 else np.dtype("<f4")
    samples = np.frombuffer(raw, dtype=dtype).astype(np.float64)
    if bits == 16:
        samples /= 32768.0
    if channels == 2:
        samples = samples.reshape(-1, 2).mean(axis=1)
    return samples, rate


class WAVNotPCMError(ValueError):
    pass




def frame_features(path: str) -> np.ndarray:
    """Per-frame (bass, mid, treble) energies, shape (n_frames, 3).

    Frames advance `_HOP` samples; energies are mean-square magnitudes of
    FFT bins in each band. Normalized so the loudest band peak reaches
    1.0, then sqrt-compressed so quiet passages still carry usable
    dynamic range (a calm meditation track sits far below its loudest
    peak; linear scaling would starve the visual engine).
    """
    samples, rate = load_wav_pcm(path)
    if len(samples) < _WINDOW:
        raise ValueError(f"{path}: audio too short ({len(samples)} samples)")

    frames = 1 + (len(samples) - _WINDOW) // _HOP
    idx = np.arange(_WINDOW)[None, :] + _HOP * np.arange(frames)[:, None]
    windows = samples[idx] * np.hanning(_WINDOW)
    spectrum = np.abs(np.fft.rfft(windows, axis=1)) ** 2
    freqs = np.fft.rfftfreq(_WINDOW, 1.0 / rate)

    out = np.empty((frames, 3))
    for i, lo, hi in ((0, 20.0, 250.0), (1, 250.0, 2000.0), (2, 2000.0, 11025.0)):
        mask = (freqs >= lo) & (freqs < hi)
        out[:, i] = spectrum[:, mask].mean(axis=1)

    peak = out.max()
    if peak <= 0:
        return np.zeros_like(out)
    return np.sqrt(out / peak)


def extreme_windows(features: np.ndarray, seconds: float, hop_rate: float) -> tuple[float, float]:
    """Timestamps (seconds) of the highest-bass and lowest-bass 5 s windows.

    `hop_rate` is frames per second of the feature matrix (rate/_HOP).
    """
    span = max(1, int(round(seconds * hop_rate)))
    if span >= len(features):
        return 0.0, 0.0
    bass = features[:, 0]
    sums = np.convolve(bass, np.ones(span), mode="valid")
    return float(np.argmax(sums) / hop_rate), float(np.argmin(sums) / hop_rate)
