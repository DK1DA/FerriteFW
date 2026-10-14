#!/usr/bin/env python3
"""Ferrite link protocol v1, PC side: the reference for the web app (manual chapter 29, Appendix "Link protocol v1";
the radio's side is ferrite-core's link.rs and ferrite-app's link.rs).

    pip install pyserial
    python3 tools/ferrite_link.py PORT hello
    python3 tools/ferrite_link.py PORT info
    python3 tools/ferrite_link.py PORT read-settings settings.bin
    python3 tools/ferrite_link.py PORT write-settings settings.bin
    python3 tools/ferrite_link.py PORT read-channel 12            # channel 12 as shown on the radio (1-1000)
    python3 tools/ferrite_link.py PORT write-channel 12 --name "Club rpt" [--rx 146.94]
    python3 tools/ferrite_link.py PORT delete-channel 12
    python3 tools/ferrite_link.py PORT baud 230400
    python3 tools/ferrite_link.py PORT reboot                     # into the bootloader, for flash.py
    python3 tools/ferrite_link.py PORT slot [A|B|C]             # a slot's working fields (LINK-33)
    python3 tools/ferrite_link.py PORT key [--seconds N] [--slot active|a|b|c]   # remote PTT (LINK-20)
    python3 tools/ferrite_link.py --pin 1234 PORT ...             # Bluetooth while a PIN lock is on (LINK-30)

PORT is a serial device (the programming cable, 115200 8N1), or tcp:HOST:PORT for the simulator:
`cargo run -p ferrite-sim -- --term-headless 120 --term-tcp 7300`, then `tcp:127.0.0.1:7300`.

Frames: COBS(type, seq, payload, CRC-16/CCITT-FALSE little-endian) then 0x00. Each request is answered with
type | 0x80, the same seq, and a payload starting with a status byte.

The cable carries the terminal or the link, one at a time (TERM-2; LINK-28 for the cable against Bluetooth). With
0 9 2 4 Cable port on Auto (or Link) this tool takes it: it sends Ctrl-C first, which closes an open terminal (quit
term.py or any terminal program holding the serial device first: one program per device), then a 0x00, which gives
the cable to the link. The link keeps it while frames come and lets go 10 s after the last; then Enter in a terminal
program opens the terminal again. With Cable port on Terminal or KISS the link never answers.

`key` (LINK-20): keys like a PTT press, with every transmit check, never on an emergency frequency; sends a keepalive
every 0.5 s (the radio unkeys after 2 s without one) and KEY off after --seconds (default 5) or Ctrl-C, always.
A refusal prints the radio's reason. On the cable a `tx` build sends nothing while keyed (TX-43): the answers wait on
the radio and arrive after the unkey, so this tool doesn't wait for them while keyed (a `tx-dry` build answers at
once). A PTT pressed on the radio ends the remote transmission (the radio's operator wins).
"""
import argparse
import socket
import struct
import sys
import time

PROTOCOL = 2
HELLO, INFO, AUTH, READ, WRITE, DELETE, BEGIN, END, SLOT, BAUD, REBOOT, KEY = (0x01, 0x02, 0x04, 0x10, 0x11, 0x12, 0x13,
                                                                           0x14, 0x16, 0x20, 0x21, 0x30)
SETTINGS, CHANNEL, ZONE, PRESET = 1, 2, 3, 4
# KEY's remote PTT (Appendix G): key 0x80, then the action (0 off, 1 on + slot, 2 keepalive); slot 0xFF: the active one
PTT, PTT_OFF, PTT_ON, PTT_ALIVE, ACTIVE = 0x80, 0, 1, 2, 0xFF
SLOTS = {"active": ACTIVE, "a": 0, "b": 1, "c": 2}
STATUS = ["ok", "bad frame", "unknown type", "bad argument", "refused", "not authorised (send AUTH: --pin)",
          "pending (asking on the radio)", "busy", "not found", "invalid", "cancelled"]
MODES = ["FM", "NFM", "AM"]


