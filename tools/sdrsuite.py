"""TERM-34 `txtest suite` measured from one RTL-SDR capture: every transmit baseband test of one key-up, segmented by
the section markers, each section measured, a text report and one self-contained HTML page (PNG charts embedded)
written next to the capture.

    sdrsuite.py 145.495 --save suite.u8      # `:txtest suite` on the radio; start this, then hold PTT ~65 s
    sdrsuite.py 0 --load suite.u8            # analyse a saved capture again (the frequency is only a label)
    sdrsuite.py 0 --selftest [--out DIR]     # the whole suite synthesised as IQ and measured, no radio or SDR

The suite (ferrite-core `txsuite.rs`, mirrored in `steps()` below; keep the two the same). Each section opens with
its marker: 300 ms of silence (carrier only), then 150 ms of a tone from DAC1 in `data` at 350 + 50 n Hz. Times are
from the end of the marker; the whole is 59.95 s from the key-up, then 1 kHz until the release.

    n  marker  what                                                             length after the marker
    1  400 Hz  sweep in data: 23 third-octave tones 50 Hz-8 kHz, 250 ms + 50 ms     6.96 s
    2  450 Hz  the same sweep in voice                                              6.96 s
    3  500 Hz  DAC1 AFSK flags in data: 100 % (APRS level) 3 s, 200 ms gap, 50 % 2 s 5.20 s
    4  550 Hz  tone-generator AFSK flags (afsk-bk) 2 s each + 200 ms carrier:        17.60 s
               REG_40 0x300 0x387 0x3E0 0x43C 0x4A0 0x50A in voice, then 0x43C in
               data (REG_2B 100) and flat (111)
    5  600 Hz  tone generator at stock's REG_40: 1000 Hz 1 s, 300 ms, then the end   4.08 s
               tones (Beep, Two-tone up, Two-tone down, Chirp, Quindar), 300 ms after each
    6  650 Hz  sub-tones alone (voice, DAC1 silent): CTCSS 67.0, 100.0, 151.4,       12.00 s
               250.3 Hz, DCS 023 N; 2 s each + 400 ms
    7  700 Hz  4FSK in data at 60 % (M17 levels, RRC 0.5, PRBS9)                     3.30 s
       1 kHz   the end: held until the release

Measured: the sweep tables and -3 dB points of both set-ups; AFSK deviation, twist, bit timing jitter and phase at
the switches (sdrtx's `afsk`); REG_40 -> deviation with a fitted curve and the REG_40 for 3.0 kHz, the filters'
effect, each burst's swell; the 1 kHz attack (time constant, start level) and steady level; each end tone's
frequency, length and level at its start and end; CTCSS frequency and deviation, DCS bit rate, deviation and code
(decoded; normal when a 1 bit is positive deviation); 4FSK levels and symbol errors (sdrtx's `eye`). Needs numpy and
matplotlib (the project's .venv) and rtl_sdr for a capture.
"""
import argparse, base64, contextlib, html, io, os, sys, tempfile
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sdrtx  # noqa: E402
import sdrtones  # noqa: E402

# ---- The suite, as ferrite-core's txsuite.rs ----

REG40_STOCK = 0x50A
REG40_STEPS = [0x300, 0x387, 0x3E0, 0x43C, 0x4A0, 0x50A]
REG40_FILTERS = 0x43C
MARKER_GAP, MARKER = 0.300, 0.150
LEAD, TAIL = 1.000, 0.300
SWEEP_TONE, SWEEP_GAP = 0.250, 0.050
SWEEP_STEP = 6.960
END_TONES = [("Beep", [(1000, 150)]), ("Two-tone up", [(800, 100), (1200, 100)]), ("Two-tone down", [(1200, 100), (800, 100)]),
             ("Chirp", [(1000, 60), (1250, 60), (1500, 60)]), ("Quindar", [(2475, 250)])]
CTCSS = [67.0, 100.0, 151.4, 250.3]
DCS = 0o23
SECTIONS = ["sweep, data", "sweep, voice", "AFSK from DAC1, data, 100 % then 50 %",
            "AFSK from the tone generator per REG_40, then data and flat", "tone generator 1 kHz, then the end tones",
            "sub-tones: CTCSS 67.0, 100.0, 151.4, 250.3 Hz, DCS 023", "4FSK, data, 60 %"]
SUITE_S = 59.950
AFSK_DAC = 3900  # the APRS level's DAC peak (ferrite-core audio::AFSK_AMP)
MARKER_AMP, SWEEP_AMP = 3000, 4000


def marker_hz(n):
    return 350 + 50 * n


def steps():
    """The suite's steps in order: dicts with s (length), kind, arg, filters, reg40, section (0 the lead-in)."""
    out = []

    def add(sec, s, kind, arg=None, filters="data", reg40=REG40_STOCK):
        out.append(dict(s=s, kind=kind, arg=arg, filters=filters, reg40=reg40, section=sec))

    add(0, LEAD, "sil")
    for n in range(1, 8):
        add(n, MARKER_GAP, "sil")
        add(n, MARKER, "tone", marker_hz(n))
        if n in (1, 2):
            add(n, SWEEP_STEP, "sweep", filters="data" if n == 1 else "voice")
        elif n == 3:
            add(n, 3.0, "afsk", 100)
            add(n, 0.2, "sil")
            add(n, 2.0, "afsk", 50)
        elif n == 4:
            for r, f in [(r, "voice") for r in REG40_STEPS] + [(REG40_FILTERS, "data"), (REG40_FILTERS, "flat")]:
                add(n, 2.0, "bkflags", filters=f, reg40=r)
                add(n, 0.2, "bkquiet", filters=f, reg40=r)
        elif n == 5:
            add(n, 1.0, "bktone", 1000, "voice")
            add(n, 0.3, "bkquiet", filters="voice")
            for _, ts in END_TONES:
                for hz, ms in ts:
                    add(n, ms / 1000, "bktone", hz, "voice")
                add(n, 0.3, "bkquiet", filters="voice")
        elif n == 6:
            for t in [round(c * 10) for c in CTCSS] + [0x8000 | DCS]:
                add(n, 2.0, "sub", t, "voice")
                add(n, 0.4, "sil", filters="voice")
        else:
            add(n, 3.0, "fsk4", 60)
            add(n, TAIL, "sil")
    t = 0.0
    for st in out:
        st["t"] = t
        t += st["s"]
    return out


STEPS = steps()


def marker_end_t(n):
    """When section n's marker ends, s from the key-up (the suite's start)."""
    return next(st["t"] + st["s"] for st in STEPS if st["section"] == n and st["kind"] == "tone")


def section_len(n):
    end = SUITE_S if n == 7 else marker_end_t(n + 1) - MARKER - MARKER_GAP
    return end - marker_end_t(n)


