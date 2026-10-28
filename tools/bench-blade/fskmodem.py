"""The BK4829 FSK modem's bench frames (TERM-43, decision 305; docs/research/bk4829-fsk-modem.md): a bladeRF
generator and a decoder of captures, byte for byte the firmware's `ferrite_core::fskbench` frame.

  fskmodem.py gen OUT.sc16 SPEED [--frames N] [--first K] [--gap S] [--crc NAME|none] [--dev HZ] [--invert] [--lsb]
  fskmodem.py dec CAP.u8 SPEED [--invert] [--dev HZ]
  fskmodem.py selftest

SPEED is the firmware's word: 1200, 2400 (or any plain FSK rate 300-9600), ffsk1800, ffsk2400.

gen: FM IQ for the bladeRF as fmgen.py makes it (sc16q11 interleaved, 1 MS/s, I/Q mirrored: Q first; the signal
200 kHz above the tuned centre), 0.5 s of carrier first and last, GAP s of carrier between frames. Each frame:
8 preamble bytes (0xAA: sync byte 0's top bit is 1), the sync word 85 CF AB 45, 32 data bytes (counter, PRBS9, our
CRC-16/CCITT-FALSE), the chip's CRC (2 bytes, the --crc candidate: the chip's is not documented; `none` for
`fsk rx SPEED nocrc`), then 2 bytes of 0xAA. FSK: NRZ straight on the carrier, a 1 at +DEV (default 2000 Hz);
FFSK: continuous-phase audio tones at 1200 baud, a 1 on 1200 Hz and a 0 on 1800 or 2400 Hz, FM at DEV (default
3000 Hz). --invert swaps the levels or tones; --lsb sends each byte least significant bit first.

dec: an rtl_sdr-style capture (u8 IQ, 1.024 MS/s, the signal 250 kHz below the centre, as bladecap.sh writes it):
FM demodulated, the sync word found by correlation in either polarity and bit order, each frame's bytes read and
checked: the counter, PRBS9 bit errors, our CRC, the byte order, and which CRC-16 (of the catalogue below) matches
the two bytes after the data, the chip's CRC when it is on.
"""
import sys
import numpy as np

FS_TX = 1_000_000
SHIFT_TX = 200_000
FS_CAP = 1_024_000
SHIFT_CAP = -250_000
LEN = 32
PRBS_LEN = LEN - 4
PREAMBLE = 8
SYNC = bytes([0x85, 0xCF, 0xAB, 0x45])
LEAD = 0.5


def prbs9_bits(n):
    s, out = 0x1FF, []
    for _ in range(n):
        b = ((s >> 8) ^ (s >> 4)) & 1
        s = ((s << 1) | b) & 0x1FF
        out.append(b)
    return out


def prbs():
    bits = prbs9_bits(8 * PRBS_LEN)
    return bytes(int("".join(map(str, bits[8 * i:8 * i + 8])), 2) for i in range(PRBS_LEN))


def crc16(data, poly=0x1021, init=0xFFFF, refin=False, refout=False, xorout=0):
    def rev(x, n):
        return int(f"{x:0{n}b}"[::-1], 2)
    c = init
    for b in data:
        if refin:
            b = rev(b, 8)
        c ^= b << 8
        for _ in range(8):
            c = ((c << 1) ^ poly) & 0xFFFF if c & 0x8000 else (c << 1) & 0xFFFF
    if refout:
        c = rev(c, 16)
    return c ^ xorout


# The usual CRC-16s (reveng's catalogue names): the chip's is one of these, or none.
CRCS = {
    "ccitt-false": dict(poly=0x1021, init=0xFFFF),
    "xmodem": dict(poly=0x1021, init=0x0000),
    "kermit": dict(poly=0x1021, init=0x0000, refin=True, refout=True),
    "x25": dict(poly=0x1021, init=0xFFFF, refin=True, refout=True, xorout=0xFFFF),
    "genibus": dict(poly=0x1021, init=0xFFFF, xorout=0xFFFF),
    "mcrf4xx": dict(poly=0x1021, init=0xFFFF, refin=True, refout=True),
    "arc": dict(poly=0x8005, init=0x0000, refin=True, refout=True),
    "modbus": dict(poly=0x8005, init=0xFFFF, refin=True, refout=True),
    "buypass": dict(poly=0x8005, init=0x0000),
    "dnp": dict(poly=0x3D65, init=0x0000, refin=True, refout=True, xorout=0xFFFF),
}


