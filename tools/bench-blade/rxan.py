"""The radio's receive audio (a WAV from `ferrite_link audio`) of fmgen.py's sweep: each tone's level against 1 kHz,
the -3 dB points, a plot. rxan.py sweep FILE.wav OUT.png"""
import sys, wave
import numpy as np

THIRD = [50, 63, 80, 100, 125, 160, 200, 250, 315, 400, 500, 630, 800, 1000, 1250, 1600, 2000, 2500, 3150, 4000, 5000, 6300, 8000]


def load(p):
    with wave.open(p) as w:
        fs = w.getframerate()
        x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(float)
    return x - x.mean(), fs


def level(x, fs, hz):
    """The tone's amplitude by a windowed single-bin DFT."""
    n = len(x)
    t = np.arange(n) / fs
    w = np.hanning(n)
    return 2 * abs(np.sum(x * w * np.exp(-2j * np.pi * hz * t))) / w.sum()


def sweep(path, png):
    x, fs = load(path)
    # each tone's strongest 0.2 s window anywhere (the tones differ in frequency, so no timing is needed)
    win, hop = int(0.2 * fs), int(0.05 * fs)
    def best(hz):
        return max(level(x[i:i + win], fs, hz) for i in range(0, len(x) - win, hop)) if hz < fs / 2 else 0.0
    rows = [(hz, best(hz)) for hz in THIRD]
    ref = best(1000)
    print(f"1 kHz level {ref:.0f} (of 32767), audio {fs} Hz")
    db = [(hz, 20 * np.log10(max(v, 1e-3) / ref)) for hz, v in rows]
    for hz, d in db:
        print(f"  {hz:5d} Hz  {d:+6.1f} dB")
    lo = next((hz for hz, d in db if d > -3), None)
    hi = next((hz for hz, d in reversed(db) if d > -3), None)
    print(f"  within 3 dB of 1 kHz: {lo} Hz to {hi} Hz (third-octave steps)")
    try:
        import matplotlib; matplotlib.use("Agg"); import matplotlib.pyplot as plt
        f, ax = plt.subplots(figsize=(8, 4.6))
        ax.semilogx([h for h, _ in db], [d for _, d in db], "o-")
        ax.axhline(-3, color="r", ls="--", lw=0.8)
        ax.set_xlabel("audio frequency (Hz)"); ax.set_ylabel("level, dB against 1 kHz")
        ax.set_title(f"RX response (radio's audio to the PC, {fs} Hz stream)")
        ax.grid(True, which="both", alpha=0.3); ax.set_ylim(-60, 10)
        f.tight_layout(); f.savefig(png, dpi=110)
        print("plot:", png)
    except ImportError:
        pass


if __name__ == "__main__":
    sweep(sys.argv[2], sys.argv[3])