def crc16(data):
    crc = 0xFFFF
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def cobs_encode(src):
    out, code_at, code = bytearray([0]), 0, 1
    for b in src:
        if b == 0:
            out[code_at] = code
            code_at, code = len(out), 1
            out.append(0)
        else:
            out.append(b)
            code += 1
            if code == 0xFF:
                out[code_at] = code
                code_at, code = len(out), 1
                out.append(0)
    out[code_at] = code
    return bytes(out)


def cobs_decode(src):
    out, i = bytearray(), 0
    while i < len(src):
        code = src[i]
        if code == 0 or i + code > len(src):
            raise ValueError("bad COBS")
        out += src[i + 1:i + code]
        i += code
        if code < 0xFF and i < len(src):
            out.append(0)
    return bytes(out)


class Port:
    def __init__(self, spec):
        if spec.startswith("tcp:"):
            _, host, port = spec.split(":")
            self.sock = socket.create_connection((host, int(port)), timeout=3)
            self.ser = None
        else:
            import serial
            self.ser = serial.Serial(spec, 115200, timeout=0.05)
            self.sock = None
        self.buf = bytearray()

    def write(self, b):
        (self.sock.sendall if self.sock else self.ser.write)(b)

    def read(self):
        if self.sock:
            self.sock.settimeout(0.05)
            try:
                return self.sock.recv(4096)
            except socket.timeout:
                return b""
        return self.ser.read(4096)

    def baud(self, rate):
        if self.ser:
            self.ser.baudrate = rate

    def close(self):
        (self.sock or self.ser).close()


class Link:
    def __init__(self, port):
        self.p, self.seq = port, 0
        # Ctrl-C closes the terminal if it has the cable (TERM-2; nothing otherwise); its goodbye text is dropped.
        self.p.write(b"\x03")
        time.sleep(0.3)
        self.p.read()
        self.p.buf.clear()
        self.p.write(b"\x00")  # TERM-2: a 0x00 gives the cable to the link

    def send(self, ty, payload=b""):
        """One request, not waiting for its answer; its seq."""
        self.seq = (self.seq + 1) & 0xFF
        body = bytes([ty, self.seq]) + payload
        self.p.write(cobs_encode(body + struct.pack("<H", crc16(body))) + b"\x00")
        return self.seq

    def answers(self):
        """The whole answers received so far: (type, seq, status, rest) each."""
        self.p.buf += self.p.read()
        out = []
        while b"\x00" in self.p.buf:
            raw, _, rest = bytes(self.p.buf).partition(b"\x00")
            self.p.buf = bytearray(rest)
            if not raw:
                continue
            try:
                f = cobs_decode(raw)
            except ValueError:
                continue  # terminal text, or a damaged frame
            if len(f) < 5 or crc16(f[:-2]) != struct.unpack("<H", f[-2:])[0]:
                continue
            out.append((f[0], f[1], f[2], f[3:-2]))
        return out

    def ask(self, ty, payload=b"", timeout=2.0, retry_pending=30):
        for _ in range(retry_pending + 1):
            seq = self.send(ty, payload)
            end = time.time() + timeout
            pending = False
            while time.time() < end and not pending:
                for t, s, status, rest in self.answers():
                    if t == ty | 0x80 and s == seq:
                        if status == 6:  # pending: the radio asks its owner; ask again
                            time.sleep(1)
                            pending = True
                            break
                        return status, rest
            if not pending:
                raise TimeoutError(f"no answer to 0x{ty:02X}")
        raise TimeoutError("the radio is still asking its owner")

    def ok(self, ty, payload=b""):
        s, rest = self.ask(ty, payload)
        if s != 0:
            why = rest.decode("ascii", "replace") if s == 9 else ""
            sys.exit(f"0x{ty:02X}: {STATUS[s] if s < len(STATUS) else s} {why}".strip())
        return rest

    def read(self, what, idx):
        rec = bytearray()
        while True:
            a = self.ok(READ, struct.pack("<BHHB", what, idx, len(rec), 200))
            total = struct.unpack("<H", a[:2])[0]
            rec += a[2:]
            if len(rec) >= total:
                return bytes(rec)

    def write(self, what, idx, rec, part=200):
        for off in range(0, max(len(rec), 1), part):
            self.ok(WRITE, struct.pack("<BHHH", what, idx, off, len(rec)) + rec[off:off + part])