def payload(n):
    p = n.to_bytes(2, "big") + prbs()
    return p + crc16(p).to_bytes(2, "big")


def speed(word):
    w = word.lower()
    if w == "ffsk1800":
        return ("ffsk", 1200, 1800)
    if w == "ffsk2400":
        return ("ffsk", 1200, 2400)
    baud = int(w)
    if not 300 <= baud <= 9600:
        raise SystemExit("SPEED: 1200, 2400 (300-9600), ffsk1800 or ffsk2400")
    return ("fsk", baud, None)


def bits_of(data, lsb=False):
    out = []
    for b in data:
        bs = [(b >> i) & 1 for i in range(8)]
        out += bs if lsb else bs[::-1]
    return out


def frame_bytes(n, crc="ccitt-false"):
    pre = bytes([0xAA if SYNC[0] & 0x80 else 0x55] * PREAMBLE)
    data = payload(n)
    chip = b"" if crc == "none" else crc16(data, **CRCS[crc]).to_bytes(2, "big")
    return pre + SYNC + data + chip + bytes([0xAA, 0xAA])


def freq_dev(bits, sp, fs, dev, invert, gap_after=None):
    """The instantaneous frequency (Hz off the carrier) for bits at sp's rate, sampled at fs."""
    kind, baud, space = sp
    n = int(len(bits) * fs / baud)
    idx = np.minimum((np.arange(n) * baud / fs).astype(int), len(bits) - 1)
    b = np.array(bits, np.int8)[idx]
    if invert:
        b = 1 - b
    if kind == "fsk":
        return (b * 2 - 1) * dev
    tone = np.where(b > 0, 1200.0, float(space))
    return np.sin(2 * np.pi * np.cumsum(tone) / fs) * dev


def modulate(sp, counters, fs, dev, invert=False, lsb=False, crc="ccitt-false", gap=0.05):
    parts = [np.zeros(int(LEAD * fs))]
    for k in counters:
        parts.append(freq_dev(bits_of(frame_bytes(k, crc), lsb), sp, fs, dev, invert))
        parts.append(np.zeros(int(gap * fs)))
    parts.append(np.zeros(int(LEAD * fs)))
    return np.concatenate(parts)


def default_dev(sp):
    return 2000.0 if sp[0] == "fsk" else 3000.0


def opt(args, name, default, conv=str):
    if name in args:
        i = args.index(name)
        v = args[i + 1]
        del args[i:i + 2]
        return conv(v)
    return default


def flag(args, name):
    if name in args:
        args.remove(name)
        return True
    return False


def gen(args):
    out, word = args[0], args[1]
    rest = args[2:]
    sp = speed(word)
    n = opt(rest, "--frames", 10, int)
    first = opt(rest, "--first", 0, int)
    gap = opt(rest, "--gap", 0.05, float)
    crc = opt(rest, "--crc", "ccitt-false")
    dev = opt(rest, "--dev", default_dev(sp), float)
    invert, lsb = flag(rest, "--invert"), flag(rest, "--lsb")
    if rest or (crc != "none" and crc not in CRCS):
        raise SystemExit(__doc__)
    fd = modulate(sp, range(first, first + n), FS_TX, dev, invert, lsb, crc, gap) + SHIFT_TX
    ph = 2 * np.pi * np.cumsum(fd) / FS_TX
    iq = np.empty(2 * len(ph), np.int16)
    iq[0::2] = (np.sin(ph) * 1500).astype(np.int16)  # the bladeRF's I/Q mirrored (fmgen.py): Q first
    iq[1::2] = (np.cos(ph) * 1500).astype(np.int16)
    iq.tofile(out)
    print(f"{out}: {len(ph) / FS_TX:.2f} s, {word}, frames {first}-{first + n - 1}, dev {dev:.0f} Hz, chip CRC {crc}"
          f"{', inverted' if invert else ''}{', LSB first' if lsb else ''}")


