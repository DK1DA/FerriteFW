"""An RTL-SDR recording of APRS bursts, burst by burst (APRS-56.4: what a sender really put on the air): when the
carrier came up, when the AFSK began and how long its flags ran before the frame, so a frame the receiver lost can
be told from one the sender cut short. Writes each burst's audio (48 kHz WAV) for Dire Wolf's `atest`.

    rtl_sdr -f 145435000 -s 240000 -g 40 cap.u8        # on the machine with the dongle: the signal 60 kHz up
    aprsiq.py cap.u8 --offset 60000 [--out DIR] [--start UNIX_TIME]

--start: the recording's first sample's wall-clock time, to print each burst's time beside aprslead.py's `t=`.
Needs numpy (the project's .venv).
"""
import argparse, os, wave
import numpy as np

FS = 240_000
AF = 48_000


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("cap")
    ap.add_argument("--offset", type=float, default=60_000)
    ap.add_argument("--out", default=None)
    ap.add_argument("--start", type=float, default=None)
    a = ap.parse_args()
    raw = np.fromfile(a.cap, np.uint8).astype(np.float32) - 127.5
    iq = raw[0::2] + 1j * raw[1::2]
    n = np.arange(len(iq))
    iq *= np.exp(-2j * np.pi * a.offset * n / FS)
    # 25 kHz channel: a moving-average low-pass, then 5:1 to 48 kHz
    k = 9
    iq = np.convolve(iq, np.ones(k) / k, mode="same")[::5]
    pw = np.abs(iq) ** 2
    # the carrier: power over the floor, in 1 ms steps
    step = AF // 1000
    ms = pw[: len(pw) // step * step].reshape(-1, step).mean(axis=1)
    floor = np.median(ms)
    on = ms > floor * 10
    edges = np.flatnonzero(np.diff(on.astype(int)))
    starts = [e + 1 for e in edges if not on[e]]
    ends = [e + 1 for e in edges if on[e]]
    fm = np.angle(iq[1:] * np.conj(iq[:-1])) * AF / (2 * np.pi)  # instantaneous frequency, Hz
    out = a.out or os.path.dirname(os.path.abspath(a.cap))
    print(f"floor {10 * np.log10(floor):.1f} dB, {len(starts)} bursts")
    print("burst  t            carrier_ms  afsk_after_ms  afsk_ms  wav")
    for i, s in enumerate(starts):
        e = next((x for x in ends if x > s), len(on))
        if e - s < 30:
            continue
        seg = fm[s * step: e * step]
        # the AFSK: 1200 and 2200 Hz in the demodulated audio, against the carrier's silence
        seg = seg - np.median(seg)
        t = np.arange(len(seg)) / AF
        win = AF // 200  # 5 ms
        def tone(hz):
            z = seg * np.exp(-2j * np.pi * hz * t)
            return np.abs(np.convolve(z, np.ones(win) / win, mode="same"))
        lvl = tone(1200) + tone(2200)
        thr = lvl.max() * 0.3
        first = np.flatnonzero(lvl > thr)
        afsk_at = first[0] / AF * 1000 if len(first) else float("nan")
        afsk_len = (first[-1] - first[0]) / AF * 1000 if len(first) else 0
        name = os.path.join(out, f"burst{i:03d}.wav")
        audio = np.clip(seg / 4000 * 20000, -32767, 32767).astype(np.int16)
        with wave.open(name, "wb") as w:
            w.setnchannels(1); w.setsampwidth(2); w.setframerate(AF)
            w.writeframes(np.concatenate([np.zeros(AF // 10, np.int16), audio, np.zeros(AF // 10, np.int16)]).tobytes())
        when = f"{a.start + s / 1000:.2f}" if a.start else f"{s / 1000:9.3f}"
        print(f"{i:5}  {when:11}  {e - s:10}  {afsk_at:13.1f}  {afsk_len:7.0f}  {name}")


main()
