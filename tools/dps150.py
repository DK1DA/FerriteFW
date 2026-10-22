#!/usr/bin/env python3
"""The FNIRSI DPS-150 bench supply over its USB serial port: read it, set it, and log the radio's current draw.

    tools/dps150.py PORT info                  # model, versions, set point, output, input voltage
    tools/dps150.py PORT set VOLTS AMPS        # the set point (the output stays as it is)
    tools/dps150.py PORT on | off              # the output
    tools/dps150.py PORT log SECONDS [--csv F] # sample V/I/P; prints mean, median, min, max, mAh, mWh

The protocol (115200 8N1; reverse-engineered by the community, see cho45/fnirsi-dps-150): frames to the supply
start F1, from it F0, then a command, a type, a length, the data and a checksum, the sum of type, length and data
bytes mod 256. Commands: A1 get, B1 set, B0 baud, C1 session (1 opens, 0 closes). Floats are 32-bit little-endian.
Types used here: C0 input V; C1/C2 V/I set; C3 output V, I, P; D9 Ah; DA Wh; DB output on; DE/DF/E0 model,
hardware and firmware; FF everything.

The radio runs from 2S Li-ion: `set` refuses more than 8.4 V unless --max-volts says otherwise. As a module:
`with Dps150(port) as d: d.reading()`.
"""
import argparse
import statistics
import struct
import sys
import time

import serial

GET, SET, BAUD, SESSION = 0xA1, 0xB1, 0xB0, 0xC1
T_IN_V, T_VSET, T_ISET, T_OUT, T_AH, T_WH, T_OUTPUT_ON = 0xC0, 0xC1, 0xC2, 0xC3, 0xD9, 0xDA, 0xDB
T_MODEL, T_HW, T_FW, T_ALL = 0xDE, 0xDF, 0xE0, 0xFF
BAUDS = [9600, 19200, 38400, 57600, 115200]


def frame(cmd, typ, data):
    data = bytes(data)
    return bytes([0xF1, cmd, typ, len(data), *data, (typ + len(data) + sum(data)) & 0xFF])


class Dps150:
    def __init__(self, port, timeout=0.2):
        self.s = serial.Serial(port, 115200, timeout=timeout, rtscts=False)
        self.buf = b""
        self.state = {}

    def __enter__(self):
        self.send(SESSION, 0, [1])
        self.send(BAUD, 0, [BAUDS.index(115200) + 1])
        return self

    def __exit__(self, *_):
        try:
            self.send(SESSION, 0, [0])
        finally:
            self.s.close()

    def send(self, cmd, typ, data):
        self.s.write(frame(cmd, typ, data))
        time.sleep(0.05)  # the supply drops frames sent back to back

    def get(self, typ):
        self.send(GET, typ, [0])

    def set_float(self, typ, value):
        self.send(SET, typ, struct.pack("<f", value))

    def poll(self, seconds=0.0):
        """Read what has arrived (for at least `seconds`); returns the frames' types, updates `state`."""
        seen = []
        end = time.monotonic() + seconds
        while True:
            self.buf += self.s.read(max(1, self.s.in_waiting))
            while True:
                i = self.buf.find(b"\xf0\xa1")
                if i < 0:
                    self.buf = self.buf[-1:]
                    break
                if len(self.buf) < i + 5:
                    self.buf = self.buf[i:]
                    break
                typ, n = self.buf[i + 2], self.buf[i + 3]
                if len(self.buf) < i + 5 + n:
                    self.buf = self.buf[i:]
                    break
                data, cs = self.buf[i + 4:i + 4 + n], self.buf[i + 4 + n]
                self.buf = self.buf[i + 5 + n:]
                if (typ + n + sum(data)) & 0xFF != cs:
                    continue
                self.parse(typ, data)
                seen.append(typ)
            if time.monotonic() >= end:
                return seen

    def parse(self, typ, d):
        f = lambda o: struct.unpack_from("<f", d, o)[0]
        st = self.state
        if typ == T_OUT and len(d) >= 12:
            st["v"], st["i"], st["p"], st["t"] = f(0), f(4), f(8), time.monotonic()
        elif typ == T_IN_V and len(d) >= 4:
            st["vin"] = f(0)
        elif typ == T_AH and len(d) >= 4:
            st["ah"] = f(0)
        elif typ == T_WH and len(d) >= 4:
            st["wh"] = f(0)
        elif typ == T_OUTPUT_ON and d:
            st["on"] = d[0] == 1
        elif typ in (T_MODEL, T_HW, T_FW):
            st[{T_MODEL: "model", T_HW: "hw", T_FW: "fw"}[typ]] = d.decode("ascii", "replace").strip("\0 ")
        elif typ == T_ALL and len(d) >= 110:
            st.update(vin=f(0), vset=f(4), iset=f(8), v=f(12), i=f(16), p=f(20), temp=f(24), on=d[107] == 1,
                      cv=d[109] == 1, t=time.monotonic())

    def everything(self):
        self.get(T_ALL)
        self.poll(0.4)
        return self.state

    def reading(self, stale=1.0):
        """The latest output V, I, P: from the supply's own stream, or asked for when it's older than `stale` s."""
        self.poll()
        if time.monotonic() - self.state.get("t", 0) > stale:
            self.get(T_ALL)
            self.poll(0.3)
        return self.state.get("v"), self.state.get("i"), self.state.get("p")


