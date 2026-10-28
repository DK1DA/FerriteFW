#!/usr/bin/env python3
"""TERM-33.16 (decisions 308, 310, 311): the TX pre-equalisers' design, magnitude and phase, from the identified
bypass channel (black radio, Oct 27, 2026: REG_47<0> = 1, "bypass all AF TX filters", and the TX soft limiter off).

    txeq_design.py                      # design both, print the checks, the headroom and the predicted eyes
    txeq_design.py --rust               # also print the Rust statics (crates/ferrite-core/src/txeq.rs)
    txeq_design.py --h CAP_H.npy        # also predict the eyes through a measured channel (txident.py --save-h)

The channel (tools/bench-blade/txident.py on the low-power captures, Oct 27): the DAC's injection network is a
first-order low-pass at FP (minimum phase: its phase falls to -90 degrees above a few hundred Hz, so a magnitude-
only equaliser leaves the signal's Hilbert transform: what wrecked 9600 Bd and smeared 4800), the board's coupling
a high-pass near FH, and the chip's digital filter linear phase (R below: flat to 3 kHz, -2.4 dB at 5 kHz, -11.8 at
8 kHz, measured phase within +-6 degrees). The equaliser is one FIR (not symmetric) designed by complex weighted
least squares so that equaliser x channel = a pure delay x the band's flat target: the pole's phase is undone
with its magnitude, R's magnitude too; the coupling high-pass is left (its group delay is under 0.15 ms above
200 Hz, where these signals have their energy).

Level (decision 310, unchanged): every tone at the deviation the plain `txtest sweep data` 1 kHz tone gets; the
bypass loses 6.8 dB against it at 1 kHz; REG_7D to its top (+5.5 dB), `eq8` also REG_40 0x57A (+2 dB by the
power law the tone generator follows), the taps the rest.

Predicted eyes: each test signal (as txtest makes it, tools/bench-blade/txident.py's gen) through the equaliser and
the channel, white noise for 55 dB SNR in 0-24 kHz, then txdata.py's slicer (BER, eye).
Only numpy.
"""
import os, sys
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, ".."))
import txident  # noqa: E402
import txdata  # noqa: E402

FS = 48_000
# The bypass sweep, the soft limiter off (third octaves, dB against its own 1 kHz tone; sdrtx.py's table, Oct 27).
MEAS = np.array([
    (50, 14.5), (63, 14.6), (80, 14.6), (100, 14.4), (125, 13.9), (160, 12.9), (200, 11.8), (250, 10.5), (315, 8.7),
    (400, 7.0), (500, 5.4), (630, 3.7), (800, 1.8), (1000, 0.0), (1250, -1.9), (1600, -4.0), (2000, -6.1),
    (2500, -8.2), (3150, -10.6), (4000, -13.4), (5000, -16.6), (6300, -21.4), (8000, -30.2)])
# The identified channel (txident.py, the broadband bypass captures lp_g3ruheq8 and lp_f9600eq8): the injection's
# pole, the coupling's high-pass.
FP = 168.0
FH = 35.0
# The bypass's loss at 1 kHz against the plain `data` sweep tone: 1385 against 3046 Hz at the same DAC level.
BYPASS_LOSS_DB = 20 * np.log10(3046 / 1385)
# REG_40's low 12 bits against the tone generator's deviation (wide, Hz; analysis/audio_path.md, Oct 13, 2026).
REG40_PTS = np.array([(0x300, 1027), (0x387, 1574), (0x3E0, 1934), (0x43C, 2551), (0x4A0, 3344), (0x50A, 4290)])
MIC_DATA = 0x34       # REG_7D[5:0] of a data key-up (crates/ferrite-core/src/vox.rs); 0.5 dB a step, 0x3F the top
DEV_KEYUP = 0x50A     # REG_40[11:0] every key-up writes (drivers/bk4829/src/tx.rs TX_DEVIATION)
SWEEP_AMP = 4000
DAC_FULL = 32767
SNR_DB = 55.0


class Spec:
    def __init__(self, name, top, roll_end, taps, r40_db):
        self.name, self.top, self.roll_end, self.taps, self.r40_db = name, top, roll_end, taps, r40_db


SPECS = [Spec("eq", 5000, 7000, 95, 0.0), Spec("eq8", 8000, 10000, 127, 2.0)]


def analog(f):
    """The injection's pole and the coupling's high-pass, unit gain at 1 kHz in magnitude (complex)."""
    s = 1j * np.asarray(f, float)
    h = 1 / (1 + s / FP) * (s / FH) / (1 + s / FH)
    s1 = 1j * 1000.0
    return h / abs(1 / (1 + s1 / FP) * (s1 / FH) / (1 + s1 / FH))