def fir_lowpass(cut, fs, taps=129):
    k = np.arange(taps) - (taps - 1) / 2
    h = np.sinc(2 * cut / fs * k) * np.hamming(taps)
    return h / h.sum()


def demod(iq, fs, sp, dev):
    """Complex baseband at fs (the carrier at 0 Hz) -> (soft symbols at fsd, fsd): FM, then for FFSK the tones."""
    kind, baud, space = sp
    dec = 16
    bw = 2.5 * baud + dev if kind == "fsk" else dev + 3000
    x = np.convolve(iq, fir_lowpass(min(bw, fs / dec / 2 * 0.9), fs), mode="same")[::dec]
    fsd = fs / dec
    f = np.angle(x[1:] * np.conj(x[:-1])) * fsd / (2 * np.pi)
    f = np.concatenate([[0.0], f])
    if kind == "fsk":
        f = np.convolve(f, np.ones(max(int(fsd / baud / 2), 1)) / max(int(fsd / baud / 2), 1), mode="same")
        return f, fsd
    t = np.arange(len(f)) / fsd
    spb = int(fsd / baud)
    box = np.ones(spb) / spb
    em = np.abs(np.convolve(f * np.exp(-2j * np.pi * 1200 * t), box, mode="same"))
    es = np.abs(np.convolve(f * np.exp(-2j * np.pi * space * t), box, mode="same"))
    return em - es, fsd


def decode(iq, fs, sp, dev, invert=False, quiet=False):
    """Frames found in a complex baseband capture: a list of dicts."""
    kind, baud, _ = sp
    soft, fsd = demod(iq, fs, sp, dev)
    spb = fsd / baud
    found = []
    for lsb in (False, True):
        sync_bits = bits_of(SYNC, lsb)
        tmpl_bits = np.array(sync_bits) * 2 - 1
        if invert:
            tmpl_bits = -tmpl_bits
        tmpl = np.repeat(tmpl_bits, int(round(spb))).astype(float)
        lev = soft - np.convolve(soft, np.ones(int(spb * 64)) / int(spb * 64), mode="same") if kind == "fsk" else soft
        scale = np.sqrt(np.convolve(lev ** 2, np.ones(len(tmpl)), mode="same")) * np.sqrt(len(tmpl)) + 1e-9
        c = np.correlate(lev, tmpl, mode="same") / scale
        thr = 0.75
        i, n = 0, len(c)
        while i < n:
            if abs(c[i]) > thr:
                j = i + int(np.argmax(np.abs(c[i:i + int(spb * 8)])))
                pol = 1 if c[j] > 0 else -1
                start = j - len(tmpl) // 2 + len(tmpl)  # the first data bit's start
                nbits = 8 * (LEN + 2)
                centres = (start + (np.arange(nbits) + 0.5) * spb).astype(int)
                if centres[-1] >= len(lev):
                    break
                b = (lev[centres] * pol > 0).astype(int)
                data = bytes(int("".join(map(str, (b[8 * k:8 * k + 8][::-1] if lsb else b[8 * k:8 * k + 8]))), 2)
                             for k in range(LEN + 2))
                found.append(dict(at=j / fsd, lsb=lsb, inverted=(pol < 0) != invert, **check(data)))
                i = int(start + nbits * spb)
            else:
                i += 1
    found.sort(key=lambda f: f["at"])
    if not quiet:
        for f in found:
            print(f"{f['at']:8.3f} s  #{f['counter']:5d}  prbs errors {f['errors']:3d}  our crc {'ok' if f['crc'] else 'bad'}"
                  f"  {'lo-first' if f['lo_first'] else 'hi-first'}  {'LSB' if f['lsb'] else 'MSB'} first"
                  f"{'  inverted' if f['inverted'] else ''}  chip crc: {', '.join(f['chip_crc']) or 'none matched'}")
        clean = sum(1 for f in found if f["crc"] and f["errors"] == 0)
        print(f"{len(found)} frames, {clean} clean, {sum(f['errors'] for f in found)} PRBS bit errors")
    return found


