"""Bit errors of fmgen.py's PRBS9 data signals in the radio's receive audio (a WAV from `ferrite_link audio`).

  rxdata.py FILE.wav fsk BAUD [eq]              # direct FSK / GMSK: the audio is the baseband, integrated over a bit
  rxdata.py FILE.wav fsk4 SYM [eq]              # 4FSK (fmgen's fsk4): an RRC 0.5 matched filter, 3 thresholds
  rxdata.py FILE.wav afsk BAUD MARK SPACE [eq]  # tone energies over a bit, mark minus space

A plain receiver, the best timing phase and pattern offset found by search in each chunk. Prints the bit error rate
with and without a running DC removal (the receive audio's high-pass leaves NRZ data wandering), and an eye figure:
the decisions' mean distance from their threshold over their spread (above ~3 is clean).

- The WAV is brought to 48 kHz by FFT (linear interpolation from 16 kHz loses the top: -3.5 dB at 6 kHz).
- Chunks touching a gap (the stream's dropped blocks, filled with zeros) are skipped and counted.
- `hp`: undoes the board's two coupling high-passes (R16: first order at 450 and 220 Hz, within 1 dB of the
  measured low end) in level and phase, their poles moved to 15 Hz. Combine with `eq` for the top.
- `eq`: a receive equaliser, the inverse of black's measured response (bench log R16: route 9, REG_43 5A20,
  `docs/research/rx-meter-oct28/r9if5.txt`), boost capped at +12 dB, flat above 7 kHz, nothing below 60 Hz.
"""
import sys, wave
import numpy as np

FS = 48_000
# R16, route 9 + 5A20: dB against 1 kHz
R16 = {50: -36.7, 80: -27.1, 100: -22.2, 125: -17.9, 160: -14.1, 200: -11.3, 250: -9.0, 315: -6.9, 400: -5.0,
       500: -3.4, 630: -2.0, 800: -0.8, 1000: 0.0, 1250: 0.6, 1600: 0.9, 2000: 0.9, 2500: 0.6, 3150: 0.0,
       4000: -1.1, 5000: -2.5, 6300: -4.7, 8000: -8.4, 10000: -15.8}


def prbs9(n):
    s, out = 0x1FF, np.empty(n, np.int8)
    for i in range(n):
        b = ((s >> 8) ^ (s >> 4)) & 1
        s = ((s << 1) | b) & 0x1FF
        out[i] = b
    return out


def mavg(x, n):
    n = max(int(n), 1)
    c = np.cumsum(np.concatenate([[0.0], x]))
    return (c[n:] - c[:-n]) / n


