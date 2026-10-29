"""txtones.py REF.u8 CAP.u8...: each third-octave tone's deviation (Hz, best 0.2 s window) in bladecap captures of a
txtest sweep, and dB against REF's 1 kHz (the plain sweep: the 0 dB target)."""
import sys
import numpy as np
THIRD = [50, 63, 80, 100, 125, 160, 200, 250, 315, 400, 500, 630, 800, 1000, 1250, 1600, 2000, 2500, 3150, 4000, 5000, 6300, 8000]
def audio(p):
    iq = np.fromfile(p, np.uint8).astype(np.float32) - 127.5
    x = iq[0::2] + 1j * iq[1::2]
    fs = 1.024e6
    x *= np.exp(2j * np.pi * 250e3 / fs * np.arange(len(x)))
    x = np.convolve(x, np.ones(16) / 16, mode="same")[::16]
    fs /= 16
    a = np.angle(x[1:] * np.conj(x[:-1])) * fs / (2 * np.pi)
    return a - np.median(a), fs
def dev(a, fs, hz):
    win, hop = int(0.2 * fs), int(0.05 * fs)
    t = np.arange(win) / fs
    w = np.hanning(win)
    e = np.exp(-2j * np.pi * hz * t) * w
    return max(2 * abs(np.dot(a[i:i + win], e)) / w.sum() for i in range(0, len(a) - win, hop))
ref = None
for p in sys.argv[1:]:
    a, fs = audio(p)
    d = {hz: dev(a, fs, hz) for hz in THIRD}
    if ref is None:
        ref = d[1000]
    print(p.rsplit('/', 1)[-1], f"(1 kHz {d[1000]:.0f} Hz):", ' '.join(f"{hz}:{20 * np.log10(max(v, 1) / ref):+.1f}" for hz, v in d.items()))
