"""The link protocol itself and the terminal over it (chapters 29, 37: LINK-11, LINK-23, TERM-13, TERM-33.13)."""
import re
import struct
import time

import ferrite_link as fl

from .core import test


@test("LINK-23", pages=("Home",))
def one_request_in_flight_no_xon_xoff(t):
    """Requests back to back, each sent after the last answer, all answered in order; payload bytes 0x11 and 0x13
    (XON and XOFF) pass inside frames: knob +17 and -19 tune exactly 17 and 19 steps."""
    stopped = t.stop_scans()
    try:
        t.cmd("slot A vfo 146.52")
        t.cmd("slot A active")
        u = t.home()
        f0 = float(u.active["freq"])
        for i in range(40):
            s, rest = t.r.link.ask(fl.UI, bytes([0]))
            t.eq(s, 0, f"request {i}: answered")
            if rest[0]:  # more pages: drain them, one request at a time
                while True:
                    s, rest = t.r.link.ask(fl.MORE)
                    if s != 0 or rest[0] == 0:
                        break
        step = None
        t.r.knob(17)  # 0x11 in the KEY frame
        u = t.ui(0.3)
        f1 = float(u.active["freq"])
        step = (f1 - f0) / 17
        t.check(step > 0, "knob +17 tuned up", (f0, f1))
        t.r.knob(-19)  # 0x13 (-19 is 0xED; +19 below)
        t.r.knob(19)
        t.r.knob(-17)
        u = t.ui(0.3)
        t.check(abs(float(u.active["freq"]) - f0) < 1e-6, "+17 -19 +19 -17: back where it started",
                (f0, u.active["freq"]))
        t.note(f"step {step * 1000:.2f} kHz")
    finally:
        t.put_back(stopped)


@test("LINK-11", pages=())
def baud_switch_and_fallback(t):
    """BAUD after HELLO: 230400 is taken, both switch and a HELLO at the new rate keeps it; BAUD 115200 goes back;
    a BAUD without a HELLO at the new rate within 1 s falls back to 115200. On the radio's 8 MHz clock 460800 and
    921600 are refused (out of the UART's 2 %)."""
    link, port = t.r.link, t.r.port
    hello = bytes([fl.PROTOCOL]) + bytes(8) + b"uitest.py"

    def baud(rate):
        s, rest = link.ask(fl.BAUD, struct.pack("<I", rate))
        return s

    try:
        t.eq(baud(230400), 0, "BAUD 230400 taken")
        time.sleep(0.05)
        port.baud(230400)
        s, _ = link.ask(fl.HELLO, hello)
        t.eq(s, 0, "HELLO at 230400 within 1 s")
        time.sleep(1.5)
        t.eq(link.ask(fl.UI, bytes([0]))[0], 0, "still at 230400 after 1.5 s (the HELLO kept it)")
        t.eq(baud(115200), 0, "BAUD 115200 back")
        time.sleep(0.05)
        port.baud(115200)
        t.eq(link.ask(fl.HELLO, hello)[0], 0, "HELLO at 115200")
        t.eq(baud(230400), 0, "BAUD 230400 again")
        time.sleep(1.6)  # no HELLO at the new rate: the radio goes back to 115200
        t.eq(link.ask(fl.HELLO, hello)[0], 0, "no HELLO at 230400: back at 115200")
        if not t.sim:
            for rate in (460800, 921600):
                t.check(baud(rate) != 0, f"{rate}: refused on the 8 MHz clock", rate)
    finally:
        port.baud(115200)
        time.sleep(1.5)
        link.ask(fl.HELLO, hello)


@test("TERM-13", pages=())
def dump_lines_load_back(t):
    """`dump` gives every setting, zone, preset and channel as terminal lines, versioned in its first line and
    ending with `end`; each line run back (as `load` runs them) is taken whole, and a second `dump` is the same
    text. (The settings, zones, presets, the home channel and the first three channels: a whole channel list
    written again would only wear the flash.)"""
    d1 = t.cmd("dump")
    lines = [x for x in d1.splitlines() if x.strip()]
    t.check(lines[0].startswith("#") and re.search(r"\d", lines[0]), "a versioned first line", lines[0])
    t.eq(lines[-1].strip(), "end", "it ends with end")
    chans = 0
    for line in lines[1:-1]:
        if line.startswith("#"):
            continue
        word = line.split()[0]
        if word == "ch":
            chans += 1
            if chans > 3:
                continue
        ok, why = t.cmd_err(line.split("#")[0].strip() if word == "set" else line)
        t.check(ok, f"{line!r} taken", why)
    d2 = t.cmd("dump")
    a = [x for x in d1.splitlines() if not x.startswith("#")]
    b = [x for x in d2.splitlines() if not x.startswith("#")]
    t.eq(b, a, "the same dump after")


@test("TERM-33.13", pages=())
def txtest_not_in_a_user_build(t):
    """On the receive and tx builds a user installs, `txtest 4fsk` is refused (not in this build: a bench build
    has it) and nothing is armed."""
    t.need(not t.r.bench, "a bench build: it has txtest")
    ok, out = t.cmd_err("txtest 4fsk")
    t.check(not ok, "refused", out)
    t.check("not in this build" in out.lower(), "names why", out)
    ok, out = t.cmd_err("txtest")
    t.check(not ok, "txtest alone too", out)