def resample(x, fs):
    n2 = int(round(len(x) * FS / fs))
    X = np.fft.rfft(x)
    Y = np.zeros(n2 // 2 + 1, complex)
    k = min(len(X), len(Y))
    Y[:k] = X[:k]
    return np.fft.irfft(Y, n2) * n2 / len(x)


def equalise(x, top_only=False):
    f = np.fft.rfftfreq(len(x), 1 / FS)
    fk = np.array(sorted(R16)); dk = np.array([R16[k] for k in fk])
    resp = np.interp(np.log10(np.clip(f, 50, 10000)), np.log10(fk), dk)
    resp[f > 7000] = np.interp(np.log10(7000), np.log10(fk), dk)
    g = 10 ** (np.clip(-resp, -20, 12) / 20)
    if top_only:
        g[f < 1000] = 1  # `hp` has the low end
    else:
        g[f < 60] = 0
    return np.fft.irfft(np.fft.rfft(x) * g, len(x))


def unhp(x, poles=(450.0, 220.0), new=15.0):
    """Each first-order high-pass s/(s+a) undone by (s+a)/(s+b): a bilinear first-order section per pole."""
    for a_hz in poles:
        k = 2 * FS
        a, b = 2 * np.pi * a_hz, 2 * np.pi * new
        b0, b1 = (k + a) / (k + b), (a - k) / (k + b)
        a1 = (b - k) / (k + b)
        y = np.empty_like(x); xp = yp = 0.0
        for i, v in enumerate(x):
            yp = b0 * v + b1 * xp - a1 * yp
            xp = v; y[i] = yp
        x = y - y.mean()
    return x


def rrc(sps, beta, span=8):
    t = np.arange(-span * sps, span * sps + 1) / sps
    h = np.empty(len(t))
    for i, v in enumerate(t):
        if abs(v) < 1e-9:
            h[i] = 1 + beta * (4 / np.pi - 1)
        elif abs(abs(4 * beta * v) - 1) < 1e-9:
            h[i] = beta / np.sqrt(2) * ((1 + 2 / np.pi) * np.sin(np.pi / (4 * beta)) + (1 - 2 / np.pi) * np.cos(np.pi / (4 * beta)))
        else:
            h[i] = (np.sin(np.pi * v * (1 - beta)) + 4 * beta * v * np.cos(np.pi * v * (1 + beta))) / (np.pi * v * (1 - (4 * beta * v) ** 2))
    return h / h.sum()


def best_shift(v, ref):
    """Circular correlation of the chunk's decisions with every shift of the 511-long reference."""
    n = len(ref)
    R = np.fft.fft(ref)
    best = (-1e18, 0, 1)
    for pol in (1, -1):
        # fold the chunk onto the reference's period: sum of v[i] at i mod 511
        fold = np.zeros(n)
        np.add.at(fold, np.arange(len(v)) % n, v * pol)
        c = np.real(np.fft.ifft(np.conj(np.fft.fft(fold)) * R))  # c[m] = sum v[i] ref[i + m]
        m = int(np.argmax(c))
        if c[m] > best[0]:
            best = (c[m], m, pol)
    return best[1], best[2]


def main():
    path, kind, rate = sys.argv[1], sys.argv[2], float(sys.argv[3])
    eq, hp = "eq" in sys.argv[4:], "hp" in sys.argv[4:]
    with wave.open(path) as w:
        fs = w.getframerate()
        x0 = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(float)
    # gaps: 30 or more exact zeros at the file's rate
    z = (x0 == 0).astype(int)
    run = mavg(z, 30) >= 1.0
    gap = np.zeros(len(x0), bool); gap[:len(run)] |= run
    for k in range(1, 30):
        gap[k:k + len(run)] |= run
    x = resample(x0 - x0.mean(), fs)
    gap = np.interp(np.arange(len(x)) * fs / FS, np.arange(len(x0)), gap.astype(float)) > 0
    if hp:
        x = unhp(x)
    if eq:
        x = equalise(x, top_only=hp)
    sp = FS / rate
    env = mavg(np.abs(x), FS // 20)
    live = np.where(env > env.max() * 0.3)[0]
    a, b = live[0] + int(0.3 * FS), live[-1] - int(0.3 * FS)
    x, gap = x[a:b], gap[a:b]
    if kind == "afsk":
        mark, space = float(sys.argv[4]), float(sys.argv[5])
        t = np.arange(len(x)) / FS
        em = np.abs(mavg(x * np.exp(-2j * np.pi * mark * t), sp))
        es = np.abs(mavg(x * np.exp(-2j * np.pi * space * t), sp))
        soft_raw = em - es
        soft_dc = em / em.mean() - es / es.mean()
    elif kind == "fsk":
        soft_raw = mavg(x, sp)
        L = int(sp * 16)
        soft_dc = soft_raw - np.convolve(soft_raw, np.ones(L) / L, mode="same")
    elif kind == "fsk4":
        soft_raw = np.convolve(x, rrc(sp, 0.5), mode="same")
        L = int(sp * 32)
        soft_dc = soft_raw - np.convolve(soft_raw, np.ones(L) / L, mode="same")
    else:
        sys.exit(__doc__)
    if kind == "fsk4":
        bits = prbs9(1022)
        lv = {(0, 1): 3, (0, 0): 1, (1, 0): -1, (1, 1): -3}
        ref = np.array([lv[(bits[2 * i], bits[2 * i + 1])] for i in range(511)], float)
        gray = {3: (0, 1), 1: (0, 0), -1: (1, 0), -3: (1, 1)}
    else:
        ref = prbs9(511) * 2 - 1.0
    C = 256  # symbols per chunk: timing and pattern offset found in each (the stream's timing slips between them)
    print(f"{path.rsplit('/', 1)[-1]}: {kind} {rate:g}{' hp' if hp else ''}{' eq' if eq else ''}")
    for name, soft in (("plain", soft_raw), ("corrected", soft_dc)):
        errs, nbits, eyes, skipped = 0, 0, [], 0
        for c0 in range(0, int(len(soft) / sp) - C - 1, C):
            span = slice(int(c0 * sp), int((c0 + C + 1) * sp))
            if gap[span].any():
                skipped += 1
                continue
            best = None
            for ph in np.linspace(0, sp, 12, endpoint=False):
                idx = (ph + (c0 + np.arange(C)) * sp).astype(int)
                v = soft[idx]
                m, pol = best_shift(v, ref)
                r = np.resize(np.roll(ref, -m), C)
                v = v * pol
                if kind == "fsk4":
                    k = np.dot(v, r) / np.dot(r, r)  # the level of +1
                    d = np.clip(np.round((v / k - 1) / 2) * 2 + 1, -3, 3)  # nearest of -3 -1 1 3
                    e = sum(int(gray[int(p)][0] != gray[int(q)][0]) + int(gray[int(p)][1] != gray[int(q)][1]) for p, q in zip(d, r))
                    dist = np.abs(v / k - np.where(np.abs(r) == 3, 2 * np.sign(r), 0))  # distance from the nearest threshold
                    eye = dist.mean() / max(dist.std(), 1e-9)
                    nb = 2 * C
                else:
                    h = np.sign(v)
                    e = int(np.sum(h != r))
                    d = v * r
                    eye = d.mean() / max(d.std(), 1e-9)
                    nb = C
                if best is None or e < best[0]:
                    best = (e, eye, nb)
            errs += best[0]; nbits += best[2]; eyes.append(best[1])
        sk = f", {skipped} chunks skipped (gaps)" if skipped else ""
        print(f"  {name:9s}: BER {errs / max(nbits, 1):.4f} over {nbits} bits, eye median {np.median(eyes) if eyes else 0:.1f}, worst {min(eyes) if eyes else 0:.1f}{sk}")


main()
