#!/usr/bin/env python3
"""Synthesize the tap sound: a hard, smacking plastic hit, between a clap and
an open snare.

Each preset layers a sharp impact, band-limited noise bursts with a tail
(the smack, clap or snare wires), short plastic resonances, and a low body.

    scripts/make-tap.py [PRESET] [OUT.wav]

Defaults to the `smack` preset written to assets/tap_plastic.wav.
"""
import math
import os
import sys
import wave

import numpy as np

RATE = 44100
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

PRESETS = {
    # Hard plastic slap: sharp, mid-heavy noise and a woody plastic ring, kept quick.
    "smack": dict(
        bursts=[(0.0, 1.0)],
        noise=dict(kind="bandpass", freq=1900, q=0.7, decay=0.03, gain=1.0),
        modes=[(880, 0.55, 0.018), (1530, 0.4, 0.0128), (2650, 0.25, 0.008)],
        body=dict(start=240, end=190, decay=0.012, gain=0.6),
        impact=0.6,
        seconds=0.17,
    ),
    # Clap: a few quick bursts, then a longer diffuse tail.
    "clap": dict(
        bursts=[(0.0, 0.8), (0.008, 0.9), (0.017, 0.75), (0.026, 1.0)],
        noise=dict(kind="bandpass", freq=1250, q=0.9, decay=0.12, gain=1.0),
        modes=[(1100, 0.25, 0.03), (2300, 0.15, 0.02)],
        body=dict(start=210, end=180, decay=0.02, gain=0.3),
        impact=0.4,
        seconds=0.4,
    ),
    # Open snare: pitched drum body plus bright snare-wire noise that rings on.
    "snare": dict(
        bursts=[(0.0, 1.0)],
        noise=dict(kind="highpass", freq=1500, q=0.7, decay=0.18, gain=0.9),
        modes=[(330, 0.35, 0.05), (1050, 0.25, 0.03)],
        body=dict(start=225, end=180, decay=0.06, gain=1.0),
        impact=0.5,
        seconds=0.45,
    ),
}


def biquad(x, kind, freq, q):
    """RBJ cookbook band-pass (constant peak gain) or high-pass filter."""
    w = 2 * math.pi * freq / RATE
    alpha = math.sin(w) / (2 * q)
    cos = math.cos(w)
    if kind == "bandpass":
        b = (alpha, 0.0, -alpha)
    else:
        b = ((1 + cos) / 2, -(1 + cos), (1 + cos) / 2)
    a0, a1, a2 = 1 + alpha, -2 * cos, 1 - alpha
    b0, b1, b2 = (c / a0 for c in b)
    a1, a2 = a1 / a0, a2 / a0
    y = np.zeros_like(x)
    x1 = x2 = y1 = y2 = 0.0
    for i, xi in enumerate(x):
        yi = b0 * xi + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2
        x2, x1, y2, y1 = x1, xi, y1, yi
        y[i] = yi
    return y


def hit(bursts, noise, modes, body, impact, seconds, seed=11):
    x = np.arange(int(RATE * seconds)) / RATE
    rng = np.random.default_rng(seed)
    onset = np.minimum(1, x / 0.0002)

    # Contact click: differentiated noise is a crude high-pass.
    out = impact * np.diff(rng.standard_normal(len(x)), prepend=0) * np.exp(-x / 0.0008)

    # Noise bursts; the last one carries the long tail.
    envelope = np.zeros_like(x)
    for i, (start, amp) in enumerate(bursts):
        dt = np.clip(x - start, 0, None)
        decay = noise["decay"] if i == len(bursts) - 1 else 0.004
        envelope += np.where(x >= start, amp * np.exp(-dt / decay) * np.minimum(1, dt / 0.0003), 0)
    filtered = biquad(rng.standard_normal(len(x)), noise["kind"], noise["freq"], noise["q"])
    out += noise["gain"] * filtered / (np.max(np.abs(filtered)) or 1) * 3 * envelope

    for freq, amp, decay in modes:
        out += amp * np.sin(2 * np.pi * freq * x + rng.uniform(0, 2 * np.pi)) * np.exp(-x / decay) * onset

    # Body: a quick downward pitch sweep, like a drum head settling.
    f = body["end"] + (body["start"] - body["end"]) * np.exp(-x / 0.015)
    out += body["gain"] * np.sin(2 * np.pi * np.cumsum(f) / RATE) * np.exp(-x / body["decay"]) * onset
    return out


def save(path, samples, gain=0.85):
    samples = samples / (np.max(np.abs(samples)) or 1) * gain
    fade = int(RATE * 0.01)
    samples[-fade:] *= np.linspace(1, 0, fade)
    with wave.open(path, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes((samples * 32767).astype(np.int16).tobytes())


def main():
    preset = sys.argv[1] if len(sys.argv) > 1 else "smack"
    out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(ROOT, "assets", "tap_plastic.wav")
    if preset not in PRESETS:
        sys.exit(f"unknown preset {preset!r}; choose from {', '.join(PRESETS)}")
    save(out, hit(**PRESETS[preset]))
    print("wrote", out)


if __name__ == "__main__":
    main()