def why(status, rest):
    s = STATUS[status] if status < len(STATUS) else str(status)
    return f"{s}: {rest.decode('ascii', 'replace')}" if status == 9 else s


def remote_ptt(link, seconds, slot):
    """LINK-20: key, keepalives every 0.5 s, KEY off after `seconds` or Ctrl-C: always, in `finally`, refused or not."""
    ended = None
    on = None
    try:
        on = link.send(KEY, bytes([PTT, PTT_ON, SLOTS[slot]]))
        keyed = None
        end = time.time() + 0.6
        while time.time() < end and keyed is None:
            for t, s, status, rest in link.answers():
                if t == KEY | 0x80 and s == on:
                    keyed = status == 0
                    if keyed:
                        print(f"keyed: slot {'ABC'[rest[0]] if rest and rest[0] < 3 else '?'}")
                    else:
                        ended = f"refused: {why(status, rest)}"
                        print(ended)
        if keyed is None:
            print("keyed, as far as can be told: the cable holds its answers while transmitting (TX-43)")
        stop = time.time() + seconds
        while time.time() < stop and ended is None:
            link.send(KEY, bytes([PTT, PTT_ALIVE]))
            nxt = min(time.time() + 0.5, stop)
            while time.time() < nxt and ended is None:
                for t, s, status, rest in link.answers():
                    if t == KEY | 0x80 and status != 0:
                        ended = f"the radio unkeyed: {why(status, rest)}"
                        print(ended)
    except KeyboardInterrupt:
        print("Ctrl-C")
    finally:
        off = link.send(KEY, bytes([PTT, PTT_OFF]))
        # Answers held while keyed (TX-43) come out after the unkey; KEY off's own is the last.
        end = time.time() + 3
        done = False
        while time.time() < end and not done:
            for t, s, status, rest in link.answers():
                if t != KEY | 0x80:
                    continue
                if s == off:
                    done = True
                elif status != 0 and ended is None:
                    ended = f"the radio unkeyed: {why(status, rest)}"
                    print(ended)
        print("unkeyed: KEY off answered" if done else "KEY off sent; no answer within 3 s")
    return 1 if ended else 0


def text(b):
    n = b[0]
    return b[1:1 + n].decode("ascii", "replace"), b[1 + n:]


def channel_fields(rec):
    num, zone, rx = struct.unpack("<HBI", rec[:7])
    name = rec[7:23].split(b"\x00")[0].decode("ascii", "replace")
    return num, zone, rx, name


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("port")
    ap.add_argument("cmd")
    ap.add_argument("args", nargs="*")
    ap.add_argument("--name")
    ap.add_argument("--rx", type=float, help="RX frequency in MHz")
    ap.add_argument("--seconds", type=float, default=5.0, help="key: how long to stay keyed")
    ap.add_argument("--slot", choices=sorted(SLOTS), default="active",
                    help="key: the active slot (PTT A), b (PTT B), or a/c only while active")
    ap.add_argument("--pin", help="the radio's PIN, for Bluetooth while a PIN lock is on (LINK-30)")
    a = ap.parse_args()
    port = Port(a.port)
    try:
        sys.exit(run(Link(port), a))
    finally:
        port.close()