def check(b):
    """A frame's 32 data bytes plus the 2 after them: both byte orders tried, as the firmware's `check`."""
    want = prbs()
    best = None
    for lo in (False, True):
        d = bytes(b[i ^ 1] for i in range(LEN)) if lo else bytes(b[:LEN])
        errors = sum(bin(x ^ y).count("1") for x, y in zip(d[2:2 + PRBS_LEN], want))
        r = dict(counter=int.from_bytes(d[:2], "big"), errors=errors,
                 crc=crc16(d[:LEN - 2]) == int.from_bytes(d[LEN - 2:LEN], "big"), lo_first=lo)
        if best is None or errors < best["errors"]:
            best = r
    tail = b[LEN:LEN + 2]
    best["chip_crc"] = [f"{name} ({end})" for name, p in CRCS.items() for end in ("big", "little")
                        if crc16(b[:LEN], **p).to_bytes(2, end) == tail]
    return best


def dec(args):
    path, word = args[0], args[1]
    rest = args[2:]
    sp = speed(word)
    dev = opt(rest, "--dev", default_dev(sp), float)
    invert = flag(rest, "--invert")
    if rest:
        raise SystemExit(__doc__)
    u = np.fromfile(path, np.uint8).astype(np.float32) - 127.5
    iq = u[0::2] + 1j * u[1::2]
    t = np.arange(len(iq)) / FS_CAP
    iq = iq * np.exp(-2j * np.pi * SHIFT_CAP * t)
    decode(iq, FS_CAP, sp, dev, invert)


def selftest():
    assert crc16(b"123456789") == 0x29B1, "CRC-16/CCITT-FALSE check value"
    checks = {"xmodem": 0x31C3, "kermit": 0x2189, "x25": 0x906E, "arc": 0xBB3D, "modbus": 0x4B37, "genibus": 0xD64E,
              "mcrf4xx": 0x6F91, "buypass": 0xFEE8, "dnp": 0xEA82}
    for name, want in checks.items():
        assert crc16(b"123456789", **CRCS[name]) == want, name
    p = payload(1)
    print("frame 1:", p.hex())
    assert len(p) == LEN and crc16(p[:LEN - 2]) == int.from_bytes(p[LEN - 2:], "big")
    rng = np.random.default_rng(1)
    for word in ("1200", "2400", "4800", "ffsk1800", "ffsk2400"):
        sp = speed(word)
        for crc, lsb, inv in (("ccitt-false", False, False), ("xmodem", True, True)):
            fd = modulate(sp, range(5, 9), FS_CAP, default_dev(sp), inv, lsb, crc)
            ph = 2 * np.pi * np.cumsum(fd) / FS_CAP
            iq = np.exp(1j * ph) * 40 + (rng.normal(size=len(ph)) + 1j * rng.normal(size=len(ph))) * 4
            found = decode(iq, FS_CAP, sp, default_dev(sp), quiet=True)
            got = [(f["counter"], f["errors"], f["crc"], f["lsb"], f["inverted"]) for f in found]
            assert got == [(k, 0, True, lsb, inv) for k in range(5, 9)], (word, crc, got)
            assert all(any(c.startswith(crc + " (big") for c in f["chip_crc"]) for f in found), (word, found[0]["chip_crc"])
        print(f"{word}: 4 frames back clean, both bit orders and polarities, chip CRC found")
    print("selftest ok")


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    cmd, args = sys.argv[1], sys.argv[2:]
    if cmd == "gen" and len(args) >= 2:
        gen(args)
    elif cmd == "dec" and len(args) >= 2:
        dec(args)
    elif cmd == "selftest":
        selftest()
    else:
        raise SystemExit(__doc__)


main()