# The 104 standard DCS codes (ferrite-app slot.rs DCS): a codeword's rotations are other codes' too (023 N is also
# 340 N, 766 N and 047 I ...); the decode names the standard ones, normal first.
DCS_CODES = [0o023, 0o025, 0o026, 0o031, 0o032, 0o036, 0o043, 0o047, 0o051, 0o053, 0o054, 0o065, 0o071, 0o072, 0o073, 0o074,
             0o114, 0o115, 0o116, 0o122, 0o125, 0o131, 0o132, 0o134, 0o143, 0o145, 0o152, 0o155, 0o156, 0o162, 0o165, 0o172,
             0o174, 0o205, 0o212, 0o223, 0o225, 0o226, 0o243, 0o244, 0o245, 0o246, 0o251, 0o252, 0o255, 0o261, 0o263, 0o265,
             0o266, 0o271, 0o274, 0o306, 0o311, 0o315, 0o325, 0o331, 0o332, 0o343, 0o346, 0o351, 0o356, 0o364, 0o365, 0o371,
             0o411, 0o412, 0o413, 0o423, 0o431, 0o432, 0o445, 0o446, 0o452, 0o454, 0o455, 0o462, 0o464, 0o465, 0o466, 0o503,
             0o506, 0o516, 0o523, 0o526, 0o532, 0o546, 0o565, 0o606, 0o612, 0o624, 0o627, 0o631, 0o632, 0o654, 0o662, 0o664,
             0o703, 0o712, 0o723, 0o731, 0o732, 0o734, 0o743, 0o754]


def dcs_codeword(code, inverted=False):
    """bk4829's dcs_codeword: the code in bits 8:0, 100, then 11 Golay (23,12) parity bits."""
    data = 0x800 | (code & 0x1FF)
    r = data << 11
    for i in range(22, 10, -1):
        if r >> i & 1:
            r ^= 0xC75 << (i - 11)
    w = data | r << 12
    return (~w & 0x7FFFFF) if inverted else w


# ---- Signal helpers ----


