#!/usr/bin/env python3
"""TERM-33.17 (decision 308): bit errors of the radio's `txtest fsk`, `gmsk` and `g3ruh` transmissions, off the air.

  txdata.py CAP.u8 fsk BAUD        # `txtest fsk BAUD [rc] ...`
  txdata.py CAP.u8 gmsk BAUD       # `txtest gmsk BAUD ...`
  txdata.py CAP.u8 g3ruh           # `txtest g3ruh ...`: descrambled (x^17 + x^12 + 1) before counting
  txdata.py CAP.u8 fsk4            # `txtest 4fsk ...`: M17's 4800 symbols; the deviation goes through rxdata.py's 4FSK
                                   # analysis (RRC 0.5 matched filter, three thresholds), as CAP.u8.fsk4.wav
  [--skip S] [--seconds S]         # analyse from S s into the key-up, for S s (default: all of it)

CAP.u8 is a bladecap.sh capture (u8 IQ at 1.024 MS/s, the signal 250 kHz below the centre, I/Q already swapped
to rtl_sdr's order), or an rtl_sdr one; tools/sdrtx.py's demodulator finds the keyed part and FM-demodulates it
(64 kS/s, 16 kHz audio low-pass). Then a plain receiver: each bit's samples averaged (integrate and dump), and in
every 256 bits the best timing phase, polarity and PRBS9 offset found by search, as rxdata.py does for the receive
side. Printed: the bit error rate (plain, and with a running DC removal over 64 bits), the deviation at the bit
centres, and the eye: the decisions' mean distance from 0 over their spread (above ~3 clean), and the vertical
opening (the 0.1 % worst bit of each side, as a share of the distance between the two levels; 100 % ideal,
0 or less closed).

`txdata.py --selftest` measures the analyser's own floor: the same signals made as IQ through the identified bypass
channel, with and without the equalisers, at 55 dB over the floor.

The sweep with an equaliser (`txtest sweep data bypass eq`) is sdrtx.py's: `sdrtx.py 0 --load CAP.u8 --sweep`.
"""
import argparse, os, sys
import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sdrtx  # noqa: E402

C = 256  # bits per chunk: timing and pattern offset found in each


def prbs9(n):
    s, out = 0x1FF, np.empty(n, np.int8)
    for i in range(n):
        b = ((s >> 8) ^ (s >> 4)) & 1
        s = ((s << 1) | b) & 0x1FF
        out[i] = b
    return out


def descramble(bits):
    """G3RUH's descrambler: each bit XOR the bits received 12 and 17 before (the first 17 are wrong)."""
    b = bits.astype(np.int8)
    out = b.copy()
    out[12:] ^= b[:-12]
    out[17:] ^= b[:-17]
    return out


