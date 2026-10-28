#!/usr/bin/env python3
"""TERM-33.16 (decisions 308, 310): the TX pre-equalisers' design, from the bypass sweep (black radio, Oct 27, 2026:
REG_47<0> = 1, "bypass all AF TX filters", and the TX soft limiter off, REG_2F<7:5> = 000).

    txeq_design.py            # design both, print the checks and the report
    txeq_design.py --rust     # also print the Rust statics (crates/ferrite-core/src/txeq.rs)

Two linear-phase FIRs at the DAC's 48 kHz, each inverting the measured curve:
  - eq:  flat 100 Hz - 5 kHz, the boost rolling off above 5 kHz (none past ~7.5 kHz);
  - eq8: flat 100 Hz - 8 kHz (about +30 dB at 8 kHz), rolling off above.
The target (Alex, Oct 27): every tone at the deviation the plain `txtest sweep data` 1 kHz tone gets (3046 Hz at
SWEEP_AMP). The bypass itself loses 6.8 dB against that at 1 kHz (1385 Hz, the same DAC level), so the chain's gain
at 1 kHz must be +6.8 dB on top of the curve's inverse.

What the first bench run (decision 308's design) taught (Oct 27, 2026):
  - The first design's curve was taken with the soft limiter still on: +9 dB at 100 Hz and a 4.5 dB step between
    800 Hz and 1 kHz. With the limiter off the curve is smooth and the low end is +14.5 dB (MEAS below).
  - REG_7D's mic sensitivity works under the bypass exactly as the table says: `eq`'s 5.0 dB back-off won back by
    +10 steps gave 1391 Hz against the plain bypass tone's 1385. The 6.8 dB the band was low is the bypass's own
    loss, not a failed win-back.
  - REG_40 is not linear: the tone generator's deviation against it (analysis/audio_path.md, wide, Oct 13) follows a
    power law, deviation ~ REG_40^2.9 (REG40_PTS below). `eq8`'s 0xD77 was meant as +8.5 dB and is about +25 dB by
    that law: the signal went far past the capture's +-20 kHz and read as noise. Only a small step past the measured
    0x50A is used now, and only by `eq8`.
The gain stages left after the bypass: REG_7D[5:0] (0.5 dB a step, the data key-up's 0x34 up to 0x3F: +5.5 dB)
and REG_40. The rest comes from the DAC's swing: the plain tone uses 4000 of 32767 (18.3 dB of room).

Each equaliser is fitted by weighted least squares on its zero-phase response (a cosine series), in amplitude with
weights 1/target (a dB fit), reweighted towards minimax over the measured points, then rounded to i16 taps with the
largest shift that fits. Reported per equaliser: the residual, the boost, the gain stages, each test signal's DAC
peak at levels 100 and 150 (where it saturates), and what that means for the target.
Only numpy.
"""
import sys
import numpy as np

FS = 48_000
# The bypass sweep, the soft limiter off (third octaves, dB against its own 1 kHz tone; sdrtx.py's table, Oct 27).
MEAS = np.array([
    (50, 14.5), (63, 14.6), (80, 14.6), (100, 14.4), (125, 13.9), (160, 12.9), (200, 11.8), (250, 10.5), (315, 8.7),
    (400, 7.0), (500, 5.4), (630, 3.7), (800, 1.8), (1000, 0.0), (1250, -1.9), (1600, -4.0), (2000, -6.1),
    (2500, -8.2), (3150, -10.6), (4000, -13.4), (5000, -16.6), (6300, -21.4), (8000, -30.2)])
# The bypass's loss at 1 kHz against the plain `data` sweep tone: 1385 against 3046 Hz at the same DAC level.
BYPASS_LOSS_DB = 20 * np.log10(3046 / 1385)
# REG_40's low 12 bits against the tone generator's deviation (wide, Hz; analysis/audio_path.md, Oct 13, 2026).
REG40_PTS = np.array([(0x300, 1027), (0x387, 1574), (0x3E0, 1934), (0x43C, 2551), (0x4A0, 3344), (0x50A, 4290)])
# The generators' nominal amplitudes (full scale 32767) and deviations (crates/ferrite-core/src/txtest.rs).
SWEEP_AMP = 4000          # the sweep's tones: about 3.0 kHz deviation at 1 kHz in `data` (bench, Oct 27)
DAC_FULL = 32767
LEVEL_MAX = 1.5           # the [10-150] level option's top


def smooth(f):
    """The measured curve, smoothed: a least-squares quintic in log2(f) through 63 Hz-8 kHz, held flat outside it."""
    m = MEAS[MEAS[:, 0] >= 63]
    c = np.polyfit(np.log2(m[:, 0]), m[:, 1], 5)
    return np.polyval(c, np.log2(np.clip(f, 63, 8000)))


