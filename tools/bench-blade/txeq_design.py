#!/usr/bin/env python3
"""TERM-33.16 (decision 308): the TX pre-equalisers' design, from the bypass sweep of Oct 27, 2026 (black radio,
REG_47<0> = 1: "bypass all AF TX filters"; docs/research/tx-response/audio-limits-oct27.html).

    txeq_design.py            # design both, print the checks and the report
    txeq_design.py --rust     # also print the Rust statics (crates/ferrite-core/src/txeq.rs)

Two linear-phase FIRs at the DAC's 48 kHz, each inverting the measured curve (level against the 1 kHz sweep tone):
  - eq:  flat 100 Hz - 5 kHz, the boost rolling off above 5 kHz (none past ~7.5 kHz);
  - eq8: flat 100 Hz - 8 kHz (the stretch: about +29 dB at 8 kHz), rolling off above.
Both have exactly 0 dB at 1 kHz (Alex, Oct 27: an equalised 1 kHz tone comes out at the deviation the plain 1 kHz
sweep tone gives), and pull the low end (+9 dB at 100 Hz) down to flat.

The measured curve is smoothed first (a least-squares cubic in log frequency over 63 Hz-8 kHz): the step between
800 Hz (+4.5) and 1 kHz (0) is likely a measurement step. The predicted residual is the measured points (not the
smoothed curve) times the equaliser, so that step shows in it.

Each equaliser is fitted by weighted least squares on its zero-phase response (a cosine series), in amplitude with
weights 1/target (relative error, so a dB fit), then the taps are rounded to Q-format. Reported per equaliser: taps,
residual, boost, the headroom the boost costs for each test signal at full deviation (how far the generator backs
off so the DAC never clips), how much the BK4829's gain stages win back, and the noise.
Only numpy.
"""
import sys
import numpy as np

FS = 48_000
# The bypass sweep (third octaves, dB against the 1 kHz sweep tone).
MEAS = np.array([
    (50, 8.9), (63, 9.3), (80, 9.5), (100, 9.7), (125, 9.6), (160, 9.1), (200, 8.5), (250, 7.9), (315, 7.2),
    (400, 6.4), (500, 5.7), (630, 5.1), (800, 4.5), (1000, 0.0), (1250, -1.9), (1600, -4.0), (2000, -6.1),
    (2500, -8.2), (3150, -10.6), (4000, -13.8), (5000, -16.5), (6300, -21.4), (8000, -29.3)])
# The generators' nominal amplitudes (full scale 32767) and deviations (crates/ferrite-core/src/txtest.rs).
SWEEP_AMP = 4000          # the sweep's tones: about 3.0 kHz deviation at 1 kHz in `data` (bench, Oct 27)
DAC_FULL = 32767
LEVEL_MAX = 1.5           # the [10-150] level option's top


def smooth(f):
    """The measured curve, smoothed: a least-squares quintic in log2(f) through 63 Hz-8 kHz, held flat outside it.
    It runs 2.2 dB under the 800 Hz point and 0.5 over the 1 kHz one: the step between them (and the same step in
    the every-filter-off curve, but not in the data set-up's) can't be followed by any smooth response, and is
    likely a measurement step; the bench sweep with the equaliser settles it."""
    m = MEAS[MEAS[:, 0] >= 63]
    c = np.polyfit(np.log2(m[:, 0]), m[:, 1], 5)
    return np.polyval(c, np.log2(np.clip(f, 63, 8000)))


class Spec:
    def __init__(self, name, top, roll_end, taps, cap_db):
        self.name, self.top, self.roll_end, self.taps, self.cap_db = name, top, roll_end, taps, cap_db


SPECS = [Spec("eq", 5000, 7500, 79, 18.0), Spec("eq8", 8000, 11000, 95, 30.0)]


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
DEV_KEYUP = 0x50A     # REG_40[11:0] every key-up writes (drivers/bk4829/src/tx.rs TX_DEVIATION); 0xFFF the top


def levers(back):
    """The back-off (dB) won back: REG_7D's mic sensitivity first (0.5 dB steps, up to 0x3F), then REG_40's
    deviation (assumed linear; up to 0xFFF). Returns (mic steps, REG_40 value, the back-off actually used, dB):
    the taps are scaled down by exactly what the two stages give back, so the 1 kHz tone's deviation is the plain
    one's if both stages are as linear as the register tables say."""
    steps = min(0x3F - MIC_DATA, int(np.ceil(back * 2 - 1e-9)))
    rest = back - steps * 0.5
    dev = DEV_KEYUP if rest <= 0 else min(0xFFF, int(np.ceil(DEV_KEYUP * 10 ** (rest / 20))))
    return steps, dev, steps * 0.5 + 20 * np.log10(dev / DEV_KEYUP)


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


