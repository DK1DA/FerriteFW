"""Measure a transmission on an RTL-SDR: carrier offset, FM deviation and the audio tones (bench checks, TX-*,
APRS-*). Captures `seconds` of IQ 250 kHz off the frequency (away from the dongle's DC spike), finds the keyed part by
power, FM-demodulates it and prints what it measured; `--tones` lists the strongest audio tones with their deviation.

    sdrtx.py 145.495 --seconds 15 [--gain 0] [--ppm 0] [--save cap.u8] [--load cap.u8]

Needs librtlsdr (`rtl_sdr` on the PATH) and numpy (the project's .venv). The dongle's own crystal error (often
+-30 ppm, about +-4 kHz at 145 MHz) is in the offset unless --ppm corrects it; the deviation and tones don't depend on it.
"""
import argparse, subprocess, sys
import numpy as np

FS = 1_024_000
OFF = 250_000
DEC = 16  # 64 kS/s after the low-pass


def lowpass(cut, fs, taps=255):
    n = np.arange(taps) - (taps - 1) / 2
    h = np.sinc(2 * cut / fs * n) * np.hamming(taps)
    return h / h.sum()


def main():
    a = argparse.ArgumentParser()
    a.add_argument("mhz", type=float)
    a.add_argument("--seconds", type=float, default=15)
    a.add_argument("--gain", type=float, default=0, help="tuner gain dB (0 = least; the radio is close)")
    a.add_argument("--ppm", type=int, default=0)
    a.add_argument("--save")
    a.add_argument("--load")
    a.add_argument("--tones", action="store_true")
    o = a.parse_args()
    f0 = o.mhz * 1e6
    if o.load:
        raw = np.fromfile(o.load, np.uint8)
    else:
        n = int(o.seconds * FS) * 2
        cmd = ["rtl_sdr", "-f", str(int(f0 + OFF)), "-s", str(FS), "-g", str(o.gain), "-p", str(o.ppm), "-n", str(n // 2), "-"]
        print("capturing", o.seconds, "s at", o.mhz, "MHz (gain", o.gain, "dB): key up now", file=sys.stderr)
        raw = np.frombuffer(subprocess.run(cmd, capture_output=True).stdout, np.uint8)
        if o.save:
            raw.tofile(o.save)
    clip = np.mean((raw == 0) | (raw == 255))
    x = (raw[0::2].astype(np.float32) - 127.5) + 1j * (raw[1::2].astype(np.float32) - 127.5)
    x *= np.exp(2j * np.pi * OFF * np.arange(len(x)) / FS)  # the signal from -OFF to 0 Hz
    x = np.convolve(x, lowpass(20_000, FS), "same")[::DEC]
    fs = FS / DEC
    p = np.abs(x) ** 2
    blk = int(fs * 0.01)
    pb = p[: len(p) // blk * blk].reshape(-1, blk).mean(1)
    floor = np.percentile(pb, 10)  # the quietest tenth: a capture can be mostly keyed
    on = pb > floor * 100  # 20 dB over the floor
    if not on.any():
        print(f"no transmission found (floor {10*np.log10(floor+1e-12):.1f} dB, clipping {clip:.2%})")
        return
    i0, i1 = np.argmax(on), len(on) - np.argmax(on[::-1])
    seg = x[(i0 + 2) * blk : (i1 - 2) * blk]  # 20 ms in from each end: no key-up transients
    fm = np.angle(seg[1:] * np.conj(seg[:-1])) * fs / (2 * np.pi)
    fm = np.convolve(fm, lowpass(6_000, fs), "same")
    off = np.mean(fm)
    dev = fm - off
    print(f"keyed {(i1 - i0) * 0.01:.2f} s, {10*np.log10(pb[i0:i1].mean()/floor):.0f} dB over the floor, IQ clipping {clip:.2%}")
    print(f"carrier offset {off:+.0f} Hz ({off / o.mhz:+.1f} ppm, the dongle's error included unless --ppm)")
    print(f"deviation: peak {np.max(np.abs(dev)):.0f} Hz, 99.9 % {np.percentile(np.abs(dev), 99.9):.0f} Hz, rms {np.sqrt(np.mean(dev**2)):.0f} Hz")
    spec = np.abs(np.fft.rfft(dev * np.hanning(len(dev)))) * 2 / (np.hanning(len(dev)).sum())
    fr = np.fft.rfftfreq(len(dev), 1 / fs)

    def tone(hz, bw=40):
        m = (fr > hz - bw) & (fr < hz + bw)
        k = np.argmax(spec * m)
        return fr[k], spec[k]

    f12, a12 = tone(1200)
    f22, a22 = tone(2200)
    if a12 > 0 and a22 > 0:
        print(f"AFSK: {f12:.0f} Hz at {a12:.0f} Hz dev, {f22:.0f} Hz at {a22:.0f} Hz dev, twist (2200 over 1200) {20*np.log10(a22/a12):+.1f} dB")
    if o.tones:
        band = (fr > 50) & (fr < 6000)
        top = np.argsort(spec * band)[::-1]
        seen = []
        for k in top:
            if all(abs(fr[k] - s) > 30 for s in seen):
                seen.append(fr[k])
                print(f"  tone {fr[k]:7.1f} Hz  {spec[k]:6.0f} Hz dev")
            if len(seen) == 8:
                break


if __name__ == "__main__":
    main()
