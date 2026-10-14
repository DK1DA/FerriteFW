"""The tones at the end of a transmission on an RTL-SDR (TONE-10 end tones, CW ID, tail elimination, the 400 ms
carrier hold of TX-47): the last `--tail` seconds of the keyed part, the audio tone tracked every 5 ms, each steady
tone listed with its frequency, length and deviation, then how long the carrier stays up after the last tone.

    sdrtones.py 145.495 --seconds 15 [--save cap.u8] [--load cap.u8] [--tail 2]

Needs librtlsdr and numpy (the project's .venv); shares sdrtx.py's capture and demodulation.
"""
import argparse, os, sys
import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sdrtx  # noqa: E402


def steady_tones(fm, fs, step_s=0.005, win_s=0.02, min_dev=150):
    """The steady tones in an FM deviation (Hz): per `step_s`, the strongest tone 300-3500 Hz over `win_s` (FFT with
    zero padding) and its deviation; runs where the frequency holds within 30 Hz and the deviation is well above the
    quiet. Each (start s, end s, Hz, median deviation Hz)."""
    blk = int(fs * step_s)
    w = int(fs * win_s)
    win = np.hanning(w)
    fr = np.fft.rfftfreq(8192, 1 / fs)
    band = (fr > 300) & (fr < 3500)
    rows = []
    for k in range(0, len(fm) - w, blk):
        sp = np.abs(np.fft.rfft(fm[k : k + w] * win, 8192)) * 2 / win.sum()
        j = np.argmax(sp * band)
        rows.append((k / fs, fr[j], sp[j]))
    if not rows:
        return []
    rows = np.array(rows)
    quiet = np.percentile(rows[:, 2], 20)
    tones, start = [], None
    for r in range(len(rows)):
        t, f, d = rows[r]
        live = d > max(min_dev, quiet * 4)
        same = start is not None and abs(f - rows[start][1]) < 30
        if start is not None and (not live or not same):
            if r - start >= 4:
                tones.append((rows[start][0], rows[r - 1][0] + win_s, np.median(rows[start:r, 1]), np.median(rows[start:r, 2])))
            start = None
        if live and start is None:
            start = r
    if start is not None and len(rows) - start >= 4:
        tones.append((rows[start][0], rows[-1][0] + win_s, np.median(rows[start:, 1]), np.median(rows[start:, 2])))
    return tones


def main():
    a = argparse.ArgumentParser()
    a.add_argument("mhz", type=float)
    a.add_argument("--seconds", type=float, default=15)
    a.add_argument("--gain", type=float, default=0)
    a.add_argument("--ppm", type=int, default=0)
    a.add_argument("--save")
    a.add_argument("--load")
    a.add_argument("--tail", type=float, default=2.0, help="seconds before the unkey to look at")
    o = a.parse_args()
    fs_in, off = sdrtx.FS, sdrtx.OFF
    if o.load:
        raw = np.fromfile(o.load, np.uint8)
    else:
        import subprocess
        n = int(o.seconds * fs_in)
        print("capturing", o.seconds, "s: key up, then release", file=sys.stderr)
        raw = np.frombuffer(subprocess.run(["rtl_sdr", "-f", str(int(o.mhz * 1e6 + off)), "-s", str(fs_in), "-g", str(o.gain),
                                            "-p", str(o.ppm), "-n", str(n), "-"], capture_output=True).stdout, np.uint8)
        if o.save:
            raw.tofile(o.save)
    x = (raw[0::2].astype(np.float32) - 127.5) + 1j * (raw[1::2].astype(np.float32) - 127.5)
    x *= np.exp(2j * np.pi * off * np.arange(len(x)) / fs_in)
    x = np.convolve(x, sdrtx.lowpass(20_000, fs_in), "same")[:: sdrtx.DEC]
    fs = fs_in / sdrtx.DEC
    p = np.abs(x) ** 2
    blk = int(fs * 0.005)
    pb = p[: len(p) // blk * blk].reshape(-1, blk).mean(1)
    floor = np.percentile(pb, 10)
    if floor > 1e3:
        floor = 1.0
    on = pb > floor * 100
    if not on.any():
        print("no transmission found")
        return
    i1 = len(on) - np.argmax(on[::-1])  # carrier drop, blocks
    i0 = np.argmax(on)
    print(f"keyed {(i1 - i0) * 0.005:.2f} s")
    a0 = max(i0 + 4, i1 - int(o.tail / 0.005))
    seg = x[a0 * blk : i1 * blk]
    fm = np.angle(seg[1:] * np.conj(seg[:-1])) * fs / (2 * np.pi)
    fm = np.convolve(fm, sdrtx.lowpass(4_000, fs), "same")
    fm -= np.median(fm)
    tones = steady_tones(fm, fs)
    end = (i1 - a0) * 0.005
    if not tones:
        print(f"no steady tone in the last {o.tail:.1f} s")
    for t0, t1, f, d in tones:
        print(f"  tone {f:7.1f} Hz  {(t1 - t0) * 1000:5.0f} ms  {d:5.0f} Hz dev   ends {(end - t1) * 1000:5.0f} ms before the carrier drops")


if __name__ == "__main__":
    main()
