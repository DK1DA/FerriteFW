"""bbdemod.py IN.wav OUT.wav [LO_HZ HI_HZ]: FM demodulation in software of a baseband route's real low-IF capture: the
analytic signal (positive frequencies only, LO-HI kept), its centre found from the mean instantaneous frequency, the
instantaneous frequency in Hz written as a 48 kHz WAV (1 Hz = 4 counts) for rxdata.py."""
import sys, wave
import numpy as np
w = wave.open(sys.argv[1]); fs = w.getframerate(); x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(float)
lo, hi = (float(sys.argv[3]), float(sys.argv[4])) if len(sys.argv) > 4 else (300.0, fs / 2 - 200)
X = np.fft.fft(x - x.mean()); f = np.fft.fftfreq(len(x), 1 / fs)
X[(f < lo) | (f > hi)] = 0
z = np.fft.ifft(X * 2)
# where the carrier is: the analytic signal's envelope, smoothed over 50 ms, above half its 95th percentile
env = np.convolve(np.abs(z), np.ones(fs // 20) / (fs // 20), mode="same")
on = np.where(env > 0.5 * np.percentile(env, 95))[0]
z = z[on[0] + fs // 4: on[-1] - fs // 4]
print(f"carrier {len(z) / fs:.1f} s, in-band level {20 * np.log10(np.abs(z).mean() / 32768):.1f} dBFS")
inst = np.angle(z[1:] * np.conj(z[:-1])) * fs / (2 * np.pi)
fc = np.median(inst); inst -= fc
print(f"centre {fc:.0f} Hz, deviation rms {inst.std():.0f} Hz")
n2 = int(round(len(inst) * 48000 / fs)); I = np.fft.rfft(inst); Y = np.zeros(n2 // 2 + 1, complex); k = min(len(I), len(Y)); Y[:k] = I[:k]
y = np.fft.irfft(Y, n2) * n2 / len(inst)
with wave.open(sys.argv[2], "wb") as o:
    o.setnchannels(1); o.setsampwidth(2); o.setframerate(48000); o.writeframes(np.clip(y * 4, -32767, 32767).astype(np.int16).tobytes())