def r_mag_db(f):
    """The chip's linear-phase part, dB: the measured sweep less the analogue model, smoothed (a quintic in log f),
    0 below 1 kHz (where the model alone matches the sweep within 1 dB), held at its 8 kHz value above (the
    remainder measured -35 dB at 9 kHz, but nothing is known further up: the equaliser itself must fall there, so
    the DAC's room isn't spent on what the chip may or may not filter)."""
    m = MEAS[MEAS[:, 0] >= 1000]
    r = m[:, 1] - 20 * np.log10(np.abs(analog(m[:, 0])))
    c = np.polyfit(np.log2(m[:, 0]), r - r[0], 3)
    f = np.asarray(f, float)
    out = np.where(f <= 1000, 0.0, np.polyval(c, np.log2(np.clip(f, 1000, 8000))))
    hi = f > 8000
    out[hi] = np.polyval(c, np.log2(8000))
    return out


def channel(f):
    """The bypass channel's model, unit at 1 kHz: analogue x R."""
    return analog(f) * 10 ** (r_mag_db(f) / 20)


def target_mag(f, s):
    """Flat to the band's top, a raised-cosine fall (in amplitude) to the roll-off's end, then nothing."""
    f = np.asarray(f, float)
    a = np.ones_like(f)
    x = np.clip((f - s.top) / (s.roll_end - s.top), 0, 1)
    a = np.where(f > s.top, (1 + np.cos(np.pi * x)) / 2, a)
    return a


