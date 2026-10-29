"""airfreq.py CAP.u8 [STEP_S]: the carrier's mean frequency offset (Hz, against the first keyed second) per step."""
import sys
import numpy as np
raw = np.fromfile(sys.argv[1], np.uint8); step = float(sys.argv[2]) if len(sys.argv) > 2 else 0.25
iq = (raw.astype(np.float32) - 127.5).reshape(-1, 2); z = iq[:, 0] + 1j * iq[:, 1]
fs0 = 1_024_000; z = z * np.exp(-2j * np.pi * (-250_000) * np.arange(len(z)) / fs0)
dec = 16; z = np.convolve(z, np.ones(64) / 64, mode="same")[::dec]; fs = fs0 / dec
dev = np.angle(z[1:] * np.conj(z[:-1])) * fs / (2 * np.pi); amp = np.abs(z[1:]); on = amp > 0.3 * np.percentile(amp, 99)
n = int(fs * step); ref = None
for i in range(0, len(dev) - n, n):
    if on[i:i + n].mean() < 0.9: continue
    m = np.median(dev[i:i + n])
    ref = m if ref is None else ref
    print(f"{i / fs:6.2f} s  {m - ref:+8.0f} Hz")
