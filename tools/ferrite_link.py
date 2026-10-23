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
    python3 tools/ferrite_link.py PORT used [channel|zone|preset]  # which numbers hold a record (LINK-39)
    python3 tools/ferrite_link.py PORT readmany [channel|zone|preset] [FIRST [COUNT]]  # whole records, several a frame (LINK-40)
    python3 tools/ferrite_link.py PORT log [N]                    # the event log, newest first (LINK-42)
    python3 tools/ferrite_link.py PORT baud 230400
    python3 tools/ferrite_link.py PORT reboot                     # into the bootloader, for flash.py
    python3 tools/ferrite_link.py PORT term "LINE" ["LINE" ...]   # terminal command lines over the link (LINK-34)
    python3 tools/ferrite_link.py PORT slot [A|B|C]             # a slot's working fields (LINK-33)
    python3 tools/ferrite_link.py PORT key [--seconds N] [--slot active|a|b|c]   # remote PTT (LINK-20)
    python3 tools/ferrite_link.py PORT keys "MENU 0 8 2 1 OK" [--long]   # remote keys (LINK-19); NAME:long|down|up
    python3 tools/ferrite_link.py PORT knob +3                    # knob steps (LINK-19)
    python3 tools/ferrite_link.py PORT ui [keys]                  # what the screen shows (LINK-19's readback)
    python3 tools/ferrite_link.py PORT status                     # the compact one-line status (LINK-19, what=2)
    python3 tools/ferrite_link.py PORT aprs-cfg                   # the APRS settings record (LINK-43)
    python3 tools/ferrite_link.py PORT heard                      # the APRS heard list (LINK-44)
    python3 tools/ferrite_link.py PORT msgs                       # APRS messages in and out, newest first (LINK-44)
    python3 tools/ferrite_link.py PORT msg-send CALL "TEXT"       # queue an APRS message as compose does (LINK-45)
    python3 tools/ferrite_link.py PORT subscribe [heard,msgs,ui] [--seconds N] [--screen]   # print pushed events
    python3 tools/ferrite_link.py --pin 1234 PORT ...             # Bluetooth while a PIN lock is on (LINK-30)