def mavg(x, n):
    n = max(int(round(n)), 1)
    c = np.cumsum(np.concatenate([[0.0], x]))
    y = (c[n:] - c[:-n]) / n
    return np.concatenate([np.full(n // 2, y[0]), y, np.full(n - 1 - n // 2, y[-1])])


def analyse(soft, spb, scrambled, label, quiet=False):
    ref = prbs9(511)
    n = C - 17 if scrambled else C
    # every PRBS9 offset's next n bits, +-1: one matrix product scores them all
    R = np.array([np.resize(np.roll(ref, -m), n) for m in range(511)], np.float64) * 2 - 1
    errs = bits = 0
    eyes, ones, zeros = [], [], []
    for c0 in range(0, int(len(soft) / spb) - C - 1, C):
        best = None
        for ph in np.linspace(0, spb, 16, endpoint=False):
            idx = (ph + (c0 + np.arange(C)) * spb).astype(int)
            v = soft[idx]
            for pol in (1, -1):
                h = (v * pol > 0).astype(np.int8)
                d = descramble(h)[17:] if scrambled else h
                # the PRBS9 offset with the most agreements
                c = R @ (d.astype(np.float64) * 2 - 1)
                m = int(np.argmax(c))
                e = int(round(n - c[m])) // 2
                # the eye: on the line bits as decided (for g3ruh the scrambled ones); the timing phase with the
                # fewest errors, and of those the widest eye
                vv = (v * pol)[17:] if scrambled else v * pol
                hh = h[17:] if scrambled else h
                sv = np.where(hh == 1, vv, -vv)
                eye = sv.mean() / max(sv.std(), 1e-9)
                if best is None or (e, -eye) < (best[0], -best[1]):
                    best = (e, eye, vv, hh)
        e, _, vv, h = best
        errs += e
        bits += n
        s = np.where(h == 1, vv, -vv)
        eyes.append(s.mean() / max(s.std(), 1e-9))
        ones.extend(vv[h == 1])
        zeros.extend(vv[h == 0])
    if not bits:
        if not quiet:
            print(f"  {label}: too short")
        return None
    ones, zeros = np.array(ones), np.array(zeros)
    m1, m0 = ones.mean(), zeros.mean()
    opening = (np.percentile(ones, 0.1) - np.percentile(zeros, 99.9)) / (m1 - m0)
    if not quiet:
        print(f"  {label:9s}: BER {errs / bits:.5f} ({errs} of {bits} bits), eye {np.median(eyes):.1f} (worst chunk {min(eyes):.1f}), "
              f"opening {opening * 100:.0f} %, levels {m0:+.0f} / {m1:+.0f} Hz")
    return dict(ber=errs / bits, eye=float(np.median(eyes)), opening=float(opening), levels=(m0, m1))


def iq(dev_hz, cnr_db, rng, lead=0.3):
    """u8 IQ at 1.024 MS/s as bladecap.sh writes it: the FM signal 250 kHz below the centre, amplitude 100, white
    noise for `cnr_db` of carrier over the floor in sdrtx.py's 20 kHz baseband (its "dB over the floor")."""
    n = len(dev_hz)
    m = int(round(n * sdrtx.FS / 48_000))
    up = np.fft.irfft(np.fft.rfft(dev_hz), m) * m / n
    ln = int(lead * sdrtx.FS)
    f = np.concatenate([np.zeros(ln), up, np.zeros(ln)]) - sdrtx.OFF
    amp = np.concatenate([np.zeros(ln), np.full(m, 100.0), np.zeros(ln)])
    sigma = 100 / np.sqrt(2) * 10 ** (-cnr_db / 20) * np.sqrt(sdrtx.FS / 40_000)
    x = amp * np.exp(2j * np.pi * np.cumsum(f) / sdrtx.FS) + rng.normal(0, sigma, len(f)) + 1j * rng.normal(0, sigma, len(f))
    raw = np.empty(2 * len(x), np.uint8)
    raw[0::2] = np.clip(np.round(x.real + 127.5), 0, 255)
    raw[1::2] = np.clip(np.round(x.imag + 127.5), 0, 255)
    return raw


def selftest(cnr_db=55.0, seconds=4.0):
    """The analyser's own floor (decision 311): txtest's signals (txident.py's gen) at the level each fits, with no
    channel, through the identified bypass channel alone (txeq_design.py's model: the 168 Hz pole, the 35 Hz
    coupling, the chip's roll-off), and through it with `eq` and `eq8` (the taps in txeq.rs), FM-modulated as IQ at
    `cnr_db` over the floor and quantised to u8, then this script's demodulator and slicer."""
    import txident, txeq_design as d
    rng = np.random.default_rng(7)
    sigs = [("fsk", 1200, False), ("fsk", 4800, True), ("gmsk", 4800, False), ("fsk", 9600, True), ("g3ruh", 9600, True)]
    print(f"txdata selftest: {cnr_db:g} dB over the floor, {seconds:g} s each; BER, eye, opening, as `plain` reads them")
    print(f"  {'signal':12s} {'no channel':>20s} {'bypass, no eq':>20s} {'bypass + eq':>20s} {'bypass + eq8':>20s}")
    for kind, baud, rc in sigs:
        cells = []
        for case in ("clean", "chan", "eq", "eq8"):
            g = None
            if case in ("eq", "eq8"):
                g = txident.taps(case)
            x = txident.gen(kind, baud, rc, int(48_000 * seconds), 100)
            if g is not None:
                x = np.clip(np.convolve(x, g)[: len(x)], -32767, 32767)
            if case != "clean":
                fx = np.fft.rfftfreq(len(x), 1 / 48_000)
                x = np.fft.irfft(np.fft.rfft(x) * d.channel(fx), len(x))
            dev = x / np.sqrt(np.mean(x ** 2)) * 2000  # about 2 kHz rms deviation, as on the bench
            raw = iq(dev, cnr_db, rng)
            dd = sdrtx.demod(raw, 0, 16_000, quiet=True)
            y, fs, _ = dd
            y = y[int(0.1 * fs):]
            spb = fs / baud
            r = analyse(mavg(y, spb), spb, kind == "g3ruh", "", quiet=True)
            cells.append(f"{r['ber'] * 100:6.2f} % {r['eye']:4.1f} {r['opening'] * 100:3.0f} %")
        name = "g3ruh" if kind == "g3ruh" else f"{kind} {baud}{' rc' if rc else ''}"
        print(f"  {name:12s} " + " ".join(f"{c:>20s}" for c in cells))


def main():
    if "--selftest" in sys.argv:
        return selftest()
    a = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    a.add_argument("cap")
    a.add_argument("kind", choices=["fsk", "gmsk", "g3ruh", "fsk4"])
    a.add_argument("baud", type=float, nargs="?")
    a.add_argument("--skip", type=float, default=0.1, help="seconds into the keyed part to start (default 0.1)")
    a.add_argument("--seconds", type=float, help="seconds to analyse (default: to the end)")
    o = a.parse_args()
    baud = {"g3ruh": 9600.0, "fsk4": 4800.0}.get(o.kind, o.baud)
    if not baud:
        a.error("fsk and gmsk need the BAUD")
    raw = np.fromfile(o.cap, np.uint8)
    d = sdrtx.demod(raw, 0, 16_000)
    if d is None:
        return
    dev, fs, _ = d
    dev = dev[int(o.skip * fs):]
    if o.seconds:
        dev = dev[: int(o.seconds * fs)]
    if o.kind == "fsk4":
        import subprocess, wave
        print(f"fsk4 4800 sym: {len(dev) / fs:.1f} s; deviation 99.9 % {np.percentile(np.abs(dev), 99.9):.0f} Hz")
        out = o.cap + ".fsk4.wav"
        with wave.open(out, "wb") as w:
            w.setnchannels(1); w.setsampwidth(2); w.setframerate(int(fs))
            w.writeframes(np.clip(dev * 4, -32767, 32767).astype(np.int16).tobytes())
        sys.stdout.flush()
        subprocess.run([sys.executable, "-I", os.path.join(os.path.dirname(os.path.abspath(__file__)), "rxdata.py"), out, "fsk4", "4800"])
        return
    spb = fs / baud
    print(f"{o.kind} {baud:g} Bd: {len(dev) / fs:.1f} s, {spb:.2f} samples a bit; deviation 99.9 % {np.percentile(np.abs(dev), 99.9):.0f} Hz")
    soft = mavg(dev, spb)  # a bit's samples averaged, centred: a bit's centre reads its whole span
    analyse(soft, spb, o.kind == "g3ruh", "plain")
    L = int(spb * 64)
    analyse(soft - mavg(soft, L), spb, o.kind == "g3ruh", "DC off")


if __name__ == "__main__":
    main()
