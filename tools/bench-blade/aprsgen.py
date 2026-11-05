"""An APRS packet burst as FM IQ for the bladeRF (sc16q11, 1 MS/s, the signal SHIFT Hz above the tuned centre, as
fmgen.py): no carrier, then the carrier keyed with TXDELAY ms of flags, one AX.25 UI frame, two closing flags, and
no carrier again, as a real station sends. For the squelch-to-capture timing (PWR-26.9.1, APRS-56): the shortest
TXDELAY black still decodes is the whole delay from the carrier to the decoder listening.

  aprsgen.py OUT.sc16 TXDELAY_MS [--ssid N] [--warm GAP_MS]
  --ssid N      the source VA7IF-N, so the heard list shows which burst got through
  --warm GAP    a first burst (300 ms of flags) GAP ms before the test burst: the capture is still running (its tail
                is 300 ms, SQ_TAIL_MS), so the test burst shows the decoder's own need, without the start-up
Deviation: DEV (env, default 3000 Hz peak).
"""
import os, sys, wave
import numpy as np

FS = 1_000_000
AF = 48_000
SHIFT = 200_000
DEV = float(os.environ.get("DEV", "3000"))
BAUD, MARK, SPACE = 1200, 1200, 2200
QUIET = 0.3  # no carrier before and after, s


def addr(call, ssid, last):
    c = call.ljust(6)[:6].upper()
    return bytes(ord(x) << 1 for x in c) + bytes([0x60 | (ssid << 1) | (1 if last else 0)])


def fcs(data):
    crc = 0xFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0x8408 if crc & 1 else crc >> 1
    crc ^= 0xFFFF
    return bytes([crc & 0xFF, crc >> 8])


def frame_bits(ssid, text):
    body = addr("APZFER", 0, False) + addr("VA7IF", ssid, True) + b"\x03\xf0" + text.encode()
    body += fcs(body)
    out, ones = [], 0
    for b in body:
        for i in range(8):
            bit = (b >> i) & 1
            out.append(bit)
            ones = ones + 1 if bit else 0
            if ones == 5:
                out.append(0)
                ones = 0
    return out


FLAG = [0, 1, 1, 1, 1, 1, 1, 0]  # 0x7E, LSB first


def burst(txdelay_ms, ssid, text):
    """NRZ bits (before NRZI) of one transmission."""
    n = max(1, round(txdelay_ms / 1000 * BAUD / 8))
    return FLAG * n + frame_bits(ssid, text) + FLAG * 2


def afsk(bits):
    """NRZI (0 = change tone), continuous-phase AFSK audio at AF, unit peak."""
    tone, f = MARK, []
    spb = AF / BAUD
    for i, b in enumerate(bits):
        if b == 0:
            tone = SPACE if tone == MARK else MARK
        f += [tone] * (int((i + 1) * spb) - int(i * spb))
    return np.sin(2 * np.pi * np.cumsum(f) / AF)


def main():
    a = sys.argv[1:]
    if len(a) < 2:
        sys.exit(__doc__)
    out, txd = a[0], float(a[1])
    ssid = int(a[a.index("--ssid") + 1]) if "--ssid" in a else 1
    warm = float(a[a.index("--warm") + 1]) if "--warm" in a else None
    q = np.zeros(int(QUIET * AF))
    parts, keyed = [q], [np.zeros_like(q)]
    if warm is not None:
        w = afsk(burst(300, 0, ">bench warm"))
        g = np.zeros(int(warm / 1000 * AF))
        parts += [w, g]; keyed += [np.ones_like(w), np.zeros_like(g)]
    t = afsk(burst(txd, ssid, f">bench T{txd:.0f}"))
    parts += [t, q]; keyed += [np.ones_like(t), np.zeros_like(q)]
    fd, on = np.concatenate(parts) * DEV, np.concatenate(keyed)
    with wave.open(out + ".audio.wav", "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(AF)
        w.writeframes((fd / DEV * 30000).astype(np.int16).tobytes())
    tt = np.arange(int(len(fd) * FS / AF)) / FS
    src = np.arange(len(fd)) / AF
    fi = np.interp(tt, src, fd) + SHIFT
    amp = np.interp(tt, src, on) > 0.5  # keyed: the carrier, else nothing
    ph = 2 * np.pi * np.cumsum(fi) / FS
    iq = np.empty(2 * len(ph), np.int16)
    # the bladeRF's I/Q is the mirror of the usual order (bench, Oct 27): Q first
    iq[0::2] = (np.sin(ph) * 1500 * amp).astype(np.int16)
    iq[1::2] = (np.cos(ph) * 1500 * amp).astype(np.int16)
    iq.tofile(out)
    print(f"{out}: {len(ph) / FS:.2f} s, TXDELAY {txd:.0f} ms, VA7IF-{ssid}" + (f", warm gap {warm:.0f} ms" if warm is not None else ""))


main()