PORT is a serial device (the programming cable, 115200 8N1), tcp:HOST:PORT for the simulator
(`cargo run -p ferrite-sim -- --term-headless 120 --term-tcp 7300`, then `tcp:127.0.0.1:7300`), or ble:NAME_OR_ADDRESS
for the radio's Bluetooth (0 9 2 1 on; the BT513 advertises as "walkie-talkie"; on macOS the address is the
CoreBluetooth UUID): `ble:walkie-talkie`. Needs bleak (`pip install bleak`). The link runs on service FFE0,
characteristic FFE1 (write without response, notify), in 20-byte writes paced for the module's 9600-baud UART
(LINK-25): about 0.8 KB/s each way, so a UI readback takes a second or so. FF31 (the module's own control) is never
touched. The cable's session and a Bluetooth one run at the same time (LINK-28).

Frames: COBS(type, seq, payload, CRC-16/CCITT-FALSE little-endian) then 0x00. Each request is answered with
type | 0x80, the same seq, and a payload starting with a status byte.

The cable carries the terminal or the link, one at a time (TERM-2; LINK-28 for the cable against Bluetooth). With
0 9 2 3 Cable port on Auto (or Link) this tool takes it: it sends Ctrl-C first, which closes an open terminal (quit
term.py or any terminal program holding the serial device first: one program per device), then a 0x00, which gives
the cable to the link. The link keeps it while frames come and lets go 10 s after the last; then Enter in a terminal
program opens the terminal again. With Cable port on Terminal or KISS the link never answers.

`term "LINE"` (LINK-34) runs terminal command lines (`slot`, `set`, `get`, `bk`, `mic`, `txtest`, `help`, ...) on this
one link session, as the terminal's command line would, and prints their output; no switching to the terminal.

`key` (LINK-20): keys like a PTT press, with every transmit check, never on an emergency frequency; sends a keepalive
every 0.5 s (the radio unkeys after 2 s without one) and KEY off after --seconds (default 5) or Ctrl-C, always.
A refusal prints the radio's reason. On the cable a `tx` build sends nothing while keyed (TX-43): the answers wait on
the radio and arrive after the unkey, so this tool doesn't wait for them while keyed (a `tx-dry` build answers at
once). A PTT pressed on the radio ends the remote transmission (the radio's operator wins).

`keys` and `knob` (LINK-19) press the keypad, the side keys and the knob as a person would: the same path as the
keys (long press, lights, wake, lock). Names: MENU (OK), BACK (EXIT), UP, DOWN, LEFT, RIGHT, SLOT (ABC), VM, 0-9,
STAR (*), HASH (#), SIDE1 (PF1), SIDE2 (PF2); `ui keys` lists them with their stock codes. A key is a short press,
NAME:long a long one (held 750 ms), NAME:down / NAME:up hold and let go. Never the PTTs: a remote key never transmits, but for the crossband repeater's # on its start page (XB-20.2).
`ui` prints what the screen shows (the screen, status bar, home cards or list rows, hints, toast). `status` prints
the compact status, one short line (`st scr=Home act=A keyed=- remote=0 mode=off A=RX/20 B=-/0 C=-/0 xb=0`): the
facts tests poll, cheap on Bluetooth where the whole readback takes seconds.

As a library (the test runner's):

    from ferrite_link import Radio
    with Radio("tcp:127.0.0.1:7300") as r:      # or "/dev/cu.usbserial-114220", or "ble:walkie-talkie" (pin="1234")
        r.keys("MENU 0 7")                      # or r.press("MENU"), r.press("HASH", long=True)
        r.knob(+2)
        ui = r.ui()                             # {"screen": {...}, "rows": [...], "sel": {...}, "hints": [...]}
        assert ui["screen"]["id"] == "Group" and ui["sel"]["name"] == "Theme"
"""
import argparse
import socket
import struct
import sys
import time

PROTOCOL = 2
HELLO, INFO, AUTH, READ, WRITE, DELETE, BEGIN, END, SLOT, BAUD, REBOOT, KEY, CMD, MORE, UI = (
    0x01, 0x02, 0x04, 0x10, 0x11, 0x12, 0x13, 0x14, 0x16, 0x20, 0x21, 0x30, 0x31, 0x32, 0x33)
USED, READ_MANY, WRITE_MANY, LOG_READ = 0x15, 0x17, 0x18, 0x40  # LINK-39 - LINK-42
LOG_SOURCES = ["APRS", "Pager", "SAME", "DSC", "AIS", "DTMF", "Selcall", "GPS"]  # LINK-42's source byte
SUBSCRIBE, EVENT, HEARD, MSGS, MSG_SEND = 0x50, 0x51, 0x52, 0x53, 0x54  # LINK-43 - LINK-48
SETTINGS, CHANNEL, ZONE, PRESET, APRS = 1, 2, 3, 4, 5
CONTACT = 6  # APRS-42
ONE_FRAME = (CHANNEL, ZONE, PRESET, CONTACT)  # READ_MANY/WRITE_MANY only; READ/WRITE are for SETTINGS and APRS (LINK-21)
# SUBSCRIBE's mask and flags, EVENT's kinds (Appendix G)
SUB_HEARD, SUB_MSGS, SUB_UI, RENEW = 1, 2, 4, 1
EV_HEARD, EV_MSG, EV_MSG_STATE, EV_UI = 1, 2, 3, 4
WHATS = {"channel": CHANNEL, "zone": ZONE, "preset": PRESET}
# KEY's remote PTT (Appendix G): key 0x80, then the action (0 off, 1 on + slot, 2 keepalive); slot 0xFF: the active one
PTT, PTT_OFF, PTT_ON, PTT_ALIVE, ACTIVE = 0x80, 0, 1, 2, 0xFF
SLOTS = {"active": ACTIVE, "a": 0, "b": 1, "c": 2}
# KEY's keypad (LINK-19): stock's key code, then the action (0 up, 1 down, 2 short, 3 long); 0x81 the knob + i8 steps
KEY_UP, KEY_DOWN, KEY_SHORT, KEY_LONG, KNOB = 0, 1, 2, 3, 0x81
LONG_HOLD = 0.75  # the radio lets a remote long press go 750 ms after it went down
KEY_CODES = {"MENU": 6, "OK": 6, "BACK": 9, "EXIT": 9, "UP": 7, "DOWN": 8, "LEFT": 4, "RIGHT": 5, "SLOT": 3,
             "ABC": 3, "VM": 2, "V/M": 2, "0": 17, "1": 10, "2": 11, "3": 12, "4": 14, "5": 15, "6": 16, "7": 18,
             "8": 19, "9": 20, "STAR": 13, "*": 13, "HASH": 21, "#": 21, "SIDE1": 0, "PF1": 0, "SIDE2": 1, "PF2": 1}
ACTIONS = {"short": KEY_SHORT, "long": KEY_LONG, "down": KEY_DOWN, "up": KEY_UP}
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


BLE_DATA = "0000ffe1-0000-1000-8000-00805f9b34fb"  # FFE1: the BT513's transparent UART (FF31 never touched)
BLE_CHUNK = 20  # the ATT payload of the default 23-byte MTU
BLE_PACE = 0.025  # seconds between chunks: 800 B/s, under the module's 9600-baud UART (960 B/s)


class BlePort:
    """The radio over Bluetooth: FFE1 writes (without response, 20 bytes, paced) and notifications, through bleak on
    its own thread and event loop so the callers stay synchronous. `target`: the advertised name or the address."""

    def __init__(self, target, timeout=15.0):
        import asyncio
        import threading
        self._asyncio = asyncio
        self._loop = asyncio.new_event_loop()
        self._thread = threading.Thread(target=self._loop.run_forever, daemon=True)
        self._thread.start()
        self._rx, self._lock, self._got = bytearray(), threading.Lock(), threading.Event()
        self.rx_total, self.rx_mark, self.last_rx, self.dropped_at = 0, 0, None, None
        self.slow = True  # paced at ~280 B/s by the radio (LINK-36): answers take longer than on the cable
        self.buf = bytearray()
        self.client = None
        self.client = self._run(self._connect(target, timeout), timeout + 15)

    def _run(self, coro, timeout=10.0):
        return self._asyncio.run_coroutine_threadsafe(coro, self._loop).result(timeout)

    async def _connect(self, target, timeout):
        from bleak import BleakClient, BleakScanner

        def match(dev, adv):
            names = {dev.name or "", getattr(adv, "local_name", None) or ""}
            return dev.address.lower() == target.lower() or target in names

        dev = await BleakScanner.find_device_by_filter(match, timeout=timeout)
        if dev is None:
            raise OSError(f"no Bluetooth device {target!r} found in {timeout:.0f} s (0 9 2 1 on? in range?)")
        client = BleakClient(dev, disconnected_callback=self._gone)
        await client.connect()
        await client.start_notify(BLE_DATA, self._notify)
        return client

    def _notify(self, _char, data):
        with self._lock:
            self._rx += data
            self.rx_total += len(data)
            self.last_rx = time.time()
        self._got.set()

    def _gone(self, _client):
        """The Bluetooth connection itself dropped (not just a slow answer): remembered for the time-out message."""
        self.dropped_at = time.time()
        print("ble: disconnected", file=sys.stderr)

    def diagnose(self, since):
        """Why an answer didn't come: the connection dropped, or it's up and the radio was silent or still sending."""
        if self.dropped_at:
            return f"Bluetooth disconnected {time.time() - self.dropped_at:.1f} s ago"
        if self.last_rx is None or self.last_rx < since:
            return "Bluetooth still connected; the radio sent nothing since the request"
        return (f"Bluetooth still connected; the radio was still sending ({self.rx_total - self.rx_mark} bytes since "
                f"the request, the last {time.time() - self.last_rx:.1f} s ago): slow, not dropped")

    def write(self, b):
        for i in range(0, len(b), BLE_CHUNK):
            self._run(self.client.write_gatt_char(BLE_DATA, bytes(b[i:i + BLE_CHUNK]), response=False))
            time.sleep(BLE_PACE)

    def read(self):
        self._got.wait(0.05)
        with self._lock:
            out = bytes(self._rx)
            self._rx.clear()
            self._got.clear()
        return out

    def baud(self, rate):
        pass  # Bluetooth's rate is the module's (LINK-15)

    def close(self):
        try:
            if self.client is not None:
                self._run(self.client.disconnect())
        finally:
            self._loop.call_soon_threadsafe(self._loop.stop)


def Port(spec):  # noqa: N802 (a class once; callers construct it)
    """The transport for PORT: ble:NAME_OR_ADDRESS, tcp:HOST:PORT, or a serial device."""
    if spec.startswith("ble:"):
        return BlePort(spec[4:])
    return StreamPort(spec)


class StreamPort:
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
        self.events = []  # EVENT frames (LINK-46) as they arrive: (event counter, payload)
        # Ctrl-C closes the terminal if it has the cable (TERM-2; nothing otherwise); its goodbye text is dropped.
        self.p.write(b"\x03")
        time.sleep(0.3)
        self.p.read()
        self.p.buf.clear()
        self.p.write(b"\x00")  # TERM-2: a 0x00 gives the cable to the link
        self.sent_at = time.time()

    # TERM-2: the radio keeps the port for the link while frames or syncs come within 10 s (link.rs ACTIVE_MS);
    # after a longer quiet, in auto, a frame's bytes go to the terminal until its closing 0x00 and the frame is
    # lost. A sync before a frame sent after this much quiet gives the port back first.
    QUIET_S = 5.0

    def send(self, ty, payload=b""):
        """One request, not waiting for its answer; its seq."""
        self.seq = (self.seq + 1) & 0xFF
        body = bytes([ty, self.seq]) + payload
        sync = b"\x00" if time.time() - self.sent_at >= self.QUIET_S else b""
        self.p.write(sync + cobs_encode(body + struct.pack("<H", crc16(body))) + b"\x00")
        self.sent_at = time.time()
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
            if f[0] == EVENT:  # pushed by the radio, not an answer (LINK-46): its type has no 0x80 bit
                self.events.append((f[1], bytes(f[2:-2])))
                continue
            out.append((f[0], f[1], f[2], f[3:-2]))
        return out

    def ask(self, ty, payload=b"", timeout=None, retry_pending=30):
        slow = getattr(self.p, "slow", False)
        if timeout is None:
            timeout = 6.0 if slow else 2.0  # Bluetooth: a 224-byte chunk is ~0.8 s idle, over 2 s on a busy radio
        resynced = False
        for _ in range(retry_pending + 2):
            t0 = time.time()
            if slow:
                self.p.rx_mark = self.p.rx_total
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
                # A frame sent just after the radio restarted (or after 10 s of quiet) can go to the terminal (TERM-2):
                # a read-only request with nothing back is sent once more after a 0x00 sync. Never a KEY, CMD, MORE
                # or write: repeating those would act twice or skip a chunk.
                if not resynced and ty in (HELLO, INFO, UI):
                    resynced = True
                    self.p.write(b"\x00")
                    self.sent_at = time.time()
                    continue
                why = f" ({self.p.diagnose(t0)})" if hasattr(self.p, "diagnose") else ""
                raise TimeoutError(f"no answer to 0x{ty:02X} in {timeout:.0f} s{why}")
        raise TimeoutError("the radio is still asking its owner")

    def ok(self, ty, payload=b""):
        s, rest = self.ask(ty, payload)
        if s != 0:
            why = rest.decode("ascii", "replace") if s == 9 else ""
            sys.exit(f"0x{ty:02X}: {STATUS[s] if s < len(STATUS) else s} {why}".strip())
        return rest

    def read(self, what, idx):
        """One record: a channel, zone or preset by READ_MANY of its index alone (LINK-21, LINK-40); the settings
        and APRS records by READ, in parts."""
        if what in ONE_FRAME:
            got = [r for i, r in self.read_many(what, idx, 1)[0] if i == idx]
            if not got:
                sys.exit(f"0x{READ_MANY:02X}: {STATUS[8]}")
            return got[0]
        rec = bytearray()
        while True:
            a = self.ok(READ, struct.pack("<BHHB", what, idx, len(rec), 200))
            total = struct.unpack("<H", a[:2])[0]
            rec += a[2:]
            if len(rec) >= total:
                return bytes(rec)

    def write(self, what, idx, rec, part=200):
        """One record: a channel, zone or preset by WRITE_MANY (LINK-41); the settings and APRS by WRITE, in parts."""
        if what in ONE_FRAME:
            self.write_many(what, [(idx, rec)])
            return
        for off in range(0, max(len(rec), 1), part):
            self.ok(WRITE, struct.pack("<BHHH", what, idx, off, len(rec)) + rec[off:off + part])

    def used(self, what):
        """LINK-39: the indexes of `what` that hold a record."""
        a = self.ok(USED, bytes([what]))
        n = struct.unpack("<H", a[:2])[0]
        return [i for i in range(n) if a[2 + i // 8] >> (i % 8) & 1]

    def read_many(self, what, first=0, count=None):
        """LINK-40: every record held from `first`, `count` indexes on (default: to the end; channels are 0-999):
        [(index, record)], and the requests it took."""
        if count is None:
            count = (1000 if what == CHANNEL else 256) - first
        out, asks, end = [], 0, first + count
        while first < end:
            a = self.ok(READ_MANY, struct.pack("<BHH", what, first, min(end - first, 0xFFFF)))
            asks += 1
            nxt, r = struct.unpack("<H", a[:2])[0], a[2:]
            while r:
                i, n = struct.unpack("<HB", r[:3])
                out.append((i, bytes(r[3:3 + n])))
                r = r[3 + n:]
            if nxt <= first:
                break  # the radio's last index (zones and presets end where they end)
            first = nxt
        return out, asks

    def write_many(self, what, recs):
        """LINK-41: (index, record) pairs, as many as fit in one frame each; raises on a refusal with how many went."""
        frame, sent = bytearray([what]), 0

        def flush():
            nonlocal sent
            if len(frame) > 1:
                s, rest = self.ask(WRITE_MANY, bytes(frame))
                if s != 0:
                    done = rest[0] if rest and s in (7, 9, 10) else 0
                    sys.exit(f"WRITE_MANY: {sent + done} written, then {why(s, rest[1:])}")
                sent += rest[0]
            del frame[1:]

        for i, rec in recs:
            item = struct.pack("<HB", i, len(rec)) + rec
            if len(frame) + len(item) > 240:
                flush()
            frame += item
        flush()
        return sent

    def log(self, most=0):
        """LINK-42: the event log newest first, `most` records (0: all): dicts."""
        out, before = [], 0xFFFFFFFF
        while True:
            a = self.ok(LOG_READ, struct.pack("<I", before))
            top, kept, n = struct.unpack("<IHB", a[:7])
            r = a[7:]
            for _ in range(n):
                seq, ago, src, slot, flags, freq, ident, x, y = struct.unpack("<IIBBBIIHH", r[:23])
                r = r[23:]
                pos = None
                if flags & 2:
                    lat, lon = struct.unpack("<ii", r[:8])
                    pos, r = (lat / 1e7, lon / 1e7), r[8:]
                name, r = text(r)
                txt, r = text(r)
                out.append({"seq": seq, "ago_s": ago, "src": LOG_SOURCES[src] if src < len(LOG_SOURCES) else src,
                            "slot": slot, "unread": bool(flags & 1), "freq": freq, "id": ident, "x": x, "y": y,
                            "pos": pos, "name": name, "text": txt})
                before = seq
                if most and len(out) >= most:
                    return out
            if n == 0:
                return out


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


class RadioError(Exception):
    """The radio refused a request: `status` (STATUS's index) and its reason."""

    def __init__(self, status, rest=b""):
        self.status = status
        super().__init__(why(status, rest))


def key_code(name):
    try:
        return KEY_CODES[name.upper()]
    except KeyError:
        raise ValueError(f"no key {name!r}: {', '.join(sorted(set(KEY_CODES)))}") from None


def parse_ui_line(line):
    """One `ui` line: (kind, [positional strings], {key: value}). Strings are in double quotes with \" and \\."""
    pos, kv, i, n = [], {}, 0, len(line)

    def quoted(i):
        out, i = [], i + 1
        while i < n and line[i] != '"':
            if line[i] == "\\" and i + 1 < n:
                i += 1
            out.append(line[i])
            i += 1
        return "".join(out), i + 1

    while i < n:
        if line[i] == " ":
            i += 1
        elif line[i] == '"':
            v, i = quoted(i)
            pos.append(v)
        else:
            j = i
            while j < n and line[j] not in ' ="':
                j += 1
            word = line[i:j]
            if j < n and line[j] == "=":
                if j + 1 < n and line[j + 1] == '"':
                    v, i = quoted(j + 1)
                else:
                    k = j + 1
                    while k < n and line[k] != " ":
                        k += 1
                    v, i = line[j + 1:k], k
                kv[word] = int(v) if v.lstrip("-").isdigit() else v
            else:
                pos.append(word)
                i = j
    return (pos[0] if pos else ""), pos[1:], kv


def parse_ui(text):
    """`ui`'s lines as a dict: screen, state, bar, toast/popup, qmenu, cards, rows (and sel, the selected row),
    help, hints and the screen's own lines (value, ask, entry, ...) under their first word; `lines` is the text."""
    ui = {"lines": text.splitlines(), "cards": [], "rows": [], "qmenu": [], "toast": None, "popup": None, "sel": None,
          "hints": [], "pvrows": []}
    for line in ui["lines"]:
        kind, pos, kv = parse_ui_line(line)
        if kind == "screen":
            ui["screen"] = {"id": pos[0] if pos else "", "name": pos[1] if len(pos) > 1 else "", **kv}
        elif kind == "card":
            ui["cards"].append({"slot": pos[0] if pos else "", **kv})
        elif kind == "row":
            num, rest = (int(pos[0]), pos[1:]) if pos and pos[0].isdigit() else (None, pos)
            r = {"num": num, "name": rest[0] if rest else "", "value": rest[1] if len(rest) > 1 else "",
                 "sel": "sel" in rest[2:], "dot": "dot" in rest[2:], "grey": "grey" in rest[2:]}
            r.update({k: v for k, v in kv.items() if k not in r})  # a screen's own fields (kind=, chips=)
            ui["rows"].append(r)
            if r["sel"]:
                ui["sel"] = r
        elif kind in ("toast", "popup"):
            ui[kind] = {"head": pos[0] if pos else "", "body": pos[1] if len(pos) > 1 else ""}
        elif kind == "qmenu":
            ui["qmenu"].append(pos)
        elif kind == "bcast":
            ui.setdefault("bcast", {})[pos[0] if pos else ""] = {"rds": pos[1] if len(pos) > 1 else "", **kv}
        elif kind == "pvrow":
            ui.setdefault("pvrows", []).append({"name": pos[0] if pos else "", "place": pos[1] if len(pos) > 1 else "",
                                                "freq": pos[2] if len(pos) > 2 else ""})
        elif kind == "hints":
            ui["hints"] = list(zip(pos[0::2], pos[1::2]))
        elif kind:
            ui[kind] = {"args": pos, **kv} if pos or not kv else kv
    return ui


class Radio:
    """The link as a test runner uses it (LINK-19): press keys, turn the knob, read the screen back.

    `port` is a serial device (the cable or a Bluetooth serial port) or tcp:HOST:PORT for the simulator."""

    def __init__(self, port, pin=None, name=b"ferrite_link.py"):
        self.port = Port(port)
        self.link = Link(self.port)
        hello = self.link.ok(HELLO, bytes([PROTOCOL]) + bytes(8) + name)
        if not hello[2]:
            if not pin:
                raise RadioError(5)
            self._ok(AUTH, pin.encode("ascii"))
        rest = text(hello[3:])[1]
        self.build, self.bench = build_name(rest), build_bench(rest)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        self.port.close()

    def _ok(self, ty, payload=b"", busy_s=3.0):
        """A request, again while the radio answers `busy` (a long press still held, the store writing)."""
        end = time.time() + busy_s
        while True:
            s, rest = self.link.ask(ty, payload)
            if s == 0:
                return rest
            if s != 7 or time.time() > end:
                raise RadioError(s, rest)
            time.sleep(0.05)

    def _key(self, name, action):
        self._ok(KEY, bytes([key_code(name), action]))

    def press(self, name, long=False):
        """A short press, or a long one (held 750 ms: the call returns once it has been let go)."""
        self._key(name, KEY_LONG if long else KEY_SHORT)
        if long:
            time.sleep(LONG_HOLD + 0.05)

    def down(self, name):
        self._key(name, KEY_DOWN)

    def up(self, name):
        self._key(name, KEY_UP)

    def keys(self, seq, long=False):
        """Several keys: "MENU 0 8 2 1 OK"; a word NAME:long, NAME:down or NAME:up says how (short by default)."""
        for word in seq.split():
            name, _, how = word.partition(":")
            how = how.lower() or ("long" if long else "short")
            if how not in ACTIONS:
                raise ValueError(f"{word}: :long, :down or :up")
            if how in ("short", "long"):
                self.press(name, long=how == "long")
            else:
                self._key(name, ACTIONS[how])

    def knob(self, steps):
        """Knob steps, + clockwise; more than 20 go in several requests."""
        while steps:
            n = max(-20, min(20, steps))
            self._ok(KEY, bytes([KNOB, n & 0xFF]))
            steps -= n

    def ui_text(self, what=0):
        """What the screen shows, as the radio's text (0 the state, 1 the key names)."""
        out, rest = [], self._ok(UI, bytes([what]))
        while True:
            out.append(rest[1:].decode("utf-8", "replace"))
            if rest[0] == 0:
                return "".join(out)
            rest = self._ok(MORE)

    def ui(self):
        return parse_ui(self.ui_text())

    def status_text(self):
        """The compact status line (LINK-19, UI what=2), without its newline."""
        return self.ui_text(2).strip()

    def status(self):
        """The compact status as a dict (parse_status): screen, active, keyed, remote, mode, slots, xb."""
        return parse_status(self.status_text())

    def cmd(self, line):
        """A terminal command line (LINK-34); its output."""
        out, rest = [], self._ok(CMD, line.encode("ascii"))
        while True:
            out.append(rest[1:].decode("utf-8", "replace"))
            if rest[0] == 0:
                return "".join(out)
            if len(rest) == 1:
                time.sleep(0.05)
            rest = self._ok(MORE)


def parse_status(line):
    """The compact status line (LINK-19, UI what=2) as a dict:
    {"screen": "Home", "active": "A", "keyed": None or "A"/"B"/"C"/"HF", "remote": bool, "mode": "off"|"xb"|...,
     "slots": {"A": {"state": "RX", "meter": 20}, ...}, "xb": None or {"state": N, "passes": N}}.
    A slot's state is its card's word as the full readback's `card ... state="..."` has it ("idle", "RX", "heard",
    "RX muted", "MON", "scanning", ...): the line's `-` is "idle" and `_` a space."""
    words = line.split()
    if not words or words[0] != "st":
        raise ValueError(f"not a status line: {line!r}")
    kv = dict(w.split("=", 1) for w in words[1:])
    slots = {}
    for s in "ABC":
        if s in kv:
            st, _, m = kv[s].rpartition("/")
            slots[s] = {"state": "idle" if st == "-" else st.replace("_", " "), "meter": int(m)}
    xb = kv.get("xb", "0").split(",")
    return {
        "screen": kv.get("scr"),
        "active": kv.get("act"),
        "keyed": None if kv.get("keyed", "-") == "-" else kv["keyed"],
        "remote": kv.get("remote") == "1",
        "mode": kv.get("mode"),
        "slots": slots,
        "xb": {"state": int(xb[1]), "passes": int(xb[2])} if xb[0] == "1" and len(xb) == 3 else None,
    }


def text(b):
    n = b[0]
    return b[1:1 + n].decode("ascii", "replace"), b[1 + n:]


def channel_fields(rec):
    num, zone, rx = struct.unpack("<HBI", rec[:7])
    name = rec[7:23].split(b"\x00")[0].decode("ascii", "replace")
    return num, zone, rx, name


SYMS = ["Car", "Home", "Digi", "WX", "HT", "iGate", "Truck", "Boat"]
MSG_STATES = ["unread", "read", "pending", "acked", "retrying", "failed"]


def parse_station(b):
    """LINK-44: one station record; (dict, the rest)."""
    flags = b[0]
    call, b = text(b[1:])
    sym, ago, times = struct.unpack("<BIH", b[:7])
    b = b[7:]
    st = {"call": call, "sym": SYMS[sym] if sym < len(SYMS) else sym, "ago_s": ago, "times": times}
    if flags & 1:
        lat, lon = struct.unpack("<ii", b[:8])
        st["pos"], b = (lat / 1e7, lon / 1e7), b[8:]
    if flags & 2:
        st["course"], b = struct.unpack("<HH", b[:4]), b[4:]
    if flags & 4:
        st["alt_m"], b = struct.unpack("<i", b[:4])[0], b[4:]
    st["via"], b = text(b)
    st["comment"], b = text(b)
    return st, b


def parse_message(b):
    """LINK-44: one message record; (dict, the rest)."""
    inbound, state, tries, ago = struct.unpack("<BBBI", b[:7])
    peer, b = text(b[7:])
    body, b = text(b)
    st = MSG_STATES[state] if state < len(MSG_STATES) else str(state)
    if state == 4:
        st += f" {tries}"
    return {"in": bool(inbound), "peer": peer, "text": body, "ago_s": ago, "state": st}, b


def aprs_cfg(rec):
    """LINK-43: the APRS record."""
    f5k, beacon, power, path, digi, after, ssid = struct.unpack("<HBBBBBB", rec[:8])
    return {"freq_mhz": f5k * 0.005 if f5k else "region's", "beacon_mode": beacon & 0x7F, "beacon_paused": bool(beacon & 0x80),
            "power": ["low", "mid", "high"][power], "path": path, "digipeater": bool(digi & 1), "digi_mycall": bool(digi & 2),
            "digi_wait_s": digi >> 4 & 7, "beacon_after_voice": bool(after), "ssid": ssid,
            "custom_path": rec[8:24].split(b"\x00")[0].decode("ascii", "replace"),
            "fx25_tx": (rec[24] if len(rec) > 24 else None)}  # APRS-45: check bytes, 0 = off


# Live View (LINK-47, LINK-48; ferrite_core::liveview): the text dictionary, byte k (1-31) for entry k - 1.
LV_DICT_VERSION = 1
LV_DICT = [b"row ", b'" "', b" sel", b" dot", b" grey", b"card ", b" active=", b' head="', b'" state="', b" freq=",
           b' name="', b'" mode="', b'" tone="', b'" shift="', b'" power="', b'" s="S', b'" meter=', b" rx=", b" scan=",
           b" kind=slot", b'hints "', b"screen ", b'" title="', b"bar lock=", b" watch=", b" inbox=", b" battery=",
           b" clock=", b"state dark=", b" keyed=", b'qmenu "']


def lv_decode(b):
    return b"".join(LV_DICT[c - 1] if 1 <= c <= 31 else bytes([c]) for c in b)


def lv_tokens(line):
    """A line's tokens: split at the spaces outside double quotes (backslash escapes inside them)."""
    out, start, quoted, esc = [], 0, False, False
    for i, c in enumerate(line):
        if esc:
            esc = False
        elif quoted and c == 0x5C:
            esc = True
        elif c == 0x22:
            quoted = not quoted
        elif c == 0x20 and not quoted:
            out.append(line[start:i])
            start = i + 1
    out.append(line[start:])
    return out


def lv_key(t):
    for i, c in enumerate(t):
        if c in (0x3D, 0x22):
            return t[:i + 1] if c == 0x3D and i > 0 else None
    return None


def fnv1a(h, b):
    for c in b:
        h = ((h ^ c) * 0x01000193) & 0xFFFFFFFF
    return h


class LiveView:
    """A Live View client (the reference for Console): apply() each UI event's payload after its kind byte; False
    means ask for the whole state again (SUBSCRIBE without RENEW)."""

    def __init__(self):
        self.lines, self.last = [], None

    def apply(self, ev):
        seq, flags, check, n = ev[0], ev[1], struct.unpack("<H", ev[2:4])[0], ev[4]
        if flags & 1:
            self.lines = []
        elif self.last is None or (self.last + 1) & 0xFF != seq:
            return False  # lost or late
        self.lines = (self.lines + [b""] * n)[:n]
        i, ops = 0, ev[5:]
        while i < len(ops):
            op, ln = ops[i] >> 6, ops[i] & 0x3F
            if op == 0:
                k = ops[i + 1]
                self.lines[ln] = lv_decode(ops[i + 2:i + 2 + k])
                i += 2 + k
                continue
            if op == 2:
                count, at = len(lv_tokens(self.lines[ln])), i + 1
            else:
                count, at = ops[i + 1], i + 2
            entries, i = ops[at], at + 1
            toks = (lv_tokens(self.lines[ln]) + [b""] * count)[:count]
            for _ in range(entries):
                idx, k = ops[i], ops[i + 1]
                v = lv_decode(ops[i + 2:i + 2 + k])
                i += 2 + k
                j = idx & 0x7F
                toks[j] = ((lv_key(toks[j]) or b"") + v) if idx & 0x80 else v
            self.lines[ln] = b" ".join(toks)
        self.last = seq
        if not flags & 2:
            h = 0x811C9DC5
            for line in self.lines:
                h = fnv1a(fnv1a(h, line), b"\n")
            if h & 0xFFFF != check:
                return False
        return True

    def text(self):
        return "".join(line.decode("utf-8", "replace") + "\n" for line in self.lines)


def subscribe(link, what, seconds, screen):
    """LINK-46: subscribe, then print each event for `seconds`, renewing every 5 s; the UI's bytes a second."""
    mask = sum({"heard": SUB_HEARD, "msgs": SUB_MSGS, "ui": SUB_UI}[w] for w in what)
    r = link.ok(SUBSCRIBE, bytes([mask]))
    if mask & SUB_UI and r[1] != LV_DICT_VERSION:
        sys.exit(f"Live View dictionary {r[1]}: this tool knows {LV_DICT_VERSION}")
    lv, t0, renew, ui_bytes, ui_n = LiveView(), time.time(), time.time(), 0, 0
    try:
        while time.time() - t0 < seconds:
            link.answers()
            while link.events:
                _, p = link.events.pop(0)
                kind, body = p[0], p[1:]
                if kind == EV_HEARD:
                    st, _ = parse_station(body[1:])
                    print(f"heard {st['call']} ({st['sym']}, {st['times']}x, via {st['via'] or 'direct'}) {st.get('pos', 'no position')}; {body[0]} in the list")
                elif kind in (EV_MSG, EV_MSG_STATE):
                    m, _ = parse_message(body[2:])
                    what_ = "message" if kind == EV_MSG else "message state"
                    print(f"{what_}: {'from' if m['in'] else 'to'} {m['peer']}: {m['text']!r} [{m['state']}]")
                elif kind == EV_UI:
                    ui_bytes, ui_n = ui_bytes + len(p) + 6, ui_n + 1
                    if not lv.apply(body):
                        print("ui: lost one: asking for the whole state")
                        link.ok(SUBSCRIBE, bytes([mask]))
                        continue
                    if screen and not body[1] & 2:
                        sys.stdout.write("\x1b[H\x1b[2J" + lv.text())
                    elif not screen:
                        print(f"ui #{body[0]}: {len(p) + 6} B{' (whole)' if body[1] & 1 else ''}{' ...' if body[1] & 2 else ''}")
            if time.time() - renew >= 5:
                link.ok(SUBSCRIBE, bytes([mask, 0, RENEW]))  # a keepalive: events stop after 10 s quiet
                renew = time.time()
            time.sleep(0.05)
    except KeyboardInterrupt:
        pass
    link.ok(SUBSCRIBE, bytes([0]))
    if mask & SUB_UI:
        dt = max(time.time() - t0, 0.001)
        print(f"ui: {ui_n} updates, {ui_bytes} B on the wire, {ui_bytes / dt:.1f} B/s")
    return 0


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
    ap.add_argument("--long", action="store_true", help="keys: every key a long press")
    ap.add_argument("--screen", action="store_true", help="subscribe ui: redraw the screen's text on each update")
    a = ap.parse_args()
    port = Port(a.port)
    try:
        sys.exit(run(Link(port), a))
    finally:
        port.close()


BENCH_BIT = 0x80  # LINK-35: bit 7 of the build byte marks a bench build (decision 151: tx-dry is 0x82)


def build_name(rest):
    """LINK-35: the build byte after HELLO's version or INFO's callsign (absent before Oct 13, 2026): its flavour,
    bit 7 (a bench build) masked off; build_bench() says whether it is set. "tx+dry" (3): a full-power tx build with
    Dry TX on (TX-48), the PA drive held at 0 until it is turned off on the radio; build_dry() covers both dry kinds."""
    return {0: "receive", 1: "tx", 2: "tx-dry", 3: "tx+dry"}.get(rest[0] & 0x7F, f"unknown {rest[0]}") if rest else "not given"


def build_dry(rest):
    """LINK-35, TX-48: the PA drive is held at 0 now (a tx-dry build, or a tx build with Dry TX on)."""
    return build_name(rest) in ("tx-dry", "tx+dry")


def build_bench(rest):
    """LINK-35: a bench build (bit 7): the bench terminal commands (txtest, bk, mem, gpio, ...) are in it."""
    return bool(rest and rest[0] & BENCH_BIT)


UID_DIGITS = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"  # Crockford base32 (ferrite_core::uid)


def uid_text(b):
    """LINK-35.1, SELF-8: INFO's last 12 bytes (the unique ID as the MCU stores it) as About shows them: 20 base32
    digits in groups of five; None when absent (older firmware) or all zero (not read)."""
    if len(b) != 12 or not any(b):
        return None
    n = int.from_bytes(b, "big") << 4
    t = "".join(UID_DIGITS[(n >> (5 * (19 - i))) & 31] for i in range(20))
    return " ".join(t[i:i + 5] for i in range(0, 20, 5))


def run(link, a):
    hello = link.ok(HELLO, bytes([PROTOCOL]) + bytes(8) + b"ferrite_link.py")
    if not hello[2]:
        if not a.pin:
            sys.exit("the radio's PIN lock is on: give --pin (LINK-30)")
        link.ok(AUTH, a.pin.encode("ascii"))
    if a.cmd == "key":
        return remote_ptt(link, a.seconds, a.slot)
    if a.cmd in ("keys", "knob", "ui", "status"):
        r = Radio.__new__(Radio)  # this session, already past HELLO and AUTH
        r.port, r.link = link.p, link
        try:
            if a.cmd == "keys":
                r.keys(" ".join(a.args), long=a.long)
            elif a.cmd == "knob":
                r.knob(int(a.args[0]))
            elif a.cmd == "status":
                print(r.status_text())
            else:
                sys.stdout.write(r.ui_text(1 if a.args[:1] == ["keys"] else 0))
        except RadioError as e:
            print(f"{a.cmd}: {e}")
            return 1
        return 0
    if a.cmd == "term":
        bad = 0
        for line in a.args:
            status, rest = link.ask(CMD, line.encode("ascii"))
            while True:
                if status != 0:
                    print(f"{line}: {why(status, rest)}")
                    bad = 1
                    break
                sys.stdout.write(rest[1:].decode("utf-8", "replace"))
                if rest[0] == 0:
                    break
                if len(rest) == 1:
                    time.sleep(0.05)  # the radio is still working on it (a debug command)
                status, rest = link.ask(MORE)
        return bad
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
        ver, rest = text(hello[3:])
        print(f"protocol {hello[0]}, max payload {hello[1]}, may write {bool(hello[2])}, firmware {ver}, "
              f"build {build_name(rest)}{' (bench)' if build_bench(rest) else ''}")
    elif a.cmd == "info":
        r = link.ok(INFO)
        ver, r = text(r)
        chans, zones, presets, slen, region = struct.unpack("<HBBHB", r[:7])
        call, rest = text(r[7:])
        uid = uid_text(rest[1:13])
        print(f"firmware {ver} ({build_name(rest)}{', bench' if build_bench(rest) else ''}); {chans} channels, {zones} zones, {presets} presets; "
              f"settings {slen} B; region {region}; callsign {call or '-'}; unique ID {uid or 'not given'}")
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
    elif a.cmd == "used":
        what = WHATS[(a.args or ["channel"])[0]]
        got = link.used(what)
        base = 1 if what == CHANNEL else 0
        print(f"{len(got)} held: {' '.join(str(i + base) for i in got)}")
    elif a.cmd == "readmany":
        what = WHATS[(a.args or ["channel"])[0]]
        first = max(0, int(a.args[1]) - (1 if what == CHANNEL else 0)) if len(a.args) > 1 else 0
        count = int(a.args[2]) if len(a.args) > 2 else None
        recs, asks = link.read_many(what, first, count)
        for i, rec in recs:
            if what == CHANNEL:
                num, zone, rx, name = channel_fields(rec)
                print(f"channel {i + 1}: {name!r} {rx / 100000:.5f} MHz zone {zone if zone != 255 else 'none'}")
            else:
                print(f"{i}: {rec.hex()}")
        print(f"{len(recs)} records in {asks} requests")
    elif a.cmd == "log":
        for e in link.log(int(a.args[0]) if a.args else 0):
            where = f" {e['pos'][0]:.5f},{e['pos'][1]:.5f}" if e["pos"] else ""
            print(f"#{e['seq']} {e['ago_s']} s ago {e['src']} {e['freq'] / 100000:.5f} MHz slot {'ABC'[e['slot']] if e['slot'] < 3 else e['slot']}"
                  f"{' unread' if e['unread'] else ''} id {e['id']}{where} {e['name']!r} {e['text']!r}")
    elif a.cmd == "baud":
        rate = int(a.args[0])
        link.ok(BAUD, struct.pack("<I", rate))
        time.sleep(0.05)
        link.p.baud(rate)
        link.ok(HELLO, bytes([PROTOCOL]) + bytes(8) + b"ferrite_link.py")  # within 1 s, or the radio goes back
        print(f"now at {rate}")
    elif a.cmd == "aprs-cfg":
        rec = link.read(APRS, 0)
        for k, v in aprs_cfg(rec).items():
            print(f"{k}: {v}")
    elif a.cmd == "heard":
        i, n = 0, None
        while n is None or i < n:
            r = link.ok(HEARD, bytes([i]))
            n, rest = r[0], r[1:]
            if not rest:
                break
            while rest:
                st, rest = parse_station(rest)
                pos = "%.5f %.5f" % st["pos"] if "pos" in st else "no position"
                print(f"{st['call']:<10} {st['sym']:<5} {st['ago_s']:>6} s ago  x{st['times']:<3} {pos:<22} via {st['via'] or 'direct'}  {st['comment']}")
                i += 1
        print(f"{n} heard")
    elif a.cmd == "msgs":
        i, n = 0, None
        while n is None or i < n:
            r = link.ok(MSGS, struct.pack("<H", i))
            n, rest = struct.unpack("<H", r[:2])[0], r[2:]
            if not rest:
                break
            while rest:
                m, rest = parse_message(rest)
                print(f"{'from' if m['in'] else 'to':<4} {m['peer']:<9} {m['ago_s']:>6} s ago  [{m['state']}] {m['text']}")
                i += 1
        print(f"{n} messages")
    elif a.cmd == "msg-send":
        to, body = a.args[0].encode("ascii"), a.args[1].encode("ascii")
        r = link.ok(MSG_SEND, bytes([len(to)]) + to + bytes([len(body)]) + body)
        print("queued: " + ("APRS mode is on, it goes out now" if r[0] else "it waits for APRS mode (MENU 0 5 3)"))
    elif a.cmd == "subscribe":
        what = (a.args[0] if a.args else "heard,msgs,ui").split(",")
        return subscribe(link, what, a.seconds if a.seconds != 5.0 else 3600, a.screen)
    elif a.cmd == "reboot":
        link.ok(REBOOT)  # LINK-24: answered, then the radio saves, goes quiet and resets into the bootloader
        print("restarting: the bootloader listens about 200 ms (run flash.py now, or it starts Ferrite again)")
    else:
        sys.exit(f"unknown command {a.cmd}; see --help")
    return 0


if __name__ == "__main__":
    main()
