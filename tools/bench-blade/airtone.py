"""airtone.py CAP.u8 [HZ]: the transmitted audio's level at HZ (default 740), per second, from a bladecap capture."""
import sys
import numpy as np
sys.path.insert(0, "/Users/infamy/Code/rt950pro/ferrite/tools")
import sdrtx
raw = np.fromfile(sys.argv[1], np.uint8); hz = float(sys.argv[2]) if len(sys.argv) > 2 else 740.0
iq = (raw.astype(np.float32) - 127.5).reshape(-1, 2); z = iq[:, 0] + 1j * iq[:, 1]
fs0 = 1_024_000; z = z * np.exp(-2j * np.pi * (-250_000) * np.arange(len(z)) / fs0)   # the signal sits 250 kHz below the centre
dec = 16; k = np.ones(dec) / dec
z = np.convolve(z, np.ones(64) / 64, mode="same")[::dec]; fs = fs0 / dec
dev = np.angle(z[1:] * np.conj(z[:-1])) * fs / (2 * np.pi)
amp = np.abs(z[1:]); on = amp > 0.3 * np.percentile(amp, 99)
n = int(fs)
for i in range(0, len(dev) - n, n):
    seg = dev[i:i + n]; keyed = on[i:i + n].mean() > 0.9
    if not keyed: print(f"{i / fs:5.1f} s  (no carrier)"); continue
    seg = seg - seg.mean(); w = np.hanning(len(seg)); X = np.abs(np.fft.rfft(seg * w)) * 2 / w.sum(); f = np.fft.rfftfreq(len(seg), 1 / fs)
    line = X[(f > hz - 2) & (f < hz + 2)].max(); floor = np.median(X[(f > 300) & (f < 3000)])
    print(f"{i / fs:5.1f} s  {hz:.0f} Hz at {line:7.1f} Hz deviation (floor {floor:5.2f})")