class Spec:
    def __init__(self, name, top, roll_end, taps, cap_db):
        self.name, self.top, self.roll_end, self.taps, self.cap_db = name, top, roll_end, taps, cap_db


SPECS = [Spec("eq", 5000, 7500, 79, 18.0), Spec("eq8", 8000, 11000, 95, 31.0)]


def target_db(f, s):
    """The wanted equaliser gain, dB: minus the smoothed curve up to the top (capped), held flat below 100 Hz,
    rolling off from the top to -20 dB at `roll_end` (a raised cosine in dB), and -30 dB above."""
    g = -smooth(np.clip(f, 100, s.top))
    g = np.minimum(g, s.cap_db)
    top = min(-smooth(s.top), s.cap_db)
    hi = f > s.top
    x = np.clip((f[hi] - s.top) / (s.roll_end - s.top), 0, 1)
    g[hi] = top + (-20 - top) * (1 - np.cos(np.pi * x)) / 2
    g[f > s.roll_end] = -30
    return g


def design(s):
    """Least squares on the zero-phase response, then Lawson's reweighting so the largest error over the measured
    points from 100 Hz to the top shrinks (towards minimax), in dB."""
    n = s.taps
    m = (n - 1) // 2
    f = np.linspace(0, FS / 2, 3000)
    want = 10 ** (target_db(f, s) / 20)
    # Weights: relative error in the pass band; the transition band (top to roll_end) loosely; the stop band firmly.
    w = 1 / want
    w[(f > s.top) & (f < s.roll_end)] *= 0.3
    w[f < 60] *= 0.3
    # The measured points in the band, as rows of their own (the reweighting acts on them).
    pts = MEAS[(MEAS[:, 0] >= 100) & (MEAS[:, 0] <= s.top), 0]
    pw = np.ones(len(pts))
    pwant = 10 ** (-smooth(pts) / 20)

    def basis(fr):
        A = np.cos(2 * np.pi * np.outer(fr, np.arange(m + 1)) / FS)
        A[:, 1:] *= 2
        return A

    A, P = basis(f), basis(pts)
    for _ in range(60):
        M = np.vstack([A * w[:, None] * 0.2, P * (pw / pwant * 3)[:, None]])
        y = np.concatenate([want * w * 0.2, pw * 3])
        a = np.linalg.lstsq(M, y, rcond=None)[0]
        err = np.abs(20 * np.log10(np.abs(P @ a) / pwant))
        pw *= (err + 1e-3) / err.mean()
        pw /= pw.mean()
    return np.concatenate([a[:0:-1], a])  # symmetric, n taps


def quantise(h):
    """i16 taps and the shift: the largest power of two that keeps every tap inside i16."""
    shift = 15
    while np.max(np.abs(h)) * (1 << shift) > 32767:
        shift -= 1
    q = np.round(h * (1 << shift)).astype(int)
    return q, shift


def gain_db(q, shift, f):
    k = np.arange(len(q))
    return 20 * np.log10(np.abs(np.exp(-2j * np.pi * np.outer(f, k) / FS) @ q) / (1 << shift) + 1e-12)


def prbs9(n):
    s, out = 0x1FF, np.empty(n, np.int8)
    for i in range(n):
        b = ((s >> 8) ^ (s >> 4)) & 1
        s = ((s << 1) | b) & 0x1FF
        out[i] = b
    return out


def nrz(baud, n_bits, shaped):
    """PRBS9 NRZ at +-1, 48 kHz, optionally raised-cosine shaped (the generators' shape: txtest.rs)."""
    bits = prbs9(n_bits) * 2 - 1
    t = (np.arange(int(n_bits * FS / baud)) * baud / FS)
    i = np.minimum(t.astype(int), n_bits - 1)
    x = bits[i].astype(float)
    if shaped:
        # a half-cosine transition over the whole bit (alpha 1): at the bit boundary the mean of the two
        frac = t - np.floor(t)
        prev = bits[np.maximum(i - 1, 0)]
        nxt = bits[np.minimum(i + 1, n_bits - 1)]
        x = np.where(frac < 0.5, (bits[i] + prev) / 2 + (bits[i] - prev) / 2 * np.sin(np.pi * frac),
                     (bits[i] + nxt) / 2 + (bits[i] - nxt) / 2 * np.sin(np.pi * (1 - frac)))
    return x