def run(link, a):
    hello = link.ok(HELLO, bytes([PROTOCOL]) + bytes(8) + b"ferrite_link.py")
    if not hello[2]:
        if not a.pin:
            sys.exit("the radio's PIN lock is on: give --pin (LINK-30)")
        link.ok(AUTH, a.pin.encode("ascii"))
    if a.cmd == "key":
        return remote_ptt(link, a.seconds, a.slot)
    if a.cmd == "slot":
        for n in [ "ABC".index(x.upper()) for x in a.args] or [0, 1, 2]:
            r = link.ok(SLOT, bytes([n]))
            s, act, ch, f, mode, pw, rxt, txt, off, fl = struct.unpack("<BBHIBBHHiB", r[:19])
            where = f"ch {ch + 1}" if ch != 0xFFFF else ("aprs" if fl & 2 else "vfo")
            print(f"{'ABC'[s]}{'*' if act else ' '} {where} {f / 100000:.5f} MHz {MODES[mode]} "
                  f"{['low', 'mid', 'high'][pw]} txtone 0x{txt:04X} rxtone 0x{rxt:04X} "
                  f"{'split tx ' + str(off / 100000) if fl & 1 else 'offset ' + str(off / 100000)}"
                  f"{' scanning' if fl & 4 else ''}")
        return 0
    if a.cmd == "hello":
        ver, _ = text(hello[3:])
        print(f"protocol {hello[0]}, max payload {hello[1]}, may write {bool(hello[2])}, firmware {ver}")
    elif a.cmd == "info":
        r = link.ok(INFO)
        ver, r = text(r)
        chans, zones, presets, slen, region = struct.unpack("<HBBHB", r[:7])
        call, _ = text(r[7:])
        print(f"firmware {ver}; {chans} channels, {zones} zones, {presets} presets; settings {slen} B; "
              f"region {region}; callsign {call or '-'}")
    elif a.cmd == "read-settings":
        rec = link.read(SETTINGS, 0)
        open(a.args[0], "wb").write(rec)
        print(f"{len(rec)} bytes to {a.args[0]} (PIN and tokens read as zeros)")
    elif a.cmd == "write-settings":
        rec = open(a.args[0], "rb").read()
        link.write(SETTINGS, 0, rec)
        print(f"{len(rec)} bytes written (region, Extend TX, PIN and cable port keep the radio's)")
    elif a.cmd == "read-channel":
        n = int(a.args[0]) - 1
        rec = link.read(CHANNEL, n)
        num, zone, rx, name = channel_fields(rec)
        print(f"channel {num + 1}: {name!r} {rx / 100000:.5f} MHz zone {zone if zone != 255 else 'none'}; "
              f"{len(rec)} bytes: {rec.hex()}")
    elif a.cmd == "write-channel":
        n = int(a.args[0]) - 1
        rec = bytearray(link.read(CHANNEL, n))
        if a.name is not None:
            rec[7:23] = a.name.encode("ascii")[:16].ljust(16, b"\x00")
        if a.rx is not None:
            rec[3:7] = struct.pack("<I", round(a.rx * 100000))
        link.ok(BEGIN, struct.pack("<H", 1))
        link.write(CHANNEL, n, bytes(rec))
        link.ok(END)
        print(f"channel {n + 1} written: {channel_fields(rec)[3]!r}")
    elif a.cmd == "delete-channel":
        n = int(a.args[0]) - 1
        link.ok(DELETE, struct.pack("<BH", CHANNEL, n))
        print(f"channel {n + 1} deleted")
    elif a.cmd == "baud":
        rate = int(a.args[0])
        link.ok(BAUD, struct.pack("<I", rate))
        time.sleep(0.05)
        link.p.baud(rate)
        link.ok(HELLO, bytes([PROTOCOL]) + bytes(8) + b"ferrite_link.py")  # within 1 s, or the radio goes back
        print(f"now at {rate}")
    elif a.cmd == "reboot":
        link.ok(REBOOT)  # LINK-24: answered, then the radio saves, goes quiet and resets into the bootloader
        print("restarting: the bootloader listens about 200 ms (run flash.py now, or it starts Ferrite again)")
    else:
        sys.exit(f"unknown command {a.cmd}; see --help")
    return 0


if __name__ == "__main__":
    main()