def design(s, delay=None):
    """The FIR (s.taps, any shape) minimising sum W |G H - T|^2, T = target x e^{-j w D} x the coupling's
    high-pass (left in place), over 0-24 kHz; the delay D searched when not given. Returns (taps, D)."""
    n = s.taps
    f = np.linspace(0, FS / 2, 4000)
    H = channel(f)
    s1 = 1j * f
    hp = (s1 / FH) / (1 + s1 / FH)
    hp /= abs((1j * 1000 / FH) / (1 + 1j * 1000 / FH))
    A = target_mag(f, s) * hp
    W = np.ones_like(f)
    W[(f > s.top) & (f < s.roll_end)] = 0.05
    W[f >= s.roll_end] = 3.0
    W[f < 60] = 0.2
    k = np.arange(n)
    E = np.exp(-2j * np.pi * np.outer(f, k) / FS) * H[:, None]

    def solve(D):
        T = A * np.exp(-2j * np.pi * f * D / FS)
        M = np.vstack([(E * W[:, None]).real, (E * W[:, None]).imag])
        y = np.concatenate([(T * W).real, (T * W).imag])
        g = np.linalg.lstsq(M, y, rcond=None)[0]
        err = np.sum(W * np.abs(E @ g - T) ** 2)
        return g, err

    if delay is None:
        best = min((solve(D)[1], D) for D in range(n // 4, 3 * n // 4 + 1, 2))
        delay = best[1]
    return solve(delay)[0], delay


def response(g, f):
    return np.exp(-2j * np.pi * np.outer(np.atleast_1d(f), np.arange(len(g))) / FS) @ g


def quantise(h):
    shift = 15
    while np.max(np.abs(h)) * (1 << shift) > 32767:
        shift -= 1
    return np.round(h * (1 << shift)).astype(int), shift


def reg40_for(db):
    p = np.polyfit(np.log(REG40_PTS[:, 0]), np.log(REG40_PTS[:, 1]), 1)[0]
    v = int(round(DEV_KEYUP * 10 ** (db / 20 / p)))
    return v, 20 * p * np.log10(v / DEV_KEYUP)


SIGNALS = [("fsk", 1200, False), ("fsk", 2400, False), ("fsk", 4800, True), ("gmsk", 4800, False),
           ("fsk", 9600, True), ("gmsk", 9600, False), ("g3ruh", 9600, True)]


def through(x, H_of_f):
    n = len(x)
    fx = np.fft.rfftfreq(n, 1 / FS)
    return np.fft.irfft(np.fft.rfft(x) * H_of_f(fx), n)


def eye(kind, baud, rc, g, H_of_f, seconds=4, rng=None, level=100):
    """BER and eye (txdata.py's slicer) of txtest's signal at `level` through g (None: no equaliser), the DAC's
    clipping, and the channel."""
    x = txident.gen(kind, baud, rc, int(FS * seconds), level)
    if g is not None:
        x = np.clip(np.convolve(x, g)[: len(x)], -DAC_FULL, DAC_FULL)
    y = through(x, H_of_f)
    y = y / np.std(y) * 2000
    y += (rng or np.random.default_rng(1)).normal(0, 2000 * 10 ** (-SNR_DB / 20), len(y))
    spb = FS / (9600 if kind == "g3ruh" else baud)
    return txdata.analyse(txdata.mavg(y, spb), spb, kind == "g3ruh", "", quiet=True)


def label(kind, baud, rc):
    return "g3ruh" if kind == "g3ruh" else f"{kind} {baud}{' rc' if rc else ''}"


def main():
    rust = "--rust" in sys.argv
    hpath = sys.argv[sys.argv.index("--h") + 1] if "--h" in sys.argv else None
    pts, meas = MEAS[:, 0], MEAS[:, 1]
    mod = 20 * np.log10(np.abs(channel(pts)))
    print(f"channel model (pole {FP:g} Hz, high-pass {FH:g} Hz, R): within {np.max(np.abs(mod - meas)[pts >= 63]):.1f} dB of the sweep from 63 Hz")
    gd = lambda H, f: -np.gradient(np.unwrap(np.angle(H)), 2 * np.pi * f)
    fg = np.linspace(20, 9000, 4000)
    g0 = gd(channel(fg), fg)
    print("channel group delay, us against 1 kHz: " + " ".join(
        f"{int(c)}:{(np.interp(c, fg, g0) - np.interp(1000, fg, g0)) * 1e6:+.0f}" for c in [50, 100, 200, 400, 1000, 2000, 5000, 8000]))
    meas_h = None
    if hpath:
        h = np.load(hpath)
        c = len(h) // 2
        def meas_h(fx, h=h, c=c):
            Hm = np.exp(-2j * np.pi * np.outer(fx, np.arange(len(h)) - c) / FS) @ h
            return Hm / abs(np.interp(1000, fx, np.abs(Hm)))
    out = []
    for s in SPECS:
        g, D = design(s)
        tot = response(g, pts) * channel(pts)
        g1k = abs(response(g, [1000.0])[0] * channel([1000.0])[0])
        g = g / g1k  # the equaliser x channel at 1 kHz: 1
        tot = tot / g1k
        band = (pts >= 100) & (pts <= s.top)
        mag = 20 * np.log10(np.abs(tot))
        ft = np.linspace(100, s.top, 2000)
        gt = gd(response(g, ft) * channel(ft), ft)
        print(f"\n== {s.name}: flat 100 Hz-{s.top / 1000:g} kHz, {s.taps} taps (any shape), delay {D} samples ({D / FS * 1e3:.2f} ms)")
        print(f"  predicted, each tone against the plain data 1 kHz tone (model): {mag[band].min():+.2f} to {mag[band].max():+.2f} dB;"
              f" group delay 100 Hz-{s.top / 1000:g} kHz within {(gt.max() - gt.min()) * 1e6:.0f} us"
              f" (200 Hz up: {(gt[ft >= 200].max() - gt[ft >= 200].min()) * 1e6:.0f} us)")
        print("   " + " ".join(f"{int(f)}:{v:+.1f}" for f, v in zip(pts[band], mag[band])))
        above = np.array([6000, 7000, 8000, 9000, 10000, 12000, 16000, 20000.0])
        print("  above:", " ".join(f"{f / 1000:g}k:{20 * np.log10(abs(v)):+.0f}" for f, v in zip(above, response(g, above) * channel(above))))
        # Level: the stages, the taps the rest.
        steps = 0x3F - MIC_DATA
        r40, r40_db = reg40_for(s.r40_db) if s.r40_db else (DEV_KEYUP, 0.0)
        d1 = BYPASS_LOSS_DB - steps * 0.5 - r40_db
        gl = g * 10 ** (d1 / 20)
        print(f"  gain stages: REG_7D 0x3F ({steps * 0.5:+.1f} dB), REG_40 0x{r40:03X} ({r40_db:+.1f} dB); taps {20 * np.log10(abs(response(gl, [1000.0])[0])):+.2f} dB at 1 kHz")
        # Headroom at levels 100 and 150, and the level each signal fits the DAC up to.
        rows = [("sweep", None, SWEEP_AMP * np.max(np.abs(response(gl, pts.astype(float)))))]
        for kind, baud, rc in SIGNALS:
            x = txident.gen(kind, baud, rc, FS * 2)
            rows.append((label(kind, baud, rc), (kind, baud, rc), np.max(np.abs(np.convolve(x, gl)))))
        print(f"  DAC peaks at level 100 (full scale {DAC_FULL}), the level each fits up to, and the predicted eye there")
        print(f"  (txdata's slicer, {SNR_DB:g} dB SNR: BER, eye, opening; through the model channel" + ("; | the measured one)" if meas_h else ")"))
        for n_, sig, pk in rows:
            fit = int(min(150, 100 * DAC_FULL / pk))
            line = f"    {n_:12s} {pk:7.0f}  fits to {fit:3d} %"
            if sig:
                lv = min(100, fit)
                r = eye(*sig, gl, lambda fx: channel(fx), level=lv)
                line += f"   at {lv:3d} %: {r['ber'] * 100:5.2f} % {r['eye']:4.1f} {r['opening'] * 100:3.0f} %"
                if meas_h:
                    r2 = eye(*sig, gl, meas_h, level=lv)
                    line += f"  | {r2['ber'] * 100:5.2f} % {r2['eye']:4.1f} {r2['opening'] * 100:3.0f} %"
            print(line)
        q, shift = quantise(gl)
        out.append((s, q, shift, steps, r40, 20 * np.log10(abs(response(gl, [1000.0])[0])), D))
    if rust:
        for s, q, shift, steps, r40, g1, D in out:
            n = s.name.upper()
            print(f"\npub static {n}_TAPS: [i16; {s.taps}] = [")
            for i in range(0, s.taps, 16):
                print("    " + ", ".join(str(v) for v in q[i:i + 16]) + ",")
            print("];")
            print(f"// {n}: Q{shift}, REG_7D +{steps}, REG_40 0x{r40:03X}, taps {g1:+.2f} dB at 1 kHz, delay {D}")


if __name__ == "__main__":
    main()