def movavg(x, n):
    """Centred moving average of n samples (cumulative sums: fast on a minute of samples)."""
    n = max(1, int(n))
    c = np.concatenate([[0.0], np.cumsum(x, dtype=np.float64)])
    i = np.arange(len(x))
    lo = np.clip(i - n // 2, 0, len(x))
    hi = np.clip(i - n // 2 + n, 0, len(x))
    return (c[hi] - c[lo]) / np.maximum(hi - lo, 1)


def tone_env(x, fs, hz, ms, t0=0):
    """The amplitude of the `hz` component, smoothed over `ms` (x starts at sample t0 of the capture)."""
    z = x * np.exp(-2j * np.pi * hz * (np.arange(len(x)) + t0) / fs)
    n = fs * ms / 1000
    return 2 * np.abs(movavg(z.real, n) + 1j * movavg(z.imag, n))


def amp_env(x, fs, ms):
    """A sine's amplitude from the mean square over `ms`."""
    return np.sqrt(2 * movavg(x * x, fs * ms / 1000))


def runs(mask):
    """(start, end) index pairs of the True runs."""
    d = np.diff(np.concatenate([[0], mask.astype(np.int8), [0]]))
    return list(zip(np.nonzero(d == 1)[0], np.nonzero(d == -1)[0]))


def keyed(raw):
    """The capture's baseband, the keyed part's FM deviation (Hz, offset removed, full band) and where it starts and
    ends (samples at 64 kS/s); None if no transmission."""
    x = sdrtx.baseband(raw)
    fs = sdrtx.FS / sdrtx.DEC
    blk = int(fs * 0.01)
    p = np.abs(x) ** 2
    pb = p[: len(p) // blk * blk].reshape(-1, blk).mean(1)
    floor = np.percentile(pb, 5)
    if floor > 1e3:
        floor = 1.0
    on = pb > floor * 100
    if not on.any():
        return None
    i0, i1 = (np.argmax(on) + 2) * blk, (len(on) - np.argmax(on[::-1]) - 2) * blk
    seg = x[i0:i1]
    fm = np.angle(seg[1:] * np.conj(seg[:-1])) * fs / (2 * np.pi)
    off = float(np.median(fm))
    snr = 10 * np.log10(pb[i0 // blk : i1 // blk].mean() / floor)
    clip = float(np.mean((raw == 0) | (raw == 255)))
    return fm - off, fs, off, i0, i1, snr, clip


# ---- Segmentation ----


def find_markers(dev, fs):
    """Each section's marker: a 120-190 ms tone at its frequency (over 70 % of the audio there) after at least 250 ms
    of quiet. Returns {n: (start, end, level Hz)} in samples of `dev`; the first such run per section in time order
    after the one before."""
    tot = amp_env(dev, fs, 10)
    found = {}
    after = 0
    for n in range(1, 8):
        env = tone_env(dev, fs, marker_hz(n), 10)
        on = (env > 0.7 * tot) & (env > 300)
        for a, b in runs(on):
            if a < after or not (0.10 < (b - a) / fs < 0.20):
                continue
            lvl = np.median(env[a:b])
            # The edges where the tone crosses half its level (a 10 ms boxcar makes a step a ramp centred on it).
            lo = max(a - int(0.03 * fs), 0)
            a2 = lo + int(np.argmax(env[lo : b] > 0.5 * lvl))
            b2 = b - 1 + int(np.argmax(env[b - 1 : b + int(0.03 * fs)] < 0.5 * lvl))
            q0 = a2 - int(0.28 * fs)
            if q0 < 0 or np.mean(tot[q0 : a2 - int(0.03 * fs)]) > 0.25 * lvl:
                continue
            found[n] = (a2, b2, float(lvl))
            after = b2
            break
    return found


# ---- Per-section measurements ----


def sub_band(dev, fs, a, b, rate=2000):
    """dev[a:b] decimated to `rate` and low-passed at 300 Hz: the sub-audio band."""
    k = int(fs // rate)
    x = sdrtx.conv_same(dev[a:b], sdrtx.lowpass(900, fs, 511))[::k]
    return sdrtx.conv_same(x, sdrtx.lowpass(300, fs / k, 255)), fs / k


def ctcss(x, fs):
    """The strongest tone 60-260 Hz: frequency (parabolic peak of a zero-padded FFT) and deviation (fitted)."""
    w = np.hanning(len(x))
    nf = 1 << 19
    sp = np.abs(np.fft.rfft((x - x.mean()) * w, nf))
    fr = np.fft.rfftfreq(nf, 1 / fs)
    band = (fr > 60) & (fr < 260)
    j = int(np.argmax(sp * band))
    y0, y1, y2 = np.log(sp[j - 1 : j + 2] + 1e-12)
    f = fr[j] + 0.5 * (y0 - y2) / (y0 - 2 * y1 + y2) * (fr[1] - fr[0])
    return float(f), sdrtx.fit_amp(x - x.mean(), fs, f)


def dcs(x, fs, baud=134.4):
    """DCS from the sub-audio band: the bit clock (the phase with the most open eye, the rate fitted +-1 %), the
    deviation (median |level| at the bit centres) and the code: a 23-bit window that is a codeword with its 100
    marker, normal when a 1 bit is positive deviation. Returns (code or None, inverted, baud, deviation Hz,
    windows that decoded / windows)."""
    x = x - np.median(x)
    best = None
    for b in baud * (1 + np.arange(-10, 11) * 1e-3):
        per = fs / b
        for ph in np.arange(0, per, per / 32):
            pos = ph + np.arange(int((len(x) - ph) / per) - 1) * per
            v = np.interp(pos, np.arange(len(x)), x)
            score = np.mean(np.abs(v))
            if best is None or score > best[0]:
                best = (score, b, v)
    _, b, v = best
    bits = (v > 0).astype(int)
    votes = {}
    for s in range(len(bits) - 23):
        w = int(sum(int(bits[s + i]) << i for i in range(23)))
        for inv in (False, True):
            ww = (~w & 0x7FFFFF) if inv else w
            if ww & 0xE00 == 0x800 and dcs_codeword(ww & 0x1FF) == ww:
                votes[(ww & 0x1FF, inv)] = votes.get((ww & 0x1FF, inv), 0) + 1
    dev = float(np.median(np.abs(v)))
    if not votes:
        return None, False, float(b), dev, (0, len(bits) - 23)
    # Every rotation of the code decodes as often: standard codes first, then normal, then the most windows.
    (code, inv), k = max(votes.items(), key=lambda kv: (kv[0][0] in DCS_CODES, not kv[0][1], kv[1]))
    return code, inv, float(b), dev, (k, len(bits) - 23)


def attack(env, fs, t_on):
    """A swell's start level, time constant and 90 % time: env (an amplitude envelope) from its onset `t_on`
    (samples), fitted with S - (S - I) exp(-t / tau) over its first 0.6 s; S the steady level."""
    seg = env[t_on : t_on + int(0.6 * fs)]
    t = np.arange(len(seg)) / fs
    use = t > 0.006
    best = None
    for tau in np.geomspace(0.002, 0.5, 200):
        e = np.exp(-t[use] / tau)
        A = np.column_stack([np.ones(use.sum()), -e])
        c, res, *_ = np.linalg.lstsq(A, seg[use], rcond=None)
        r = float(np.sum((A @ c - seg[use]) ** 2))
        if best is None or r < best[0]:
            best = (r, tau, c)
    _, tau, (S, d) = best
    I = S - d
    k90 = np.nonzero(seg[int(0.006 * fs) :] >= 0.9 * S)[0] + int(0.006 * fs)
    return dict(tau_ms=tau * 1000, start=I / S if S else 0.0, steady=float(S), t90_ms=float(k90[0] / fs * 1000) if len(k90) else float("nan"))


@contextlib.contextmanager
def tee(buf):
    """Print to the terminal and into `buf`."""
    class T(io.TextIOBase):
        def write(self, s):
            sys.__stdout__.write(s)
            buf.write(s)
            return len(s)
    old = sys.stdout
    sys.stdout = T()
    try:
        yield
    finally:
        sys.stdout = old


def fit_reg40(xs, ys):
    """REG_40 (low 12 bits) to deviation: an exponential (a line through log dev) and a quadratic, the better of the
    two by relative rms; the REG_40 for 3.0 kHz from it and between the measured points. Returns a dict."""
    xs, ys = np.asarray(xs, float), np.asarray(ys, float)
    ok = ys > 0
    out = {}
    fits = {}
    if ok.sum() >= 2:
        b, a = np.polyfit(xs[ok], np.log(ys[ok]), 1)
        f = lambda x, a=a, b=b: np.exp(a + b * np.asarray(x, float))  # noqa: E731
        fits["exponential"] = (f, (np.log(3000) - a) / b if b > 0 else None, f"dev = exp({a:.3f} + {b:.5f} x)")
    if ok.sum() >= 3:
        c = np.polyfit(xs[ok], ys[ok], 2)
        f = lambda x, c=c: np.polyval(c, x)  # noqa: E731
        r = np.roots(c - np.array([0, 0, 3000]))
        r = [float(v.real) for v in r if abs(v.imag) < 1e-9 and xs.min() - 200 <= v.real <= xs.max() + 200]
        fits["quadratic"] = (f, min(r, key=lambda v: abs(v - np.interp(3000, ys, xs))) if r else None, f"dev = {c[0]:.4g} x^2 + {c[1]:.4g} x + {c[2]:.4g}")
    if not fits:
        return out
    rms = {k: float(np.sqrt(np.mean(((v[0](xs[ok]) - ys[ok]) / ys[ok]) ** 2))) for k, v in fits.items()}
    kind = min(rms, key=rms.get)
    f, x3, text = fits[kind]
    order = np.argsort(ys)
    lin = float(np.interp(3000, ys[order], xs[order])) if ys.min() <= 3000 <= ys.max() else None
    out.update(kind=kind, rms=rms[kind], f=f, x3000=x3, text=text, lin3000=lin, all_rms=rms)
    return out


# ---- The whole analysis ----


def analyse(raw, out_dir, stem, mhz=0.0):
    """Every section measured; the text report and the HTML page written to out_dir. Returns the results dict."""
    buf = io.StringIO()
    R = dict(sections={}, charts={})
    with tee(buf):
        R.update(_analyse(raw, out_dir, stem, mhz, R))
    txt = os.path.join(out_dir, stem + "-suite.txt")
    with open(txt, "w") as f:
        f.write(buf.getvalue())
    page = os.path.join(out_dir, stem + "-suite.html")
    with open(page, "w") as f:
        f.write(html_page(R, buf.getvalue(), stem, mhz))
    print(f"report: {txt}\npage:   {page}")
    R["txt"], R["html"] = txt, page
    return R


def _analyse(raw, out_dir, stem, mhz, R):
    k = keyed(raw)
    if k is None:
        print("no transmission found")
        return dict(ok=False)
    fm, fs, off, i0, i1, snr, clip = k
    print(f"txtest suite: keyed {(i1 - i0) / fs:.2f} s, {snr:.0f} dB over the floor, IQ clipping {clip:.2%}; carrier offset {off:+.0f} Hz"
          + (f" ({off / max(mhz, 1e-9):+.1f} ppm with the dongle's error)" if mhz else ""))
    dev6 = sdrtx.conv_same(fm, sdrtx.lowpass(6000, fs))
    dev12 = sdrtx.conv_same(fm, sdrtx.lowpass(12000, fs))
    R.update(fs=fs, offset=off, keyed_s=(i1 - i0) / fs, snr=snr, clip=clip)
    mk = find_markers(dev6, fs)
    R["markers"] = mk
    print("\nmarkers (s from the start of the keyed part; expected from section 1's):")
    t1 = mk[1][1] / fs if 1 in mk else None
    for n in range(1, 8):
        if n not in mk:
            print(f"  {n}  {marker_hz(n)} Hz  NOT FOUND")
            continue
        a, b, lvl = mk[n]
        exp = marker_end_t(n) - marker_end_t(1)
        got = b / fs - t1 if t1 is not None else float("nan")
        print(f"  {n}  {marker_hz(n)} Hz  {a / fs:7.3f} - {b / fs:7.3f} s ({(b - a) / fs * 1000:3.0f} ms, {lvl:4.0f} Hz dev)   "
              f"{got:7.3f} s after 1's end (expected {exp:7.3f}, {(got - exp) * 1000:+5.1f} ms)")
    if len(mk) >= 2:
        ns = sorted(mk)
        drift = (marker_end_t(ns[-1]) - marker_end_t(ns[0])) / ((mk[ns[-1]][1] - mk[ns[0]][1]) / fs) - 1
        print(f"  the radio's step clock against the dongle's: {drift * 1e6:+.0f} ppm (+: the radio's runs fast)")
        R["timing_ppm"] = drift * 1e6
    S = R["sections"]
    plt = sdrtx.pyplot()

    def at(n, t):
        return mk[n][1] + int(round(t * fs))

    # 1, 2: sweeps.
    sweeps = {}
    for n, name in ((1, "data"), (2, "voice")):
        print(f"\n== {n}: sweep, {name}")
        if n not in mk:
            print("  no marker")
            continue
        fr, a = sdrtx.sweep_measure(dev12, fs, [mk[n][1]], SWEEP_TONE, SWEEP_GAP)
        r = sdrtx.sweep_report(fr, a)
        if r:
            sweeps[name] = r
            S[n] = dict(low=r["low"], high=r["high"], ref=r["ref"], db=list(map(float, r["db"])), freqs=r["freqs"])
    # 3: DAC1 AFSK.
    print("\n== 3: AFSK from DAC1, data")
    if 3 in mk:
        S[3] = {}
        for lvl, (a, b) in ((100, (0.15, 2.9)), (50, (3.35, 5.1))):
            print(f" {lvl} %:")
            S[3][lvl] = sdrtx.afsk(dev6[at(3, a) : at(3, b)], fs)
    # 4: the tone generator's flags.
    print("\n== 4: AFSK from the tone generator (afsk-bk), per REG_40 and set-up")
    bursts = []
    if 4 in mk:
        env_all = amp_env(dev6, fs, 10)
        settings = [(r, "voice") for r in REG40_STEPS] + [(REG40_FILTERS, "data"), (REG40_FILTERS, "flat")]
        for kk, (reg, filt) in enumerate(settings):
            a0 = at(4, kk * 2.2)
            print(f" REG_40 0x{reg:03X}, {filt}:")
            r = sdrtx.afsk(dev6[a0 + int(0.3 * fs) : a0 + int(1.9 * fs)], fs)
            # The burst starts at its step's time: the chip leaves the marker's mic path (burst 1) or the carrier
            # (the others) as the Radio task writes the step, which the marker's end on air marks.
            t_on = a0
            sw = attack(env_all, fs, t_on)
            r.update(reg40=reg, filters=filt, dev=(r.get(1200, 0) + r.get(2200, 0)) / 2, swell=sw, env=env_all[t_on - int(0.05 * fs) : t_on + int(0.6 * fs)])
            print(f"  swell: starts at {sw['start'] * 100:.0f} % of its steady level, time constant {sw['tau_ms']:.0f} ms, 90 % after {sw['t90_ms']:.0f} ms")
            bursts.append(r)
        voice = [b for b in bursts if b["filters"] == "voice"]
        print("\n REG_40 -> deviation (voice; the mean of the two tones):")
        for b in voice:
            print(f"   0x{b['reg40']:03X} ({b['reg40']:4d})  {b['dev']:6.0f} Hz   twist {b.get('twist', 0):+.2f} dB   jitter {b.get('jitter_rms_us', float('nan')):.1f} us rms")
        fit = fit_reg40([b["reg40"] for b in voice], [b["dev"] for b in voice])
        if fit:
            print(f"   fit ({fit['kind']}, {fit['rms'] * 100:.1f} % rms): {fit['text']}")
            x3 = fit["x3000"]
            print(f"   REG_40 for 3.0 kHz: {f'0x{int(round(x3)):03X} ({x3:.0f})' if x3 else 'outside the fit'} from the fit; "
                  f"{f'0x{int(round(fit['lin3000'])):03X}' if fit['lin3000'] else 'not between the measured points'} between the measured points")
        f43 = {b["filters"]: b["dev"] for b in bursts if b["reg40"] == REG40_FILTERS}
        if "voice" in f43 and f43["voice"] > 0:
            print("   0x43C by set-up: " + ", ".join(f"{k} {v:.0f} Hz ({20 * np.log10(max(v, 1e-3) / f43['voice']):+.1f} dB)" for k, v in f43.items()))
        S[4] = dict(bursts=bursts, fit=fit, by_filters=f43)
    # 5: tone generator.
    print("\n== 5: tone generator at stock's REG_40 (voice)")
    if 5 in mk:
        a0 = at(5, 0)
        env = tone_env(dev6[a0 - int(0.1 * fs) : a0 + int(1.1 * fs)], fs, 1000, 5, a0 - int(0.1 * fs))
        steady = float(np.median(env[int(0.7 * fs) : int(1.05 * fs)]))
        t_on = int(0.1 * fs)  # the step's start: the marker's end (the chip leaves the mic path then)
        sw = attack(env, fs, t_on)
        print(f"  1000 Hz: steady {steady:.0f} Hz deviation; starts at {sw['start'] * 100:.0f} % ({sw['start'] * steady:.0f} Hz), "
              f"time constant {sw['tau_ms']:.0f} ms, 90 % after {sw['t90_ms']:.0f} ms")
        S[5] = dict(steady=steady, attack=sw, env=env[t_on - int(0.05 * fs) :], tones=[])
        a1, b1 = at(5, 1.15), at(5, section_len(5))
        seg = dev6[a1:b1] - np.median(dev6[a1:b1])
        found = sdrtones.steady_tones(seg, fs)
        want = [(name, hz, ms) for name, ts in END_TONES for hz, ms in ts]
        j = 0
        print("  end tones (step: Hz, length, deviation at its start / median / end):")
        for name, hz, ms in want:
            while j < len(found) and abs(found[j][2] - hz) > 40:
                j += 1
            if j == len(found):
                print(f"   {name:13s} {hz} Hz  NOT FOUND")
                S[5]["tones"].append(dict(name=name, want_hz=hz, want_ms=ms, found=False))
                continue
            t0, t1, f, d = found[j]
            j += 1
            # The tracker's runs are on 20 ms windows: the frequency fitted over the run's middle, the length from the
            # tracker's rows (a row is the tone's while its 20 ms window is mostly the tone: from 10 ms before the
            # start to 10 ms before the end, the run then ending a window later).
            m0, m1 = int((t0 + 0.012) * fs), int((t1 - 0.022) * fs)
            if m1 - m0 > int(0.02 * fs):
                x = seg[m0:m1] - np.mean(seg[m0:m1])
                nf = 1 << 20
                sp = np.abs(np.fft.rfft(x * np.hanning(len(x)), nf))
                fr = np.fft.rfftfreq(nf, 1 / fs)
                band = np.abs(fr - f) < 40
                jj = int(np.argmax(sp * band))
                y0, y1, y2 = np.log(sp[jj - 1 : jj + 2] + 1e-12)
                f = fr[jj] + 0.5 * (y0 - y2) / (y0 - 2 * y1 + y2) * (fr[1] - fr[0])
            e = tone_env(seg, fs, f, 5)
            a_, b_ = int((t0 + 0.01) * fs), int((t1 - 0.01) * fs)  # a 20 ms window is mostly the tone 10 ms in
            s0, s1 = e[min(a_ + int(0.008 * fs), b_)], e[max(b_ - int(0.008 * fs), a_)]
            ln = (b_ - a_) / fs * 1000
            print(f"   {name:13s} {f:7.1f} Hz ({f - hz:+.1f})  {ln:4.0f} ms (step {ms})  {s0:5.0f} / {d:5.0f} / {s1:5.0f} Hz")
            S[5]["tones"].append(dict(name=name, want_hz=hz, want_ms=ms, found=True, hz=f, ms=ln, dev=d, start=s0, end=s1, t0=a_ / fs + 1.15))
    # 6: sub-tones.
    print("\n== 6: sub-tones alone (voice, DAC1 silent), at the calibration's gains")
    if 6 in mk:
        S[6] = []
        for kk, c in enumerate(CTCSS + ["DCS"]):
            x, fs2 = sub_band(dev6, fs, at(6, kk * 2.4 + 0.25), at(6, kk * 2.4 + 1.9))
            if c == "DCS":
                code, inv, baud, dv, (good, tot) = dcs(x, fs2)
                cs = f"{code:03o} {'inverted' if inv else 'normal'}" if code is not None else "not decoded"
                print(f"  DCS (023 N sent): {cs}; {baud:.2f} bit/s, {dv:.0f} Hz deviation; {good} of {tot} 23-bit windows decode as it (one in 23 if every word is right)")
                S[6].append(dict(kind="DCS", code=code, inverted=inv, baud=baud, dev=dv, windows=(good, tot)))
            else:
                f, dv = ctcss(x, fs2)
                print(f"  CTCSS {c:5.1f} Hz: {f:7.2f} Hz ({f - c:+.2f}), {dv:.0f} Hz deviation")
                S[6].append(dict(kind="CTCSS", want=c, hz=f, dev=dv))
    # 7: 4FSK.
    print("\n== 7: 4FSK, data, 60 % (M17: +-1440 / +-480 Hz at 60 %)")
    if 7 in mk:
        png = os.path.join(out_dir, stem + "-suite-eye.png")
        r = sdrtx.eye(dev6[at(7, 0) : at(7, 3.0)], fs, png)
        S[7] = r
        if r is not None and os.path.exists(png):
            R["charts"]["eye"] = png
    # The end: 1 kHz held.
    if 7 in mk:
        a = at(7, section_len(7) - 0.05)
        e = tone_env(dev6[a : a + int(1.5 * fs)], fs, 1000, 10)
        ok = len(e) > 0.5 * fs and np.median(e[int(0.2 * fs) :]) > 300
        print(f"\nend: 1 kHz {'held' if ok else 'NOT found'} after section 7" + (f" ({np.median(e[int(0.2 * fs):]):.0f} Hz deviation)" if ok else ""))
        R["hold"] = bool(ok)
    if plt:
        charts(plt, R, dev6, fs, mk, sweeps, out_dir, stem)
    return dict(ok=len(mk) == 7)


# ---- Charts and the page ----

C = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#4a3aa7", "#7a7a75"]
INK, MUTED, GRID = "#0b0b0b", "#52514e", "#e4e3df"


def style(ax):
    from matplotlib.ticker import ScalarFormatter
    for sp in ("top", "right"):
        ax.spines[sp].set_visible(False)
    for sp in ("left", "bottom"):
        ax.spines[sp].set_color(MUTED)
    ax.tick_params(colors=MUTED, labelsize=9)
    ax.grid(color=GRID, lw=0.8)
    ax.set_axisbelow(True)
    for a in (ax.xaxis, ax.yaxis):
        if a.get_scale() == "linear":
            f = ScalarFormatter(useOffset=False)
            f.set_scientific(False)
            a.set_major_formatter(f)


def save(plt, fig, path):
    fig.tight_layout()
    fig.savefig(path, dpi=110, facecolor="white")
    plt.close(fig)
    return path


def charts(plt, R, dev, fs, mk, sweeps, out_dir, stem):
    p = lambda name: os.path.join(out_dir, f"{stem}-suite-{name}.png")  # noqa: E731
    # The whole key-up: deviation per 20 ms, sections marked.
    env = amp_env(dev, fs, 20)[:: int(fs * 0.02)]
    t = np.arange(len(env)) * 0.02
    fig, ax = plt.subplots(figsize=(10, 3.2))
    ax.plot(t, env, color=C[0], lw=1)
    for n, (a, b, _) in mk.items():
        ax.axvspan(a / fs, b / fs, color=C[1], alpha=0.35, lw=0)
        ax.text(a / fs, ax.get_ylim()[1] * 0.92 if ax.get_ylim()[1] else 1, f" {n}", color=INK, fontsize=9)
    ax.set_xlabel("s from the start of the keyed part")
    ax.set_ylabel("deviation (Hz), 20 ms")
    ax.set_title("The key-up: sections 1-7 at their markers", color=INK, fontsize=11, loc="left")
    style(ax)
    R["charts"]["overview"] = save(plt, fig, p("overview"))
    if sweeps:
        fig, ax = plt.subplots(figsize=(8, 4.2))
        for i, (name, r) in enumerate(sweeps.items()):
            ax.semilogx(r["freqs"], r["db"], "o-", color=C[i], lw=2, ms=4, label=f"{name}: -3 dB {r['low'] or 0:.0f}-{r['high'] or 0:.0f} Hz")
        ax.axhline(-3, color=MUTED, lw=0.8, ls="--")
        ticks = [50, 100, 200, 300, 500, 1000, 2000, 3150, 5000, 8000]
        ax.set_xticks(ticks, [str(x) for x in ticks])
        ax.set_xticks([], minor=True)
        ax.set_ylim(max(-40, ax.get_ylim()[0]), ax.get_ylim()[1])
        ax.set_xlabel("audio frequency (Hz)")
        ax.set_ylabel("deviation, dB against 1 kHz")
        ax.set_title("Sections 1-2: DAC1 to deviation", color=INK, fontsize=11, loc="left")
        ax.legend(frameon=False, fontsize=9)
        style(ax)
        R["charts"]["sweep"] = save(plt, fig, p("sweep"))
    s4 = R["sections"].get(4)
    if s4 and s4["fit"]:
        voice = [b for b in s4["bursts"] if b["filters"] == "voice"]
        xs = np.array([b["reg40"] for b in voice])
        ys = np.array([b["dev"] for b in voice])
        fig, ax = plt.subplots(figsize=(8, 4.2))
        xx = np.linspace(xs.min() - 30, xs.max() + 30, 200)
        ax.plot(xx, s4["fit"]["f"](xx), color=C[0], lw=2, label=f"fit ({s4['fit']['kind']})")
        ax.plot(xs, ys, "o", color=C[0], ms=8, mec="white", mew=2, label="voice")
        for i, (k, v) in enumerate(s4["by_filters"].items()):
            if k != "voice":
                ax.plot([REG40_FILTERS], [v], "s", color=C[1 + i], ms=8, mec="white", mew=2, label=k)
        ax.axhline(3000, color=MUTED, lw=0.8, ls="--")
        if s4["fit"]["x3000"]:
            ax.axvline(s4["fit"]["x3000"], color=MUTED, lw=0.8, ls=":")
        ax.set_xticks(xs, [f"0x{x:03X}" for x in xs])
        ax.set_xlabel("REG_40 low 12 bits")
        ax.set_ylabel("deviation (Hz)")
        ax.set_title("Section 4: the tone generator's deviation by REG_40", color=INK, fontsize=11, loc="left")
        ax.legend(frameon=False, fontsize=9)
        style(ax)
        ax.set_xticks(xs, [f"0x{x:03X}" for x in xs])
        R["charts"]["reg40"] = save(plt, fig, p("reg40"))
        fig, ax = plt.subplots(figsize=(8, 4.2))
        for i, b in enumerate(s4["bursts"]):
            e = b["env"]
            ax.plot((np.arange(len(e)) / fs - 0.05) * 1000, e, color=C[i % len(C)], lw=1.5, label=f"0x{b['reg40']:03X} {b['filters']}")
        ax.set_xlabel("ms from the burst's onset")
        ax.set_ylabel("deviation (Hz), 10 ms")
        ax.set_title("Section 4: each burst's swell", color=INK, fontsize=11, loc="left")
        ax.legend(frameon=False, fontsize=8, ncol=2)
        style(ax)
        R["charts"]["swell"] = save(plt, fig, p("swell"))
    s5 = R["sections"].get(5)
    if s5:
        e = s5["env"]
        tt = np.arange(len(e)) / fs - 0.05
        a = s5["attack"]
        fig, ax = plt.subplots(figsize=(8, 3.8))
        ax.plot(tt * 1000, e, color=C[0], lw=2, label="1000 Hz, measured")
        tf = tt[tt > 0]
        ax.plot(tf * 1000, a["steady"] * (1 - (1 - a["start"]) * np.exp(-tf / (a["tau_ms"] / 1000))), color=C[1], lw=1.5, ls="--",
                label=f"fit: {a['start'] * 100:.0f} % at the start, tau {a['tau_ms']:.0f} ms")
        ax.set_xlabel("ms from the tone's onset")
        ax.set_ylabel("deviation (Hz), 5 ms")
        ax.set_title("Section 5: the tone generator's attack", color=INK, fontsize=11, loc="left")
        ax.legend(frameon=False, fontsize=9)
        style(ax)
        R["charts"]["attack"] = save(plt, fig, p("attack"))


def b64(path):
    with open(path, "rb") as f:
        return "data:image/png;base64," + base64.b64encode(f.read()).decode()


def html_page(R, text, stem, mhz):
    S = R["sections"]
    e = html.escape
    mk = R.get("markers", {})
    rows = []
    for n in range(1, 8):
        found = f"{mk[n][1] / R['fs']:.2f} s" if n in mk else "not found"
        rows.append(f"<tr><td>{n}</td><td>{marker_hz(n)} Hz</td><td>{e(SECTIONS[n - 1])}</td><td>{section_len(n):.2f} s</td><td>{found}</td></tr>")
    key = []
    if 1 in S:
        key.append(("Sweep, data: -3 dB", f"{S[1]['low'] or 0:.0f} Hz to {S[1]['high'] or 0:.0f} Hz"))
    if 2 in S:
        key.append(("Sweep, voice: -3 dB", f"{S[2]['low'] or 0:.0f} Hz to {S[2]['high'] or 0:.0f} Hz"))
    if 3 in S and 100 in S[3]:
        a = S[3][100]
        key.append(("DAC1 AFSK at 100 %", f"{a.get(1200, 0):.0f} / {a.get(2200, 0):.0f} Hz, twist {a.get('twist', 0):+.2f} dB"))
    if 4 in S and S[4]["fit"]:
        f = S[4]["fit"]
        key.append(("REG_40 for 3.0 kHz", f"0x{int(round(f['x3000'])):03X} ({f['kind']} fit)" if f["x3000"] else "outside the fit"))
    if 5 in S:
        a = S[5]["attack"]
        key.append(("Tone generator 1 kHz", f"{S[5]['steady']:.0f} Hz; starts at {a['start'] * 100:.0f} %, tau {a['tau_ms']:.0f} ms"))
    if 6 in S:
        d = [x for x in S[6] if x["kind"] == "DCS"]
        if d:
            key.append(("DCS decoded", f"{d[0]['code']:03o} {'inverted' if d[0]['inverted'] else 'normal'}" if d[0]["code"] is not None else "not decoded"))
    if S.get(7):
        key.append(("4FSK symbol errors", f"{S[7]['errors']} of {S[7]['symbols']}"))
    tiles = "".join(f"<div class=tile><div class=k>{e(k)}</div><div class=v>{e(v)}</div></div>" for k, v in key)
    figs = "".join(f"<figure><img alt='{e(name)}' src='{b64(path)}'></figure>" for name, path in R["charts"].items() if os.path.exists(path))
    return f"""<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1">
<title>txtest suite</title><style>
:root{{--bg:#fcfcfb;--card:#ffffff;--ink:#0b0b0b;--muted:#52514e;--line:#e4e3df;--accent:#2a78d6}}
@media (prefers-color-scheme:dark){{:root:not([data-theme=light]){{--bg:#1a1a19;--card:#232322;--ink:#ffffff;--muted:#c3c2b7;--line:#3a3a38;--accent:#3987e5}}}}
:root[data-theme=dark]{{--bg:#1a1a19;--card:#232322;--ink:#ffffff;--muted:#c3c2b7;--line:#3a3a38;--accent:#3987e5}}
body{{margin:0;background:var(--bg);color:var(--ink);font:15px/1.5 system-ui,-apple-system,sans-serif}}
main{{max-width:1000px;margin:0 auto;padding:24px 16px}}h1{{font-size:22px;margin:0 0 4px}}p.sub{{color:var(--muted);margin:0 0 20px}}
.tiles{{display:grid;grid-template-columns:repeat(auto-fit,minmax(200px,1fr));gap:10px;margin:16px 0}}
.tile{{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:10px 12px}}.k{{color:var(--muted);font-size:13px}}.v{{font-weight:600}}
table{{border-collapse:collapse;width:100%;background:var(--card);font-size:14px}}td,th{{border-bottom:1px solid var(--line);padding:6px 8px;text-align:left}}
.wrap{{overflow-x:auto}}figure{{margin:16px 0;background:#fff;border-radius:8px;padding:8px}}img{{max-width:100%;height:auto;display:block}}
pre{{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:12px;overflow-x:auto;font-size:12px;line-height:1.4}}
</style></head><body><main>
<h1>txtest suite</h1><p class=sub>{e(stem)}{f' at {mhz:.4f} MHz' if mhz else ''}; keyed {R.get('keyed_s', 0):.1f} s, carrier offset {R.get('offset', 0):+.0f} Hz; TERM-34</p>
<div class=tiles>{tiles}</div>
<h2>Sections</h2><div class=wrap><table><tr><th>n</th><th>marker</th><th>what</th><th>length</th><th>marker ends</th></tr>{''.join(rows)}</table></div>
<h2>Charts</h2>{figs}
<h2>Report</h2><pre>{e(text)}</pre></main></body></html>"""


# ---- Selftest: the whole suite as synthetic IQ ----


class Synth:
    """The suite's deviation (Hz) at any time, as a model radio makes it: DAC1 through a data path (first-order
    high-pass at 188 Hz) or a voice path (second-order at 300 Hz), both with an 8th-order 3.1 kHz wall and 3.0 kHz
    for the APRS level, inverted; the tone generator's deviation exponential in REG_40 through 0x387 1660 Hz and
    0x50A 4518 Hz, `flat` halving it, each tone swelling from a quarter (tau 40 ms), the flags' switches up to 50 us
    late; CTCSS 500 Hz, DCS +-400 Hz NRZ (a 1 positive); 4FSK ideal at 60 %."""

    K = 3000 / AFSK_DAC
    TAU, START = 0.040, 0.25
    TONE_DEV = 2000.0
    SUB_DEV, DCS_DEV = 500.0, 400.0

    def __init__(self, rng):
        self.st = STEPS
        self.t = np.array([s["t"] for s in STEPS])
        self.rng = rng
        nb = 1300 * 3
        flag = (0x7E >> (np.arange(nb) % 8)) & 1
        self.mark = (np.cumsum(flag == 0) % 2) == 0
        self.jit = rng.uniform(0, 50e-6, nb + 1)
        self.jit[0] = 0
        pre = 192
        self.sym = np.concatenate([np.tile([3, -3], pre // 2), np.resize(sdrtx.prbs_symbols(), 4800 * 4)])
        self.dcsw = dcs_codeword(DCS)

    @staticmethod
    def H(f, filt):
        f = np.asarray(f, float)
        wall = 1 / np.sqrt(1 + (f / 3100) ** 16)
        if filt == "voice":
            hp = (f / 300) ** 2 / np.sqrt(1 + (f / 300) ** 4)
        elif filt == "flat":
            hp = 1 / np.sqrt(1 + (f / 150) ** 2)
        else:
            hp = (f / 188) / np.sqrt(1 + (f / 188) ** 2)
        return hp * wall

    @staticmethod
    def D(reg40):
        b = np.log(4518 / 1660) / (0x50A - 0x387)
        return 1660 * np.exp(b * (reg40 - 0x387))

    def dac_gain(self, f, filt):
        return -self.K * self.H(f, filt) / self.H(1000, "data")

    def fsk_phase(self, tt, f_of_bit, jitter):
        edges = np.arange(len(self.mark) + 1) / 1200 + (self.jit if jitter else 0)
        hz = np.where(f_of_bit, 1200.0, 2200.0)
        ph0 = np.concatenate([[0], np.cumsum(2 * np.pi * hz * np.diff(edges))])
        b = np.clip(np.searchsorted(edges, tt, side="right") - 1, 0, len(hz) - 1)
        return ph0[b] + 2 * np.pi * hz[b] * (tt - edges[b]), hz[b]

    def step_dev(self, s, tt):
        k, arg, filt = s["kind"], s["arg"], s["filters"]
        if k in ("sil", "bkquiet"):
            return np.zeros_like(tt)
        if k == "tone":
            return MARKER_AMP * self.dac_gain(arg, "data") * np.sin(2 * np.pi * arg * tt)
        if k == "sweep":
            i = np.floor(tt / (SWEEP_TONE + SWEEP_GAP)).astype(int)
            r = tt - i * (SWEEP_TONE + SWEEP_GAP)
            on = (i < 23) & (r < SWEEP_TONE)
            f = np.array(sdrtx.SWEEP_HZ + [0])[np.minimum(i, 23)]
            return np.where(on, SWEEP_AMP * self.dac_gain(np.maximum(f, 1), filt) * np.sin(2 * np.pi * f * r), 0)
        if k == "afsk":
            ph, hz = self.fsk_phase(tt, self.mark, False)
            return AFSK_DAC * arg / 100 * self.dac_gain(hz, filt) * np.sin(ph)
        if k == "bkflags":
            ph, _ = self.fsk_phase(tt, self.mark, True)
            amp = self.D(s["reg40"]) * (0.5 if filt == "flat" else 1.0)
            return amp * (1 - (1 - self.START) * np.exp(-tt / self.TAU)) * np.sin(ph)
        if k == "bktone":
            return self.TONE_DEV * (1 - (1 - self.START) * np.exp(-tt / self.TAU)) * np.sin(2 * np.pi * arg * tt)
        if k == "sub":
            if arg & 0x8000:
                b = np.floor(tt * 134.4).astype(int) % 23
                return np.where((self.dcsw >> b) & 1, self.DCS_DEV, -self.DCS_DEV)
            return self.SUB_DEV * np.sin(2 * np.pi * arg / 10 * tt)
        if k == "fsk4":
            u = 800 * arg / 100
            x = tt * 4800
            out = np.zeros_like(tt)
            k0 = np.floor(x).astype(int)
            for j in range(-6, 7):
                kk = k0 + j
                ok = kk >= 0
                out += np.where(ok, self.sym[np.clip(kk, 0, len(self.sym) - 1)] * sdrtx.rrc(x - kk), 0)
            return u * out
        raise ValueError(k)

    def dev(self, t):
        out = np.zeros_like(t)
        i = np.searchsorted(self.t, t, side="right") - 1
        for j in np.unique(i):
            m = i == j
            if j < 0:
                continue
            if j >= len(self.st) or t[m][0] >= SUITE_S:
                mm = m & (t >= SUITE_S)
                out[mm] = MARKER_AMP * self.dac_gain(1000, "data") * np.sin(2 * np.pi * 1000 * (t[mm] - SUITE_S))
                m = m & (t < SUITE_S)
                if not m.any():
                    continue
            s = self.st[j]
            out[m] = self.step_dev(s, t[m] - s["t"])
        return out


def synth_iq(rng, ppm=25.0, offset=700.0, pre=1.0, hold=1.5, post=0.5, chunk=1 << 20):
    """The whole suite as rtl_sdr's u8 IQ at sdrtx.FS: noise, the key-up, the suite on the radio's clock (`ppm` fast
    against the dongle's), 1 kHz held, the release."""
    fs = sdrtx.FS
    sy = Synth(rng)
    n = int((pre + SUITE_S / (1 + ppm * 1e-6) + hold + post) * fs)
    key0, key1 = int(pre * fs), int((pre + SUITE_S / (1 + ppm * 1e-6) + hold) * fs)
    raw = np.empty(2 * n, np.uint8)
    ph = 0.0
    for a in range(0, n, chunk):
        idx = np.arange(a, min(a + chunk, n))
        t = (idx - key0) / fs * (1 + ppm * 1e-6)
        on = (idx >= key0) & (idx < key1)
        d = np.where(on, sy.dev(np.maximum(t, -1)) + offset, 0) - sdrtx.OFF
        p = ph + 2 * np.pi * np.cumsum(d) / fs
        ph = p[-1] % (2 * np.pi)
        x = np.where(on, 60.0, 0.0) * np.exp(1j * p) + rng.normal(0, 1.5, len(idx)) + 1j * rng.normal(0, 1.5, len(idx))
        raw[2 * a : 2 * idx[-1] + 2 : 2] = np.clip(np.round(x.real + 127.5), 0, 255)
        raw[2 * a + 1 : 2 * idx[-1] + 2 : 2] = np.clip(np.round(x.imag + 127.5), 0, 255)
    return raw, sy


def selftest(out_dir):
    rng = np.random.default_rng(7)
    print("== selftest: the whole suite synthesised as IQ (radio clock +25 ppm, carrier +700 Hz), then measured")
    raw, sy = synth_iq(rng)
    R = analyse(raw, out_dir, "selftest")
    S = R["sections"]
    ok = True
    lines = []

    def check(what, cond, got):
        nonlocal ok
        ok &= bool(cond)
        lines.append(f"  [{'pass' if cond else 'FAIL'}] {what}: {got}")

    check("section table: 59.95 s, sections in order", abs(STEPS[-1]["t"] + STEPS[-1]["s"] - SUITE_S) < 1e-9 and [marker_end_t(n) for n in range(1, 8)] == sorted(marker_end_t(n) for n in range(1, 8)),
          f"{STEPS[-1]['t'] + STEPS[-1]['s']:.3f} s")
    mk = R["markers"]
    check("all seven markers found", sorted(mk) == list(range(1, 8)), sorted(mk))
    if 1 in mk:
        errs = [abs((mk[n][1] - mk[1][1]) / R["fs"] / (1 + 25e-6) ** -1 - (marker_end_t(n) - marker_end_t(1))) * 1000 for n in mk]
        check("marker ends within 5 ms of the table (the clock's ppm taken out)", max(errs) < 5, f"{max(errs):.1f} ms")
        check("timing: the radio's +25 ppm", abs(R.get("timing_ppm", 0) - 25) < 15, f"{R.get('timing_ppm', 0):+.0f} ppm")
    fr = sdrtx.SWEEP_HZ
    for n, filt in ((1, "data"), (2, "voice")):
        if n not in S:
            check(f"sweep {filt}", False, "missing")
            continue
        want_db = 20 * np.log10(Synth.H(np.array(fr), filt) / Synth.H(1000, filt))
        lo, hi = sdrtx.minus3(fr, want_db)
        r = S[n]
        check(f"sweep {filt}: low -3 dB near the model's {lo:.0f} Hz", r["low"] and abs(r["low"] - lo) < 0.05 * lo, f"{r['low']:.0f} Hz")
        check(f"sweep {filt}: high -3 dB near the model's {hi:.0f} Hz", r["high"] and abs(r["high"] - hi) < 60, f"{r['high']:.0f} Hz")
        worst = max(abs(a - b) for a, b in zip(r["db"], want_db) if a > -25)
        check(f"sweep {filt}: every tone within 0.3 dB of the model (to -25 dB)", worst < 0.3, f"{worst:.2f} dB")
    if 3 in S:
        for lvl in (100, 50):
            a = S[3][lvl]
            want = 3000 * lvl / 100 * Synth.H(1200, "data") / Synth.H(1000, "data")
            check(f"DAC1 AFSK {lvl} %: mark near {want:.0f} Hz", abs(a.get(1200, 0) - want) < 0.03 * want, f"{a.get(1200, 0):.0f} Hz")
            check(f"DAC1 AFSK {lvl} %: on-time switches (jitter under 10 us rms)", a.get("jitter_rms_us", 99) < 10, f"{a.get('jitter_rms_us', 0):.1f} us")
    if 4 in S:
        b = S[4]["bursts"]
        worst = max(abs(x["dev"] / (Synth.D(x["reg40"]) * (0.5 if x["filters"] == "flat" else 1)) - 1) for x in b)
        check("REG_40 bursts: each within 3 % of the model", worst < 0.03, f"{worst * 100:.1f} %")
        want3 = 0x387 + np.log(3000 / 1660) / (np.log(4518 / 1660) / (0x50A - 0x387))
        x3 = S[4]["fit"]["x3000"]
        check(f"REG_40 for 3.0 kHz near the model's {want3:.0f}", x3 and abs(x3 - want3) < 8, f"{x3:.0f} ({S[4]['fit']['kind']})")
        r = S[4]["by_filters"]
        check("flat halves 0x43C", abs(r["flat"] / r["voice"] - 0.5) < 0.03, f"{r['flat'] / r['voice']:.3f}")
        jit = [x.get("jitter_rms_us", 99) for x in b]
        check("tone generator switches: jitter near the model's 14 us rms", all(8 < j < 22 for j in jit), f"{min(jit):.1f}-{max(jit):.1f} us")
        sw = [x["swell"]["tau_ms"] for x in b]
        check("bursts' swell: tau near 40 ms", all(32 < t_ < 48 for t_ in sw), f"{min(sw):.0f}-{max(sw):.0f} ms")
        st = [x["swell"]["start"] for x in b]
        check("bursts' swell: starts near a quarter", all(abs(v - 0.25) < 0.06 for v in st), f"{min(st):.2f}-{max(st):.2f}")
    if 5 in S:
        a = S[5]["attack"]
        check("1 kHz steady level", abs(S[5]["steady"] - Synth.TONE_DEV) < 0.03 * Synth.TONE_DEV, f"{S[5]['steady']:.0f} Hz")
        check("1 kHz attack: tau 40 ms", abs(a["tau_ms"] - 40) < 10, f"{a['tau_ms']:.0f} ms")
        check("1 kHz attack: starts at a quarter", abs(a["start"] - 0.25) < 0.08, f"{a['start']:.2f}")
        t = S[5]["tones"]
        check("end tones: all nine steps found", all(x["found"] for x in t) and len(t) == 9, sum(x["found"] for x in t))
        if all(x["found"] for x in t):
            fe = max(abs(x["hz"] - x["want_hz"]) for x in t)
            le = max(abs(x["ms"] - x["want_ms"]) for x in t)
            check("end tones: frequencies within 1 Hz", fe < 1, f"{fe:.2f} Hz")
            check("end tones: lengths within 10 ms", le < 10, f"{le:.0f} ms")
            q = t[-1]
            check("Quindar swells: end over start", q["end"] > 1.8 * q["start"], f"{q['start']:.0f} -> {q['end']:.0f} Hz")
    if 6 in S:
        c = [x for x in S[6] if x["kind"] == "CTCSS"]
        fe = max(abs(x["hz"] - x["want"] * (1 + 25e-6)) for x in c)
        de = max(abs(x["dev"] / Synth.SUB_DEV - 1) for x in c)
        check("CTCSS frequencies within 0.1 Hz", fe < 0.1, f"{fe:.3f} Hz")
        check("CTCSS deviation within 5 %", de < 0.05, f"{de * 100:.1f} %")
        d = [x for x in S[6] if x["kind"] == "DCS"][0]
        check("DCS 023 normal decoded", d["code"] == DCS and not d["inverted"], f"{d['code']:03o} {'inverted' if d['inverted'] else 'normal'}" if d["code"] is not None else "none")
        check("DCS 134.4 bit/s", abs(d["baud"] - 134.4) < 0.5, f"{d['baud']:.2f}")
    if S.get(7):
        r = S[7]
        check("4FSK levels within 3 % of +-480 / +-1440", np.all(np.abs(r["levels"] - [-1440, -480, 480, 1440]) < 0.03 * 1440), np.round(r["levels"]))
        check("4FSK: no symbol errors", r["errors"] == 0, r["errors"])
    else:
        check("4FSK measured", False, "missing")
    check("the 1 kHz hold after section 7", R.get("hold"), R.get("hold"))
    check("the page written, charts embedded", os.path.exists(R["html"]) and os.path.getsize(R["html"]) > 50_000 and "data:image/png;base64" in open(R["html"]).read(), R["html"])
    print("\n".join(lines))
    print("selftest:", "all pass" if ok else "FAILED")
    return ok


def main():
    a = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    a.add_argument("mhz", type=float)
    a.add_argument("--seconds", type=float, default=70, help="capture length (the suite is 60 s, then 1 kHz until release)")
    a.add_argument("--gain", type=float, default=0, help="tuner gain dB (0 = least; the radio is close)")
    a.add_argument("--ppm", type=int, default=0)
    a.add_argument("--save")
    a.add_argument("--load")
    a.add_argument("--out", help="where the report, page and PNGs go (default: next to --load/--save's file)")
    a.add_argument("--selftest", action="store_true", help="the whole suite as synthetic IQ, measured and checked")
    o = a.parse_args()
    if o.selftest:
        d = o.out or tempfile.mkdtemp(prefix="sdrsuite-")
        sys.exit(0 if selftest(d) else 1)
    raw = np.fromfile(o.load, np.uint8) if o.load else sdrtx.capture(o)
    ref = o.load or o.save
    out = o.out or (os.path.dirname(os.path.abspath(ref)) if ref else os.getcwd())
    stem = os.path.splitext(os.path.basename(ref))[0] if ref else "sdrsuite"
    R = analyse(raw, out, stem, o.mhz)
    sys.exit(0 if R.get("ok") else 1)


if __name__ == "__main__":
    main()