def gmsk(baud, n_bits, bt=0.5):
    """+-1 frequency pulse train through a Gaussian of BT 0.5."""
    x = nrz(baud, n_bits, False)
    sps = FS / baud
    t = np.arange(-2 * sps, 2 * sps + 1) / sps
    sig = np.sqrt(np.log(2)) / (2 * np.pi * bt)
    g = np.exp(-t ** 2 / (2 * sig ** 2))
    return np.convolve(x, g / g.sum(), "same")


def g3ruh(n_bits):
    """PRBS9 scrambled by x^17 + x^12 + 1, NRZ at 9600, raised-cosine shaped."""
    d = prbs9(n_bits)
    s, out = 0, np.empty(n_bits, np.int8)
    for i, b in enumerate(d):
        o = int(b) ^ ((s >> 11) & 1) ^ ((s >> 16) & 1)
        s = ((s << 1) | o) & 0x1FFFF
        out[i] = o
    bits = out * 2 - 1
    t = np.arange(int(n_bits * FS / 9600)) * 9600 / FS
    i = np.minimum(t.astype(int), n_bits - 1)
    frac = t - np.floor(t)
    prev, nxt = bits[np.maximum(i - 1, 0)], bits[np.minimum(i + 1, n_bits - 1)]
    return np.where(frac < 0.5, (bits[i] + prev) / 2 + (bits[i] - prev) / 2 * np.sin(np.pi * frac),
                    (bits[i] + nxt) / 2 + (bits[i] - nxt) / 2 * np.sin(np.pi * (1 - frac)))


MIC_DATA = 0x34       # REG_7D[5:0] of a data key-up (crates/ferrite-core/src/vox.rs); 0.5 dB a step, 0x3F the top
DEV_KEYUP = 0x50A     # REG_40[11:0] every key-up writes (drivers/bk4829/src/tx.rs TX_DEVIATION)


def reg40_law():
    """The power law through REG40_PTS: deviation = k * REG_40^p (a least-squares line in log-log)."""
    p, lk = np.polyfit(np.log(REG40_PTS[:, 0]), np.log(REG40_PTS[:, 1]), 1)
    return p, lk


def reg40_for(db):
    """REG_40 for `db` more deviation than DEV_KEYUP by the power law, and the dB it then gives (rounded value)."""
    p, _ = reg40_law()
    v = int(round(DEV_KEYUP * 10 ** (db / 20 / p)))
    return v, 20 * p * np.log10(v / DEV_KEYUP)


def signals():
    k = SWEEP_AMP / 3000  # DAC counts per Hz of deviation at 1 kHz (the sweep tone's 3.0 kHz, `data`)
    return {
        "fsk 1200 (+-3 kHz)": nrz(1200, 1500, False) * 3000 * k,
        "fsk 4800 rc (+-3 kHz)": nrz(4800, 4000, True) * 3000 * k,
        "fsk 9600 (+-3 kHz)": nrz(9600, 6000, False) * 3000 * k,
        "fsk 9600 rc (+-3 kHz)": nrz(9600, 6000, True) * 3000 * k,
        "gmsk 4800 (h 0.5: +-1.2 kHz)": gmsk(4800, 4000) * 1200 * k,
        "gmsk 9600 (h 0.5: +-2.4 kHz)": gmsk(9600, 6000) * 2400 * k,
        "g3ruh (9600 rc, +-3 kHz)": g3ruh(6000) * 3000 * k,
    }


R40_MAX_DB = 3.0      # the most REG_40 is asked for past the measured 0x50A: the law is extrapolated there
DATA = ("fsk 4800 rc (+-3 kHz)", "fsk 9600 rc (+-3 kHz)", "gmsk 9600 (h 0.5: +-2.4 kHz)", "g3ruh (9600 rc, +-3 kHz)")


