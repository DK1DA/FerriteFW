#!/usr/bin/env python3
"""TERM-33.17 (decision 308): bit errors of the radio's `txtest fsk`, `gmsk` and `g3ruh` transmissions, off the air.

  txdata.py CAP.u8 fsk BAUD        # `txtest fsk BAUD [rc] ...`
  txdata.py CAP.u8 gmsk BAUD       # `txtest gmsk BAUD ...`
  txdata.py CAP.u8 g3ruh           # `txtest g3ruh ...`: descrambled (x^17 + x^12 + 1) before counting
  [--skip S] [--seconds S]         # analyse from S s into the key-up, for S s (default: all of it)

CAP.u8 is a bladecap.sh capture (u8 IQ at 1.024 MS/s, the signal 250 kHz below the centre, I/Q already swapped
to rtl_sdr's order), or an rtl_sdr one; tools/sdrtx.py's demodulator finds the keyed part and FM-demodulates it
(64 kS/s, 16 kHz audio low-pass). Then a plain receiver: each bit's samples averaged (integrate and dump), and in
every 256 bits the best timing phase, polarity and PRBS9 offset found by search, as rxdata.py does for the receive
side. Printed: the bit error rate (plain, and with a running DC removal over 64 bits), the deviation at the bit
centres, and the eye: the decisions' mean distance from 0 over their spread (above ~3 clean), and the vertical
opening (the 0.1 % worst bit of each side, as a share of the distance between the two levels; 100 % ideal,
0 or less closed).

The sweep with an equaliser (`txtest sweep data bypass eq`) is sdrtx.py's: `sdrtx.py 0 --load CAP.u8 --sweep`.
"""
import argparse, os, sys
import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
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


def analyse(soft, spb, scrambled, label):
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
        print(f"  {label}: too short")
        return
    ones, zeros = np.array(ones), np.array(zeros)
    m1, m0 = ones.mean(), zeros.mean()
    opening = (np.percentile(ones, 0.1) - np.percentile(zeros, 99.9)) / (m1 - m0)
    print(f"  {label:9s}: BER {errs / bits:.5f} ({errs} of {bits} bits), eye {np.median(eyes):.1f} (worst chunk {min(eyes):.1f}), "
          f"opening {opening * 100:.0f} %, levels {m0:+.0f} / {m1:+.0f} Hz")


def main():
    a = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    a.add_argument("cap")
    a.add_argument("kind", choices=["fsk", "gmsk", "g3ruh"])
    a.add_argument("baud", type=float, nargs="?")
    a.add_argument("--skip", type=float, default=0.1, help="seconds into the keyed part to start (default 0.1)")
    a.add_argument("--seconds", type=float, help="seconds to analyse (default: to the end)")
    o = a.parse_args()
    baud = 9600.0 if o.kind == "g3ruh" else o.baud
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
    spb = fs / baud
    print(f"{o.kind} {baud:g} Bd: {len(dev) / fs:.1f} s, {spb:.2f} samples a bit; deviation 99.9 % {np.percentile(np.abs(dev), 99.9):.0f} Hz")
    soft = mavg(dev, spb)  # a bit's samples averaged, centred: a bit's centre reads its whole span
    analyse(soft, spb, o.kind == "g3ruh", "plain")
    L = int(spb * 64)
    analyse(soft - mavg(soft, L), spb, o.kind == "g3ruh", "DC off")


if __name__ == "__main__":
    main()
