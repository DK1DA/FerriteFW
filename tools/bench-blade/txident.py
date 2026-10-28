#!/usr/bin/env python3
"""TERM-33.16 (decision 311): the transmit audio channel's complex response, identified from captures of `txtest`'s
data signals (system identification: the known DAC samples against the FM-demodulated capture).

  txident.py CAP.u8 KIND BAUD [rc] [--eq eq|eq8] [--level N] [--json OUT]   # one capture
  txident.py --set DIR                                                       # the Oct 27 low-power set: both paths

KIND fsk, gmsk or g3ruh (BAUD ignored for g3ruh). The DAC samples are made as ferrite-core's txtest.rs makes them
(PRBS9, the same edges, the same amplitudes, negated), through the equaliser's taps read from
crates/ferrite-core/src/txeq.rs when --eq is given (`eq` and `eq8` as built in the radio at capture time: pass
--taps-from FILE to read another copy). The capture (bladecap.sh's u8 IQ, 1.024 MS/s, the signal 250 kHz below the
centre) goes through tools/sdrtx.py's baseband and an FM discriminator (no audio filter but the 20 kHz baseband
low-pass), resampled to 48 kHz; the clock difference between the radio and the receiver is measured from the
alignment drift over the capture and taken out; then H(f) = sum Y X* / sum |X|^2 over 8192-sample segments
(5.9 Hz bins), with the coherence. Printed per third octave: |H| in dB against 1 kHz, the phase with the bulk delay
taken out, and the group delay against its 1 kHz value.
"""
import argparse, json, os, re, sys
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
import sdrtx  # noqa: E402

FS = 48_000
SEG = 8192
THIRD = [50, 63, 80, 100, 125, 160, 200, 250, 315, 400, 500, 630, 800, 1000, 1250, 1600, 2000, 2500, 3150, 4000, 5000,
         6300, 8000]


def prbs9(n):
    s, out = 0x1FF, np.empty(n, np.int8)
    for i in range(n):
        b = ((s >> 8) ^ (s >> 4)) & 1
        s = ((s << 1) | b) & 0x1FF
        out[i] = b
    return out


def scramble(bits):
    st, out = 0, np.empty(len(bits), np.int8)
    for i, b in enumerate(bits):
        o = int(b) ^ ((st >> 11) & 1) ^ ((st >> 16) & 1)
        st = ((st << 1) | o) & 0x1FFFF
        out[i] = o
    return out


def gauss_pulse(u, bt=0.5):
    from math import erf, sqrt, log, pi
    s = sqrt(2 * log(2)) / (2 * pi * bt)
    return np.array([0.5 * (erf((v + 0.5) / s) - erf((v - 0.5) / s)) for v in np.atleast_1d(u)])