def log(d, seconds, csv=None):
    out = open(csv, "w") if csv else None
    if out:
        out.write("t_s,volts,amps,watts\n")
    rows, t0, last = [], time.monotonic(), None
    while time.monotonic() - t0 < seconds:
        d.poll(0.1)
        t = d.state.get("t")
        if t is None or t == last:
            if t is None or time.monotonic() - t > 1.0:
                d.get(T_ALL)
            continue
        last = t
        row = (t - t0, d.state["v"], d.state["i"], d.state["p"])
        rows.append(row)
        if out:
            out.write("%.3f,%.4f,%.4f,%.4f\n" % row)
            out.flush()  # every reading on disk as it comes (a stopped log keeps what it had)
    if out:
        out.close()
    if not rows:
        sys.exit("no readings from the supply")
    amps = [r[2] for r in rows]
    span = rows[-1][0] - rows[0][0] or 1e-9
    # Trapezoid over the samples for charge and energy.
    mah = sum((b[0] - a[0]) * (a[2] + b[2]) / 2 for a, b in zip(rows, rows[1:])) / 3.6
    mwh = sum((b[0] - a[0]) * (a[3] + b[3]) / 2 for a, b in zip(rows, rows[1:])) / 3.6
    print(f"{len(rows)} samples over {span:.1f} s ({len(rows) / span:.1f}/s) at {statistics.mean(r[1] for r in rows):.3f} V")
    print(f"current mA: mean {1000 * statistics.mean(amps):.1f}  median {1000 * statistics.median(amps):.1f}  "
          f"min {1000 * min(amps):.1f}  max {1000 * max(amps):.1f}")
    print(f"power mW: mean {1000 * statistics.mean(r[3] for r in rows):.0f};  used {mah:.2f} mAh, {mwh:.1f} mWh")
    return rows


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("port")
    ap.add_argument("cmd", choices=["info", "set", "on", "off", "log"])
    ap.add_argument("args", nargs="*", type=float)
    ap.add_argument("--csv")
    ap.add_argument("--max-volts", type=float, default=8.4)
    a = ap.parse_args()
    with Dps150(a.port) as d:
        if a.cmd == "info":
            for t in (T_MODEL, T_HW, T_FW):
                d.get(t)
            d.poll(0.3)
            st = d.everything()
            for k in ("model", "hw", "fw", "vin", "vset", "iset", "on", "cv", "v", "i", "p", "temp"):
                if k in st:
                    print(f"{k:6} {st[k]:.3f}" if isinstance(st[k], float) else f"{k:6} {st[k]}")
        elif a.cmd == "set":
            if len(a.args) != 2:
                sys.exit("set VOLTS AMPS")
            v, i = a.args
            if v > a.max_volts:
                sys.exit(f"{v} V is over {a.max_volts} V (2S Li-ion); --max-volts to override")
            d.set_float(T_VSET, v)
            d.set_float(T_ISET, i)
            st = d.everything()
            print(f"set {st.get('vset', float('nan')):.3f} V, {st.get('iset', float('nan')):.3f} A")
        elif a.cmd in ("on", "off"):
            d.send(SET, T_OUTPUT_ON, [1 if a.cmd == "on" else 0])
            print("output", "on" if d.everything().get("on") else "off")
        else:
            if len(a.args) != 1:
                sys.exit("log SECONDS")
            log(d, a.args[0], a.csv)


if __name__ == "__main__":
    main()