def main():
    rust = "--rust" in sys.argv
    pts, meas = MEAS[:, 0], MEAS[:, 1]
    sm = smooth(pts) - meas
    print(f"the bypass curve (limiter off), smoothed: within {np.max(np.abs(sm[pts >= 63])):.2f} dB of the points from 63 Hz")
    p, _ = reg40_law()
    fit = np.exp(np.polyval(np.polyfit(np.log(REG40_PTS[:, 0]), np.log(REG40_PTS[:, 1]), 1), np.log(REG40_PTS[:, 0])))
    print(f"REG_40 law: deviation ~ REG_40^{p:.2f} (within {np.max(np.abs(20 * np.log10(fit / REG40_PTS[:, 1]))):.1f} dB of the six points); "
          f"0xD77 by it: {20 * p * np.log10(0xD77 / DEV_KEYUP):+.0f} dB (decision 308's eq8 meant +8.5)")
    print(f"the bypass's loss at 1 kHz against the plain data tone: {BYPASS_LOSS_DB:.1f} dB")
    sigs = signals()
    out = []
    for s in SPECS:
        unit = design(s)
        unit /= np.abs(np.sum(unit * np.exp(-2j * np.pi * 1000 * np.arange(len(unit)) / FS)))  # 0 dB at 1 kHz
        G = lambda f, h=unit: 20 * np.log10(np.abs(np.exp(-2j * np.pi * np.outer(np.atleast_1d(f), np.arange(len(h))) / FS) @ h))
        print(f"\n== {s.name}: flat 100 Hz-{s.top / 1000:g} kHz, {s.taps} taps (linear phase, {(s.taps - 1) / 2 / FS * 1000:.2f} ms delay)")
        print(f"  shape: {G(s.top)[0]:+.1f} dB at {s.top / 1000:g} kHz against 1 kHz (the curve needs {-smooth(s.top):+.1f}), {G(0.0)[0]:+.1f} dB at DC")
        band = (pts >= 100) & (pts <= s.top)
        r = meas[band] + G(pts[band])
        print(f"  predicted, each tone against the plain data 1 kHz tone, 100 Hz-{s.top / 1000:g} kHz: {r.min():+.2f} to {r.max():+.2f} dB "
              "(if REG_7D and REG_40 give what they are set for)")
        print("   " + " ".join(f"{int(f)}:{v:+.1f}" for f, v in zip(pts[band], r)))
        above = pts > s.top
        if above.any():
            print("   above the band:", " ".join(f"{int(f)}:{v:+.1f}" for f, v in zip(pts[above], meas[above] + G(pts[above]))))
        # The gain stages: REG_7D to its top; REG_40 (eq8 only) the least that keeps the data signals at 100 % inside
        # the DAC, at most R40_MAX_DB.
        steps = 0x3F - MIC_DATA
        peak = lambda h, x: np.max(np.abs(np.convolve(x, h)[len(h):-len(h)]))
        sweep_f = pts.astype(float)
        r40_db, r40 = 0.0, DEV_KEYUP
        if s.name == "eq8":
            while True:
                d1 = BYPASS_LOSS_DB - steps * 0.5 - r40_db
                h = unit * 10 ** (d1 / 20)
                if max(peak(h, sigs[n]) for n in DATA) <= DAC_FULL or r40_db >= R40_MAX_DB:
                    break
                r40_db += 0.5
            r40, r40_db = reg40_for(r40_db)
        stage = steps * 0.5 + r40_db
        d1 = BYPASS_LOSS_DB - stage
        h = unit * 10 ** (d1 / 20)
        print(f"  gain stages: REG_7D 0x{MIC_DATA:02X} -> 0x3F ({steps * 0.5:+.1f} dB, measured linear), REG_40 0x{DEV_KEYUP:03X} -> 0x{r40:03X} "
              f"({r40_db:+.1f} dB by the law{', extrapolated past 0x50A' if r40 > DEV_KEYUP else ''}); the taps make the rest: {d1:+.1f} dB at 1 kHz")
        print("  DAC peaks (full scale 32767), level 100 / 150:")
        sw = SWEEP_AMP * 10 ** (np.max(G(sweep_f) + d1) / 20)
        rows = [("sweep 50 Hz-8 kHz (its loudest tone)", sw)] + [(n, peak(h, x)) for n, x in sigs.items()]
        for n, pk in rows:
            flag = lambda v: "" if v <= DAC_FULL else f" (saturates by {20 * np.log10(v / DAC_FULL):.1f} dB)"
            print(f"    {n:40s} {pk:7.0f}{flag(pk)} / {pk * 1.5:7.0f}{flag(pk * 1.5)}")
        fits = 100 * min(1.0, DAC_FULL / sw)
        print(f"  the sweep fits the DAC up to level {int(fits)} %")
        q, shift = quantise(h)
        out.append((s, q, shift, steps, r40, stage, d1))
    print("\nCarson: audio to 8 kHz at 3 kHz deviation takes about 2 x (3 + 8) = 22 kHz of RF: a wide-channel (25 kHz) set-up only;"
          " to 5 kHz, 2 x (3 + 5) = 16 kHz.")
    if rust:
        for s, q, shift, steps, r40, stage, d1 in out:
            n = s.name.upper()
            print(f"\npub static {n}_TAPS: [i16; {s.taps}] = [")
            for i in range(0, s.taps, 16):
                print("    " + ", ".join(str(v) for v in q[i:i + 16]) + ",")
            print("];")
            print(f"// {n}: shift {shift}, mic steps {steps}, REG_40 0x{r40:03X}, stages {stage:.2f} dB, taps {d1:+.2f} dB at 1 kHz")


main()