def main():
    rust = "--rust" in sys.argv
    pts, meas = MEAS[:, 0], MEAS[:, 1]
    sm = smooth(pts) - meas
    print("the measured bypass curve, smoothed (quintic in log f): off the points by " +
          " ".join(f"{int(f)}:{v:+.1f}" for f, v in zip(pts, sm) if abs(v) >= 0.5) + " (the rest within 0.5 dB)")
    sigs = signals()
    out = []
    for s in SPECS:
        h = design(s)
        h /= np.abs(np.sum(h * np.exp(-2j * np.pi * 1000 * np.arange(len(h)) / FS)))  # exactly 0 dB at 1 kHz
        unit = h.copy()
        print(f"\n== {s.name}: flat 100 Hz-{s.top / 1000:g} kHz, {s.taps} taps (linear phase, {(s.taps - 1) / 2 / FS * 1000:.2f} ms delay)")
        G = lambda f: 20 * np.log10(np.abs(np.exp(-2j * np.pi * np.outer(np.atleast_1d(f), np.arange(len(unit))) / FS) @ unit))
        need = -smooth(np.array([float(s.top)]))[0]
        print(f"  boost: {G(s.top)[0]:+.1f} dB at {s.top / 1000:g} kHz (the curve needs {need:+.1f}), {G(0.0)[0]:+.1f} dB at DC, 0.0 at 1 kHz")
        band = (pts >= 100) & (pts <= s.top)
        r = meas[band] + G(pts[band])
        inner = band & (pts != 630) & (pts != 800)
        ri = meas[inner] + G(pts[inner])
        print(f"  predicted residual (measured points x EQ, against the plain 1 kHz tone), 100 Hz-{s.top / 1000:g} kHz: "
              f"{r.min():+.2f} to {r.max():+.2f} dB; without 630 and 800 Hz (the step): {ri.min():+.2f} to {ri.max():+.2f} dB")
        print("   " + " ".join(f"{int(f)}:{v:+.1f}" for f, v in zip(pts[band], r)))
        above = pts > s.top
        if above.any():
            print("   above the band:", " ".join(f"{int(f)}:{v:+.1f}" for f, v in zip(pts[above], meas[above] + G(pts[above]))))
        fr = np.array([6000, 7000, 8000, 9000, 10000, 12000, 16000, 20000.0])
        print("  EQ gain above:", " ".join(f"{f / 1000:g}k:{v:+.0f}" for f, v in zip(fr, G(fr))))
        # Headroom: every test signal at its nominal deviation and the top level (150 %) through the unit-gain EQ.
        peaks = {"sweep 50 Hz-8 kHz (its loudest tone)": SWEEP_AMP * 10 ** (np.max(G(pts)) / 20) * LEVEL_MAX}
        for name, x in sigs.items():
            peaks[name] = np.max(np.abs(np.convolve(x, unit)[len(unit):-len(unit)])) * LEVEL_MAX
        worst = max(peaks.values())
        back = max(0.0, 20 * np.log10(worst / DAC_FULL))
        steps, dev, used = levers(back)
        print(f"  headroom: peaks at the DAC at 150 % without a back-off (full scale {DAC_FULL}):")
        for name, p in peaks.items():
            print(f"    {name:40s} {p:7.0f}")
        print(f"  back-off: {back:.1f} dB (the worst, {max(peaks, key=peaks.get)}); won back by REG_7D 0x{MIC_DATA:02X} -> 0x{MIC_DATA + steps:02X} "
              f"({steps * 0.5:+.1f} dB) and REG_40 0x{DEV_KEYUP:03X} -> 0x{dev:03X} ({20 * np.log10(dev / DEV_KEYUP):+.1f} dB): {used:.1f} dB, "
              + ("all of it" if used >= back - 1e-6 else f"short by {back - used:.1f} dB"))
        h = unit * 10 ** (-used / 20)
        q, shift = quantise(h)
        qg = lambda f: 20 * np.log10(np.abs(np.exp(-2j * np.pi * np.outer(np.atleast_1d(f), np.arange(len(q))) / FS) @ q) / (1 << shift))
        print(f"  taps: Q{shift}, the back-off folded in: 1 kHz {qg(1000.0)[0]:+.2f} dB at the DAC, "
              f"predicted 1 kHz level against the plain tone after the gain stages {qg(1000.0)[0] + used:+.2f} dB")
        # Noise. The boost is digital, ahead of the DAC: the DAC's floor and the injection's are added after it and are
        # not boosted by it. The gain stages that win the back-off back do raise both floors by `used` against the
        # signal, at every frequency, so the cost is `used` dB of SNR; the injection network then shapes the DAC's
        # floor like the signal's plain curve (falling above 1 kHz), so the top of the band is no noisier than 1 kHz.
        tone = 20 * np.log10(SWEEP_AMP / DAC_FULL)
        snr = 6.02 * 12 + 1.76 + tone + 10 * np.log10(24000 / s.top)
        print(f"  noise: the 1 kHz tone at {tone:.1f} dBFS is {snr:.0f} dB over the DAC's 12-bit floor in 0-{s.top / 1000:g} kHz plain, "
              f"{snr - used:.0f} dB with the back-off won back by the gain stages (the floor raised {used:.1f} dB)")
        cpu = (s.taps + 1) // 2
        print(f"  MCU: {cpu} multiply-accumulates a sample (symmetric taps), {cpu * FS / 1e6:.1f} M a second")
        out.append((s, q, shift, steps, dev, used))
    print("\nCarson: audio to 8 kHz at 3 kHz deviation takes about 2 x (3 + 8) = 22 kHz of RF: a wide-channel (25 kHz) set-up only;"
          " to 5 kHz, 2 x (3 + 5) = 16 kHz.")
    if rust:
        for s, q, shift, steps, dev, used in out:
            n = s.name.upper()
            print(f"\npub static {n}_TAPS: [i16; {s.taps}] = [")
            for i in range(0, s.taps, 16):
                print("    " + ", ".join(str(v) for v in q[i:i + 16]) + ",")
            print("];")
            print(f"// {n}: shift {shift}, mic steps {steps}, REG_40 0x{dev:03X}, back-off {used:.2f} dB")


main()