def gen(kind, baud, rc, n, level=100):
    """txtest's DAC samples (before the equaliser), n of them: ferrite-core's Nrz, in floats."""
    if kind == "g3ruh":
        baud, rc = 9600, True
    step = (baud * 1000 << 32) // (FS * 1000)
    ph = np.arange(n, dtype=np.int64) * step
    k = ph >> 32
    frac = (ph & 0xFFFFFFFF) / 2.0 ** 32
    nb = int(k[-1]) + 3
    bits = prbs9(nb)
    if kind == "g3ruh":
        bits = scramble(bits)
    b = bits.astype(float) * 2 - 1
    cur = b[k]
    prev = b[np.maximum(k - 1, 0)]
    nxt = b[k + 1]
    if kind == "gmsk":
        g = lambda u: np.interp(u, GU, GP)
        v = nxt * g(frac - 1.5) + cur * g(frac - 0.5) + prev * g(frac + 0.5)
        amp = baud // 4 * 4000 // 3000
    elif rc:
        o = np.where(frac < 0.5, prev, nxt)
        v = ((cur + o) + (cur - o) * np.sin(np.pi * frac)) / 2
        amp = 4000
    else:
        v = cur
        amp = 4000
    return -v * (amp * level // 100)


GU = np.linspace(-1.5, 1.5, 3 * 64 + 1)
GP = gauss_pulse(GU)


def taps(which, path=None):
    """The equaliser's taps and shift as txeq.rs has them."""
    path = path or os.path.join(HERE, "..", "..", "crates", "ferrite-core", "src", "txeq.rs")
    src = open(path).read()
    name = {"eq": "EQ_TAPS", "eq8": "EQ8_TAPS"}[which]
    body = src[src.index(f"pub static {name}"):]
    body = body[body.index("= [") + 3: body.index("];")]
    t = np.array([int(v) for v in re.findall(r"-?\d+", body)], float)
    m = re.search(r"pub fn shift\(self\) -> u32 \{\s*match self \{\s*Eq::Eq8 => (\d+),\s*_ => (\d+)", src)
    shift = (int(m.group(1)) if which == "eq8" else int(m.group(2))) if m else 15
    return t / 2 ** shift


def fm(raw):
    """The keyed part's deviation (Hz) at 48 kHz: sdrtx's baseband, a discriminator, 20 ms trimmed each end."""
    x = sdrtx.baseband(raw)
    fs = sdrtx.FS / sdrtx.DEC
    p = np.abs(x) ** 2
    blk = int(fs * 0.01)
    pb = p[: len(p) // blk * blk].reshape(-1, blk).mean(1)
    on = pb > np.percentile(pb, 90) * 0.1
    i0, i1 = np.argmax(on), len(on) - np.argmax(on[::-1])
    seg = x[(i0 + 2) * blk: (i1 - 2) * blk]
    d = np.angle(seg[1:] * np.conj(seg[:-1])) * fs / (2 * np.pi)
    d -= d.mean()
    return resample(d, FS / fs)


def resample(x, r):
    """x at r times its rate (FFT, band-limited)."""
    n = len(x)
    m = int(round(n * r))
    X = np.fft.rfft(x)
    Y = np.zeros(m // 2 + 1, complex)
    k = min(len(X), len(Y))
    Y[:k] = X[:k]
    return np.fft.irfft(Y, m) * m / n


def lag_of(y, x, guess=None, span=None):
    """The offset of y in x (y[i] ~ x[i + lag]), fractional, and the sign."""
    if guess is None:
        m = 1 << (len(x) + len(y)).bit_length()
        c = np.fft.irfft(np.fft.rfft(x, m) * np.conj(np.fft.rfft(y, m)), m)[: len(x) - len(y)]
    else:
        lo = max(0, guess - span)
        hi = min(len(x) - len(y), guess + span)
        c = np.array([np.dot(x[i:i + len(y)], y) for i in range(lo, hi)])
        c = np.concatenate([np.zeros(lo), c])
    i = int(np.argmax(np.abs(c)))
    if 0 < i < len(c) - 1:
        a, b, d = np.abs(c[i - 1]), np.abs(c[i]), np.abs(c[i + 1])
        frac = 0.5 * (a - d) / (a - 2 * b + d) if (a - 2 * b + d) != 0 else 0
    else:
        frac = 0
    return i + frac, np.sign(c[i])


def identify(y, x):
    """H(f) of x -> y: the clock drift found and taken out, then the cross-spectrum. Returns f, H, coherence, ppm."""
    w = FS  # 1 s chunks
    # the first chunk within x's first 3 s: the generator starts at the key-up (and PRBS9 repeats within 2 s at
    # any baud), so later copies of a periodic pattern don't send the search past x's end
    L0, sg = lag_of(y[w // 5: w // 5 + w], x[: 3 * FS + w])
    L0 -= w // 5
    pts = []
    for s in range(w // 5, len(y) - w, w // 2):
        L, _ = lag_of(y[s:s + w], x, int(round(L0 + s)), 200)
        pts.append((s, L - s))
    pts = np.array(pts)
    slope, icpt = np.polyfit(pts[:, 0], pts[:, 1], 1)
    # y's sample i is x's sample i (1 + slope) + icpt: put y on x's clock
    y2 = resample(y, 1 + slope)
    L, sg = lag_of(y2[w // 5: w // 5 + w], x[: 3 * FS + w])
    L = int(round(L))
    # y2[w // 5 + i] goes with x[L + i]
    n = min(len(y2) - w // 5, len(x) - L) // SEG * SEG
    yy, xx = y2[w // 5: w // 5 + n], x[L:L + n]
    win = np.hanning(SEG)
    Sxy = np.zeros(SEG // 2 + 1, complex)
    Sxx = np.zeros(SEG // 2 + 1)
    Syy = np.zeros(SEG // 2 + 1)
    for a in range(0, n - SEG + 1, SEG // 2):
        X = np.fft.rfft(xx[a:a + SEG] * win)
        Y = np.fft.rfft(yy[a:a + SEG] * win)
        Sxy += Y * np.conj(X)
        Sxx += np.abs(X) ** 2
        Syy += np.abs(Y) ** 2
    f = np.fft.rfftfreq(SEG, 1 / FS)
    coh = np.abs(Sxy) ** 2 / np.maximum(Sxx * Syy, 1e-30)
    h, c, snr = ls_fir(yy, xx)
    H = np.fft.rfft(np.roll(np.concatenate([h, np.zeros(SEG - len(h))]), -c))
    return f, H, coh, Sxx, slope * 1e6, h, snr


def ls_fir(y, x, K=481, ridge=1e-6):
    """The least-squares FIR from x to y, two-sided: y[n] = sum_k h[k] x[n + c - k], c = K // 2 (+-5 ms), by the
    normal equations (x's autocorrelation, Toeplitz, and the cross-correlation, both by FFT). Also how much of y it
    explains, dB (the linear model's SNR)."""
    c = K // 2
    n = len(x)
    m = 1 << (2 * n).bit_length()
    Xf = np.fft.rfft(x, m)
    R = np.fft.irfft(np.abs(Xf) ** 2, m)[:K]
    P = np.fft.irfft(np.conj(np.fft.rfft(y, m)) * Xf, m)  # P[d] = sum y[n] x[n + d]
    P = np.array([P[(c - j) % m] for j in range(K)])
    T = R[np.abs(np.arange(K)[:, None] - np.arange(K)[None, :])]
    h = np.linalg.solve(T + ridge * R[0] * np.eye(K), P)
    pred = np.convolve(x, h)[c:c + n]
    return h, c, 10 * np.log10(np.var(y) / np.var(y - pred))


def third_octaves(f, H, coh, Sxx):
    """|H| dB against 1 kHz, phase (deg, the bulk delay over 300 Hz-3 kHz taken out) and group delay (us, against
    1 kHz) per third octave, from the bins with coherence over 0.9 and input power within 30 dB of the band's."""
    good = (coh > 0.9) & (Sxx > Sxx[(f > 50) & (f < 8000)].max() * 1e-4)
    ph = np.unwrap(np.angle(H))
    sel = good & (f > 300) & (f < 3000)
    d = np.polyfit(2 * np.pi * f[sel], ph[sel], 1)[0] if sel.sum() > 5 else 0
    ph2 = ph - d * 2 * np.pi * f
    ph2 -= ph2[np.argmin(np.abs(f - 1000))]
    gd = -np.gradient(ph, 2 * np.pi * f)
    ref = np.median(np.abs(H[good & (f > 900) & (f < 1100)])) if (good & (f > 900) & (f < 1100)).any() else np.nan
    gref = np.median(gd[good & (f > 800) & (f < 1250)]) if (good & (f > 800) & (f < 1250)).any() else np.nan
    rows = []
    for c in THIRD:
        m = good & (f > c * 2 ** -(1 / 6)) & (f < c * 2 ** (1 / 6))
        if m.sum() < 1:
            rows.append((c, None, None, None))
            continue
        rows.append((c, 20 * np.log10(np.median(np.abs(H[m])) / ref), float(np.degrees(np.median(ph2[m]))),
                     float((np.median(gd[m]) - gref) * 1e6)))
    return rows, -d


def run(cap, kind, baud, rc, eq, level, taps_from=None, quiet=False):
    raw = np.fromfile(cap, np.uint8)
    y = fm(raw)
    x = gen(kind, baud, rc, len(y) + 4 * FS, level)
    if eq:
        x = np.clip(np.convolve(x, taps(eq, taps_from))[: len(x)], -32767, 32767)
    f, H, coh, Sxx, ppm, h, snr = identify(y, x)
    rows, delay = third_octaves(f, H, coh, Sxx)
    if not quiet:
        print(f"{os.path.basename(cap)}: {kind} {baud} {'rc ' if rc else ''}{eq or 'plain'} level {level}: clock {ppm:+.1f} ppm, "
              f"bulk delay {delay * 1e3:.2f} ms, the linear model explains {snr:.1f} dB")
        print("   Hz     |H| dB   phase   group delay us (vs 1 kHz)")
        for c, m, p, g in rows:
            if m is not None:
                print(f"  {c:5d}  {m:+7.1f}  {p:+6.0f}  {g:+8.0f}")
    return dict(f=f, H=H, coh=coh, Sxx=Sxx, rows=rows, delay=delay, h=h, snr=snr)


# The Oct 27 low-power captures: name -> (kind, baud, rc, eq, level, path)
SET = {
    "lp_f1200": ("fsk", 1200, False, None, 100, "data"),
    "lp_f2400": ("fsk", 2400, False, None, 100, "data"),
    "lp_g4800": ("gmsk", 4800, False, None, 100, "data"),
    "lp_g3ruh": ("g3ruh", 9600, True, None, 100, "data"),
    "lp_f1200eq": ("fsk", 1200, False, "eq", 100, "bypass"),
    "lp_f2400eq": ("fsk", 2400, False, "eq", 100, "bypass"),
    "lp_f4800eq": ("fsk", 4800, True, "eq", 100, "bypass"),
    "lp_f4800eq60": ("fsk", 4800, True, "eq", 60, "bypass"),
    "lp_g4800eq": ("gmsk", 4800, False, "eq", 100, "bypass"),
    "lp_g3ruheq": ("g3ruh", 9600, True, "eq", 100, "bypass"),
    "lp_f9600eq8": ("fsk", 9600, True, "eq8", 100, "bypass"),
    "lp_g3ruheq8": ("g3ruh", 9600, True, "eq8", 100, "bypass"),
    "lp_g3ruh60": ("g3ruh", 9600, True, "eq8", 60, "bypass"),
    "lp_g3ruh30": ("g3ruh", 9600, True, "eq8", 30, "bypass"),
}


def combine(results):
    """One H per path: each capture normalised at 1 kHz, the bins averaged weighted by input power x coherence."""
    f = results[0]["f"]
    num = np.zeros(len(f), complex)
    den = np.zeros(len(f))
    for r in results:
        H, coh, Sxx = r["H"], r["coh"], r["Sxx"]
        m = (coh > 0.9) & (f > 900) & (f < 1100)
        if not m.any():
            continue
        # 1 kHz to 1 (complex: the sign and the phase there), then the bulk delay out (each capture has its own)
        i1 = np.argmin(np.abs(f - 1000))
        Hn = H / H[i1]
        ph = np.unwrap(np.angle(Hn))
        sel = (coh > 0.9) & (f > 300) & (f < 3000)
        d = np.polyfit(2 * np.pi * (f[sel] - f[i1]), ph[sel] - ph[i1], 1)[0]
        Hn = Hn * np.exp(-1j * d * 2 * np.pi * (f - f[i1]))
        wgt = Sxx * coh * (coh > 0.8) / Sxx.max()
        num += wgt * Hn
        den += wgt
    return f, num / np.maximum(den, 1e-30), den


def main():
    a = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    a.add_argument("cap", nargs="?")
    a.add_argument("kind", nargs="?", choices=["fsk", "gmsk", "g3ruh"])
    a.add_argument("baud", nargs="?", type=int, default=9600)
    a.add_argument("rc", nargs="?")
    a.add_argument("--eq", choices=["eq", "eq8"])
    a.add_argument("--level", type=int, default=100)
    a.add_argument("--taps-from")
    a.add_argument("--set", help="the directory of the Oct 27 lp_*.u8 captures: identify both paths")
    a.add_argument("--json", help="write the combined responses (f, re, im, weight per path) here")
    o = a.parse_args()
    if o.set:
        paths = {"data": [], "bypass": []}
        for name, (kind, baud, rc, eq, level, path) in SET.items():
            p = os.path.join(o.set, name + ".u8")
            if os.path.exists(p):
                paths[path].append(run(p, kind, baud, rc, eq, level, o.taps_from))
                print()
        out = {}
        for path, rs in paths.items():
            if not rs:
                continue
            f, H, w = combine(rs)
            rows, _ = third_octaves(f, H, np.where(w > 0, 1.0, 0.0), w)
            print(f"== {path} path, {len(rs)} captures combined")
            print("   Hz     |H| dB   phase   group delay us (vs 1 kHz)")
            for c, m, p, g in rows:
                if m is not None:
                    print(f"  {c:5d}  {m:+7.1f}  {p:+6.0f}  {g:+8.0f}")
            out[path] = dict(f=f.tolist(), re=H.real.tolist(), im=H.imag.tolist(), w=w.tolist())
        if o.json:
            json.dump(out, open(o.json, "w"))
        return
    if not o.cap:
        a.error("CAP and KIND, or --set DIR")
    run(o.cap, o.kind, o.baud, o.rc == "rc", o.eq, o.level, o.taps_from)


if __name__ == "__main__":
    main()
