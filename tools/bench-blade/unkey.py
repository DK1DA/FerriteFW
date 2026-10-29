"""bladeRF sc16q11 capture -> carrier envelope and AFSK activity per ms; prints each key-up's timeline:
carrier up, first/last AFSK tone, carrier down, and the hold (last tone to carrier down)."""
import sys
import numpy as np

path, fs = sys.argv[1], float(sys.argv[2])
shift = float(sys.argv[3]) if len(sys.argv) > 3 else 0.0  # the signal's offset from the tuned centre, Hz
iq = np.fromfile(path, dtype=np.int16).astype(np.float32).reshape(-1, 2)
x = iq[:, 0] + 1j * iq[:, 1]
x -= x.mean()
if shift:
    x *= np.exp(-2j * np.pi * shift / fs * np.arange(len(x)))
    # keep +-25 kHz around the signal: a moving average low-pass, then decimate by 10
    k = 40
    x = np.convolve(x, np.ones(k) / k, mode="same")[::10]
    fs /= 10
ms = int(fs / 1000)
n = len(x) // ms
p = (np.abs(x[: n * ms]) ** 2).reshape(n, ms).mean(1)
floor = np.median(p)
carrier = p > floor * 20
# FM demod, decimate to ~25 kHz audio, AFSK energy (1200/2200 Hz band) per ms vs the rest
d = np.angle(x[1:] * np.conj(x[:-1]))
dec = int(fs // 25000)
a = d[: (len(d) // dec) * dec].reshape(-1, dec).mean(1)
afs = fs / dec
am = int(afs / 1000)
seg = a[: (len(a) // am) * am].reshape(-1, am)
# tone power: AFSK deviates ~3 kHz, so the demod's ac swing is large; an unmodulated carrier is flat
swing = seg.std(1)
m = min(len(swing), n)
tone = swing[:m] > np.median(swing[:m][carrier[:m]]) * 0.5 if carrier[:m].any() else np.zeros(m, bool)
print(f"{n} ms captured, floor {10*np.log10(floor+1e-9):.1f} dB, carrier ms {carrier.sum()}")
i = 0
while i < m:
    if carrier[i]:
        j = i
        while j < m and carrier[j]:
            j += 1
        if j - i > 50:
            t = np.where(tone[i:j])[0]
            # the modulated part: the long run of tone ms
            if len(t):
                first, last = i + t[0], i + t[-1]
                print(f"key-up {i} ms: carrier {j - i} ms, tones {first - i}..{last - i} ms, carrier after the last tone {j - last} ms")
            else:
                print(f"key-up {i} ms: carrier {j - i} ms, no tones found")
        i = j
    i += 1
