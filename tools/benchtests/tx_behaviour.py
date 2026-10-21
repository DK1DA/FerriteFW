"""`bench.py --suite tx-behaviour`: how the radio behaves around a key-up, on the tx-dry build with the RTL-SDR.

    tools/bench.py --suite tx-behaviour --dry-plan               # the tests, key-ups, keyed time; touches nothing
    tools/bench.py --suite tx-behaviour                          # all of them (about 10 min)
    tools/bench.py --suite tx-behaviour --tests red_led,menu_does_nothing_keyed
    tools/bench.py --selftest                                    # includes this suite on a fake radio and SDR

Each test reads the radio back over the link while keyed (LINK-19's readback, terminal lines: `gpio`, `bk`,
`mem32`, `si`, `gps query`), presses keys remotely (never a PTT), and watches the air with the SDR. It checks the
transmit screen, the LEDs, the backlight, the cable, the bandwidth registers, txtest's filters, PTT from other
screens and slots, refusals that must never key, and what a key-up leaves alone (the relay, the path pins, the
broadcast's mute, the receive selects).

Safety, as bench.py's own (and through its code: Bench.check_build, Bench.gate, DutyLimiter, Sdr):
  - the build must say tx-dry with the bench bit (LINK-35) or the run stops before anything changes;
  - every key-up passes the DutyLimiter (at most 5 s with its 0.5 s unkey tail, a rest of 2x, at most 25 % of any
    5 min; one exception, by Alex's OK of Oct 15, 2026: cw_id's first key-up, 1 s keyed then the end tone and the
    CW ID (about 4.5 s of carrier for VE7TEST), counted whole as the "timeout" kind; the rest after it still holds);
  - right before every KEY on the keyed slot is read back (SLOT, LINK-33): its transmit frequency (after any shift
    or split) 145.495 or 446.475 MHz (config test_freqs; 145.575, split_freqs, only for split_tx's split TX), power low, not scanning, not the APRS slot; a
    refusal probe (a key-up the radio must refuse: out of band, HF, 600 MHz, the band edge, 1.25 m, emergency) is
    gated on its own frequency read back and KEY off goes at once if the radio keys it (the test fails);
  - KEY on must be answered within 0.6 s on the cable (a tx build holds its answers while keyed, TX-43): else KEY
    off and the run stops;
  - keepalives every 0.5 s on their own thread; KEY off at each key-up's deadline from that thread whatever the
    test is doing, and in `finally` always, and at exit;
  - Extend TX (0 8 9) changes only on the radio (the link can't set it): the 222 MHz and 406.025 MHz refusal probes
    run only when it was turned on by hand before the run (EXTEND_HAND), else they skip or are left out.
Everything a test changes (slots A, B and C, settings, the key lock, `rx both`, txtest) is put back after it, and
the whole radio checked against its snapshot at the end.

cw_id restarts the radio (LINK-24's REBOOT, never keyed) so its first unkey is the first since power-on (TX-40).

Not here, and why (report): TX-31.1/SAFE-17.2 (not yet: Alex, Oct 15, 2026)/TX-44.1 need long high-duty keying;
28.400 MHz isn't an approved frequency; TX-30.2 (a 20 s key-up and the pre-alert isn't in the readback);
TX-34/WATCH-16 (the readback doesn't show the watch's pause); TERM-4.4 (Bluetooth); LINK-20.11/TX-35.x/SAFE-15
(APRS, KISS, VOX: the other suites); TX-12.3 (a tone at the mic).
"""
import atexit
import contextlib
import json
import os
import re
import sys
import threading
import time

import numpy as np

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if HERE not in sys.path:
    sys.path.insert(0, HERE)
import bench  # noqa: E402
import ferrite_link as fl  # noqa: E402
import sdrtones  # noqa: E402
import sdrtx  # noqa: E402
from bench import BenchError, SafetyError, TermError, mhz10  # noqa: E402

SUITE = "tx-behaviour"
VHF, UHF = 145.495, 446.475
SPLIT_TX = 145.575  # VFO-16's same-band split: RX 145.495, TX here (approved Oct 15, 2026)
CALLSIGN, CW_ID = "0 8 7 1", "0 8 7 2"
CW_HZ, CW_DIT_S = 700, 0.060  # VOX-12: 700 Hz at 20 wpm (tonegen::CW_HZ, dit_ms(20))
TONE_LEAD_S = 0.2  # tonegen::TONE_LEAD_MS: the silent lead-in before the end tone or the CW ID
LEDS = "0 7 6"
SCREEN_TIMEOUT, ON_TRANSMIT, WAKE_ON = "0 7 4 1", "0 7 4 3", "0 7 4 4"
BATTERY_SAVE = "0 7 7"
TRIM = "0 9 7 2"
EXTEND_TX = bench.EXTEND_TX
# Extend TX changes only on the radio, on its own screen (`set 0 8 9` is refused over the link): a hand step.
EXTEND_HAND = "Extend TX is off: turn it on on the radio (MENU 0 8 9, its warning, OK) for this test, off after"
PAST_TX = 600.0  # tunable (receive reaches 630 MHz) but past every transmit range, Extend TX's included (TX-5)
END_TONE = bench.END_TONE
WAKE_VALUES = ["Keys", "Keys and RX", "Keys, RX and TX"]
C1DT = 0x40013434  # the backlight timer's compare (DISP-26)
PATH_PINS = ["PB0", "PB1", "PE12", "PE7", "PE13", "PE14"]  # VHF RX/TX, UHF RX/TX, HF RX/TX (board.rs)


def _need_extend(r):
    """The Extend TX probes run only with Extend TX already on (set by hand: EXTEND_HAND)."""
    if r.get(EXTEND_TX) != "On":
        raise Skip(EXTEND_HAND)


class Skip(Exception):
    pass


class Stop(Exception):
    """The run can't go on (the link is gone): every later test would meet the same."""


# ---- Tests ----

TESTS = []


def test(name, rules, keyups, keyed_s, wall_s, sdr=True):
    """A test: its check name (bench_rules.RULE_TAGS has its rules), the rules for the plan, and its key-ups, keyed
    seconds and rough wall time without duty waits (the plan's estimate)."""
    def deco(fn):
        TESTS.append(dict(name=name, fn=fn, rules=rules, keyups=keyups, keyed_s=keyed_s, wall_s=wall_s, sdr=sdr,
                          doc=" ".join((fn.__doc__ or "").strip().split("\n\n")[0].split())))
        return fn
    return deco


# ---- The radio over the link: bench.Radio with a lock (the keepalive thread shares it), keys and the readback ----


class LinkRadio(bench.Radio):
    def __init__(self, spec, clock=None):
        self.lock = threading.RLock()
        super().__init__(spec, clock)

    def _ask(self, ty, payload=b"", timeout=2.0):
        with self.lock:
            return super()._ask(ty, payload, timeout)

    def key_on(self, slot_code):
        with self.lock:
            return super().key_on(slot_code)

    def key_alive(self):
        with self.lock:
            return super().key_alive()

    def poll_unkey(self):
        with self.lock:
            return super().poll_unkey()

    def key_off(self):
        with self.lock:
            return super().key_off()

    def _ok(self, ty, payload=b""):
        end = time.time() + 3
        while True:
            s, rest = self._ask(ty, payload)
            if s == 0:
                return rest
            if s != 7 or time.time() > end:
                raise fl.RadioError(s, rest)
            time.sleep(0.05)

    def ui_text(self, what=0):
        out, rest = [], self._ok(fl.UI, bytes([what]))
        while True:
            out.append(rest[1:].decode("utf-8", "replace"))
            if rest[0] == 0:
                return "".join(out)
            rest = self._ok(fl.MORE)

    def press(self, name, long=False):
        self._ok(fl.KEY, bytes([fl.key_code(name), fl.KEY_LONG if long else fl.KEY_SHORT]))
        if long:
            time.sleep(fl.LONG_HOLD + 0.05)

    def reboot(self):
        """REBOOT (LINK-24): (status, reason)."""
        s, rest = self._ask(fl.REBOOT)
        return s, ("" if s == 0 else fl.why(s, rest))


# ---- Keying ----


class KeyUp:
    def __init__(self, purpose, kind):
        self.purpose, self.kind = purpose, kind
        self.accepted = self.refused = self.unkeyed = self.error = None
        self.t_sent = self.t_on = self.t_off = self.t_unkeyed = self.deadline = None
        self.off_answered = None
        self.duty = None
        self.last_alive = 0.0

    def live(self, now):
        return bool(self.accepted) and self.t_off is None and self.unkeyed is None and now < (self.deadline or 0)

    def ev(self):
        return dict(accepted=self.accepted, refused=self.refused, unkeyed=self.unkeyed, t_on=self.t_on,
                    t_off=self.t_off, t_unkeyed=self.t_unkeyed, off_answered=self.off_answered)


class Rig:
    """One run: bench.Bench (gates, duty, SDR, the build check) and the test helpers. `threaded`: keepalives on a
    thread (a real clock); the selftest's virtual clock pumps them instead."""

    def __init__(self, cfg, a, radio, sdr, clock, threaded=True, log=bench.say):
        self.cfg, self.a, self.radio, self.sdr, self.clock, self.threaded, self.out = cfg, a, radio, sdr, clock, threaded, log
        self.b = bench.Bench(cfg, a, radio=radio, sdr=sdr, clock=clock, log=log)
        self.approved = {mhz10(f) for f in cfg["test_freqs"]}
        self.approved_split = {mhz10(f) for f in cfg.get("split_freqs", [])}
        self.active_key = None
        self._off_lock = threading.Lock()
        self.changed = {}  # setting -> its value before the run
        self.snap = None
        self.extend_on = False
        self.locked_by_us = False
        self.rx_both = False
        self.cur = None  # the running test's result

    # -- clock --
    def now(self):
        return self.clock.time()

    def sleep(self, s):
        end = self.now() + max(0.0, s)
        while True:
            left = end - self.now()
            if left <= 0:
                return
            self.clock.sleep(min(left, 0.1))
            self.pump()

    def until(self, t):
        self.sleep(t - self.now())

    # -- the radio --
    def cmd(self, line):
        self.pump()
        out = self.radio.term(line)
        self.pump()
        return out

    def cmd_err(self, line):
        try:
            return True, self.cmd(line)
        except TermError as e:
            return False, str(e)

    def get(self, path):
        return self.b.setting(path)

    def set(self, path, value):
        """A setting for this run; put back at the end (and after the test)."""
        if path not in self.changed:
            self.changed[path] = self.get(path)
        if path not in self.cur.setdefault("settings", []):
            self.cur["settings"].append(path)
        self.cmd(f"set {path} {value}")

    def ui(self):
        self.pump()
        u = fl.parse_ui(self.radio.ui_text(0))
        self.pump()
        u["id"] = (u.get("screen") or {}).get("id", "")
        return u

    def press(self, name, long=False, settle=0.15):
        self.pump()
        self.radio.press(name, long=long)
        self.sleep(settle)

    def keys(self, seq, settle=0.15):
        for w in seq.split():
            n, _, how = w.partition(":")
            self.press(n, long=how == "long", settle=settle)
        return self.ui()

    def home(self):
        u = self.ui()
        if (u.get("bar") or {}).get("lock"):
            self.press("HASH", long=True)
        for _ in range(2):
            self.press("BACK", long=True)
            u = self.ui()
            if u["id"] == "Home" and not u["qmenu"]:
                return u
        raise Skip(f"can't get home (on {u['id']})")

    def gpio(self):
        out = self.cmd("gpio")
        ports = {}
        for m in re.finditer(r"P([A-E])\s+in ([01 ]{19})\s+out ([01 ]{19})", out):
            ports[m.group(1)] = (int(m.group(2).replace(" ", ""), 2), int(m.group(3).replace(" ", ""), 2))
        if len(ports) != 5:
            raise BenchError(f"gpio: can't read {out!r}")
        return ports

    @staticmethod
    def pin(ports, name, which=1):
        return ports[name[1]][which] >> int(name[2:]) & 1

    def pins(self, names):
        g = self.gpio()
        return {n: self.pin(g, n) for n in names}

    def bk(self, chip, reg):
        out = self.cmd(f"bk {chip} {reg:x}")
        m = re.search(r"REG_[0-9A-Fa-f]+ = 0x([0-9A-Fa-f]{4})", out)
        if not m:
            raise BenchError(f"bk {chip} {reg:x}: {out!r}")
        return int(m.group(1), 16)

    def mem32(self, addr):
        out = self.cmd(f"mem32 {addr:08X} 1")
        for line in out.splitlines():
            p = line.split()
            if len(p) >= 2 and re.fullmatch(r"[0-9A-Fa-f]{8}:?", p[0]) and re.fullmatch(r"[0-9A-Fa-f]{8}", p[1]):
                return int(p[1], 16)
        raise BenchError(f"mem32 {addr:08X}: {out!r}")

    def si_prop(self, prop):
        out = self.cmd(f"si prop {prop:04x}")
        m = re.search(r"->((?: [0-9A-Fa-f]{2})+)", out)
        if not m:
            raise BenchError(f"si prop {prop:04x}: {out!r}")
        b = [int(x, 16) for x in m.group(1).split()]
        return (b[-2] << 8 | b[-1]) if len(b) >= 2 else None

    # -- slots --
    def slot_line(self, L):
        return bench.parse_slot_line(self.cmd(f"slot {L}"))

    def tune(self, L, mhz, mode="fm", tone="off"):
        """Slot L's VFO on mhz, simplex, power low; read back."""
        self.cur.setdefault("slots", set()).add(L)
        self.cmd(f"slot {L} vfo {mhz:.5f} {mode} low tone {tone}")
        st = self.radio.slot("ABC".index(L))
        if st["offset"] and not st["split"]:
            self.cmd(f"slot {L} offset off")
            st = self.radio.slot("ABC".index(L))
        if st["freq10"] != mhz10(mhz) or st["offset"] or st["split"] or st["power"] != 0:
            raise Skip(f"slot {L} didn't take {mhz:.5f} simplex low: {self.cmd(f'slot {L}').strip()}")
        return st

    def activate(self, L):
        self.cur.setdefault("slots", set()).add(L)
        self.cmd(f"slot {L} active")

    def setup(self, mode="fm", tone="off"):
        """The common start: home, A on 145.495 active, B on 446.475."""
        self.home()
        self.tune("A", VHF, mode, tone)
        self.activate("A")
        return self.radio.slot(0)

    # -- keying --
    def gate(self, purpose, n, probe_mhz=None, split_ok=False):
        st = self.radio.slot(n)
        tx = st["offset"] if st["split"] else st["freq10"] + st["offset"]
        g = self.b.gate
        g(f"{purpose}: power low", st["power"] == 0, ["low", "mid", "high"][st["power"] % 3])
        g(f"{purpose}: not scanning", not st["scanning"])
        g(f"{purpose}: not the APRS slot", not st["aprs"])
        g(f"{purpose}: tx-dry", self.b.tx_dry is not None, self.b.tx_dry or "")
        if probe_mhz is not None:
            g(f"{purpose}: the probe's frequency read back", tx == mhz10(probe_mhz),
              f"{tx / 1e5:.5f} MHz, the probe {probe_mhz:.5f}")
        else:
            ok = tx in self.approved or (split_ok and st["split"] and tx in self.approved_split)
            g(f"{purpose}: transmit frequency approved", ok,
              f"{tx / 1e5:.5f} MHz; allowed {', '.join(f'{f:.3f}' for f in self.cfg['test_freqs'])}")
        return st

    def _off(self, k):
        with self._off_lock:
            try:
                ans = self.radio.key_off()
            except Exception as e:  # the radio unkeys 2 s after the last keepalive anyway
                ans = False
                k.error = k.error or f"KEY off: {e}"
            if k.t_off is None:
                k.t_off, k.off_answered = self.now(), ans

    def _alive(self, k, stop):
        while not stop.is_set():
            now = self.now()
            if now >= k.deadline:
                self._off(k)
                return
            try:
                self.radio.key_alive()
                u = self.radio.poll_unkey()
            except Exception as e:
                k.error = f"keepalive: {e}"
                self._off(k)
                return
            if u and k.unkeyed is None:
                k.unkeyed, k.t_unkeyed = u, now
                self.out(f"  the radio unkeyed: {u}")
                return
            stop.wait(min(0.5, max(0.0, k.deadline - now)))

    def pump(self):
        k = self.active_key
        if self.threaded or k is None or not k.accepted or k.t_off is not None or k.deadline is None or k.unkeyed:
            return
        now = self.now()
        if now >= k.deadline:
            self._off(k)
            return
        if now - k.last_alive >= 0.5:
            k.last_alive = now
            self.radio.key_alive()
            u = self.radio.poll_unkey()
            if u and k.unkeyed is None:
                k.unkeyed, k.t_unkeyed = u, now

    @contextlib.contextmanager
    def keyup(self, seconds, purpose, slot="active", probe_mhz=None, kind="burst", extra_tail=0.0, split_ok=False):
        """The one way this suite keys. slot: "active", "a", "b" or "c". probe_mhz: a key-up the radio must refuse,
        gated on that frequency read back; KEY off at once if it keys. kind: the DutyLimiter's (a duty exception,
        "timeout", only for the CW ID's one key-up); extra_tail: the carrier the radio adds after KEY off past the
        0.5 s unkey tail (the end tone and the CW ID), counted by the DutyLimiter."""
        probe = probe_mhz is not None
        kind = "probe" if probe else kind
        k = KeyUp(purpose, kind)
        self.b.duty.admit(self.b.duty.planned(bench.KEY_ANSWER_S if probe else seconds, extra_tail), kind, purpose)
        if slot == "active":
            n = next((i for i in range(3) if self.radio.slot(i)["active"]), None)
            if n is None:
                raise BenchError("no active slot")
        else:
            n = "abc".index(slot)
        self.gate(purpose, n, probe_mhz, split_ok)  # right before KEY on, after any duty wait
        k.t_sent = self.now()
        stop, th = threading.Event(), None
        self.active_key = k
        self.b.keyed = True
        try:
            r = self.radio.key_on(fl.SLOTS[slot])
            k.t_on = self.now()
            if r is None:
                k.accepted = True
                if getattr(self.radio, "cable", True):
                    raise SafetyError("KEY on wasn't answered within 0.6 s: a `tx` build holds its answers while "
                                      "keyed (TX-43), a tx-dry build doesn't. Unkeyed at once; stopping.")
            elif r[0] != 0:
                k.accepted, k.refused = False, r[1]
            else:
                k.accepted = True
                if probe:
                    self.out(f"  {purpose}: the radio KEYED a key-up it must refuse: KEY off at once")
                    self._off(k)
                else:
                    k.deadline = k.t_sent + seconds - 0.05
                    k.last_alive = self.now()
                    if self.threaded:
                        th = threading.Thread(target=self._alive, args=(k, stop), daemon=True)
                        th.start()
            yield k
        finally:
            stop.set()
            if th is not None:
                th.join(timeout=4)
            self._off(k)
            self.active_key = None
            self.b.keyed = False
            k.duty = self.b.duty.record(purpose, kind, k.ev(), k.t_sent, extra_tail=extra_tail if k.accepted else 0.0)
            self.cur.setdefault("keyups", []).append(dict(purpose=purpose, accepted=k.accepted, refused=k.refused,
                                                          unkeyed=k.unkeyed, keyed_s=round(k.duty["keyed_s"], 2)))

    def hold(self, k):
        """Wait for the key-up's own end (its deadline)."""
        if k.deadline:
            self.until(k.deadline + 0.05)

    @contextlib.contextmanager
    def capture(self, mhz, seconds, what, lead=1.0, key_s=None, kind="burst", extra_tail=0.0):
        """The SDR on mhz for `seconds` from now; the body runs `lead` s in; cap.raw after. key_s (kind, extra_tail):
        the key-up the body will make, so the duty wait comes before the SDR starts."""
        if key_s:
            self.b.duty.wait(self.b.duty.planned(key_s, extra_tail), kind, what, lead)
        d = os.path.join(self.sess_dir, "captures")
        os.makedirs(d, exist_ok=True)
        path = os.path.join(d, f"{self.cur['name']}-{len(self.cur.setdefault('captures', []))}-{bench.slug(what)}.u8")
        cap = type("Capture", (), {})()
        cap.mhz, cap.path, cap.raw = mhz, path, None
        self.cur["captures"].append(path)
        t0 = self.now()
        self.sdr.start(mhz, seconds, path)
        try:
            self.sleep(lead)
            yield cap
            self.sleep(t0 + seconds - self.now() + 0.1)
            cap.raw = self.sdr.finish()
        finally:
            self.sdr.stop()

    def restart(self):
        """LINK-24's REBOOT, never keyed (the radio refuses it keyed anyway, LINK-24.3): the radio saves what is
        pending and restarts through the bootloader; HELLO again within 60 s and the build checked again."""
        self.need(self.active_key is None, "a restart while keyed")
        s, why = self.radio.reboot()
        if s != 0:
            raise Skip(f"REBOOT refused: {why}")
        self.sleep(3.0)
        end = self.now() + 60
        while True:
            try:
                hello = self.radio.hello()
                break
            except Exception as e:  # noqa: BLE001  (the bootloader, then the radio starting)
                if self.now() > end:
                    raise Stop(f"no HELLO within 60 s of the restart: {e}") from e
                self.sleep(2.0)
        self.b.check_build(hello, self.radio.info(), None)
        self.sleep(2.0)
        return hello

    # -- checks --
    def check(self, cond, what, got=""):
        self.cur["checks"].append(bench._chk(what, cond, "" if got is None else str(got)[:200]))
        return bool(cond)

    def need(self, cond, why):
        if not cond:
            raise Skip(why)

    # -- the snapshot and the restore --
    def snapshot(self):
        lines = {L: self.slot_line(L) for L in "ABC"}
        active = next(L for L in "ABC" if self.radio.slot("ABC".index(L))["active"])
        vfo = {}
        for L, s in lines.items():
            vfo[L] = bench.parse_slot_line(self.cmd(f"slot {L} vfo")) if s["channel"] is not None else s
        self.snap = dict(lines=lines, vfo=vfo, active=active, end_tone=self.get(END_TONE))
        return self.snap

    def put_slot(self, L, notes):
        v = self.snap["vfo"][L]
        words = [f"slot {L} vfo {v['freq']}", v.get("mode", "").lower(), v.get("power", "")]
        if v.get("txtone"):
            words += ["txtone", v["txtone"], "rxtone", v.get("rxtone", "off")]
        for line in (" ".join(w for w in words if w),
                     f"slot {L} offset {v['offset']}" if v.get("offset") else f"slot {L} offset off"):
            ok, out = self.cmd_err(line)
            if not ok:
                notes.append(f"{line}: {out}")
        want = self.snap["lines"][L]
        if want["channel"] is not None:
            # Back to its channel as the keypad does it: V/M on the active slot (a remote key, LINK-19).
            ok = False
            with contextlib.suppress(Exception):
                self.cmd(f"slot {L} active")
                self.home()
                self.press("VM", settle=0.3)
                ok = self.slot_line(L)["channel"] == want["channel"]
            if not ok:
                notes.append(f"slot {L} was on channel {want['channel']}: its VFO is put back; press V/M on the "
                             "radio to return to the channel")

    def tidy(self, res, notes):
        """After a test: KEY off, txtest off, `rx one`, Extend TX off, the key lock, its settings and slots back."""
        with contextlib.suppress(Exception):
            self.radio.key_off()
        steps = ["txtest off"]
        if self.rx_both:
            steps.append("rx one")
            self.rx_both = False
        if self.extend_on:
            steps.append(f"set {EXTEND_TX} Off")
            self.extend_on = False
        for line in steps:
            ok, out = self.cmd_err(line)
            if not ok and "txtest" not in line:
                notes.append(f"{line}: {out}")
        for path in res.get("settings", []):
            ok, out = self.cmd_err(f"set {path} {self.changed[path]}")
            if not ok:
                notes.append(f"set {path} {self.changed[path]}: {out}")
        for L in sorted(res.get("slots", [])):
            self.put_slot(L, notes)
        if res.get("slots"):
            self.cmd_err(f"slot {self.snap['active']} active")
        with contextlib.suppress(Exception):
            u = self.ui()
            if (u.get("bar") or {}).get("lock"):
                self.press("HASH", long=True)
            if u["id"] != "Home":
                self.press("BACK", long=True)

    def verify(self, notes):
        """The radio against the snapshot: each slot's line and the active one."""
        for L in "ABC":
            want = self.snap["lines"][L]
            try:
                now = self.slot_line(L)
            except Exception as e:  # noqa: BLE001
                notes.append(f"slot {L}: can't read ({e})")
                continue
            a, b = want["line"].replace("*", " ", 1), now["line"].replace("*", " ", 1)
            if a != b:
                notes.append(f"slot {L}: was {want['line']!r}, now {now['line']!r}")
        with contextlib.suppress(Exception):
            act = next(L for L in "ABC" if self.radio.slot("ABC".index(L))["active"])
            if act != self.snap["active"]:
                notes.append(f"active slot: was {self.snap['active']}, now {act}")


# ---- Measurements ----


def carrier_s(raw):
    runs = bench.carrier_runs(raw)
    return max((b - a for a, b in runs), default=0.0), runs


def modulation_tail(raw, before_end_s=(1.1, 0.6)):
    """The modulation (rms deviation, Hz) near the key-up's start and just before KEY off (the carrier's end less
    the 0.5 s unkey hold): (start, end) or None without a carrier."""
    k = bench._keyed_dev(raw)
    if not k:
        return None
    dev, fs = k["dev"], k["fs"]
    n = len(dev)

    def rms(a, b):
        a, b = max(0, int(a * fs)), min(n, int(b * fs))
        x = dev[a:b]
        return float(np.sqrt(np.mean((x - np.median(x)) ** 2))) if b - a > fs * 0.1 else 0.0

    T = n / fs
    return rms(0.3, 0.8), rms(T - before_end_s[0], T - before_end_s[1])


def offset_hz(raw):
    k = bench._keyed_dev(raw)
    return None if not k else k["off"]


def recentre(raw, from_mhz, to_mhz, chunk=1 << 20):
    """A capture made on from_mhz (rtl_sdr's u8 IQ) as if made on to_mhz: everything moved by (from - to) MHz, so
    another frequency inside the same capture (within about +-250 kHz of it) can be measured as the bench measures
    its own (carrier_runs, _keyed_dev)."""
    n = len(raw) // 2
    out = np.empty(2 * n, np.uint8)
    w = 2 * np.pi * (from_mhz - to_mhz) * 1e6 / sdrtx.FS
    for a in range(0, n, chunk):
        b = min(a + chunk, n)
        seg = raw[2 * a: 2 * b].astype(np.float64) - 127.5
        x = (seg[0::2] + 1j * seg[1::2]) * np.exp(1j * w * np.arange(a, b))
        out[2 * a: 2 * b: 2] = np.clip(np.round(x.real + 127.5), 0, 255)
        out[2 * a + 1: 2 * b: 2] = np.clip(np.round(x.imag + 127.5), 0, 255)
    return out


MORSE = dict(zip("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789/", (
    ".- -... -.-. -.. . ..-. --. .... .. .--- -.- .-.. -- -. --- .--. --.- .-. ... - ..- ...- .-- -..- -.-- --.. "
    "----- .---- ..--- ...-- ....- ..... -.... --... ---.. ----. -..-.").split()))


def cw_steps(text, dit=CW_DIT_S):
    """tonegen::Cw in seconds: (key down?, s) with the gaps, a dit inside a character, 3 between, none after the
    last element; characters CW can't send are skipped."""
    out = []
    els = [[3 if e == "-" else 1 for e in MORSE[c]] for c in text.upper() if c in MORSE]
    for i, ch in enumerate(els):
        for j, e in enumerate(ch):
            out.append((True, e * dit))
            if j < len(ch) - 1:
                out.append((False, dit))
            elif i < len(els) - 1:
                out.append((False, 3 * dit))
    return out


def tail_tones(raw, secs):
    """The steady tones in the keyed part's last `secs` s: (start, end, Hz, dev) with start/end in s from the
    capture's start (as carrier_runs), and the carrier's runs."""
    k = bench._keyed_dev(raw)
    if not k:
        return [], []
    dev, fs = k["dev"], k["fs"]
    a = max(0, len(dev) - int(secs * fs))
    seg = dev[a:] - np.median(dev[a:])
    t_seg = (k["i0"] + 1 + a) / fs
    return [(t0 + t_seg, t1 + t_seg, f, d) for t0, t1, f, d in sdrtones.steady_tones(seg, fs)], bench.carrier_runs(raw)


# ---- The tests ----


@test("txb_transmit_screen", {"LINK-20.12": "all", "TX-22.2": "part", "TX-12.2": "part"}, 1, 3.5, 12)
def transmit_screen(r):
    """A remote key-up on a slot with a CTCSS tone: the Transmit screen (Remote, PTT: Stop remote), the slot, the
    timeout counting, no watts; the carrier on the SDR."""
    r.setup(tone="100.0")
    with r.capture(VHF, 6.5, "carrier", key_s=3.5) as cap:
        with r.keyup(3.5, "transmit screen") as k:
            r.check(k.accepted, "the remote key-up keyed", k.refused)
            r.sleep(0.8)
            u1 = r.ui()
            r.sleep(1.2)
            u2 = r.ui()
            r.hold(k)
        r.sleep(0.6)
        u3 = r.ui()
    tx1, tx2 = u1.get("transmit") or {}, u2.get("transmit") or {}
    r.check(u1["id"] == "Transmit", "the Transmit screen while keyed", u1["id"])
    r.check(tx1.get("remote") == 1, "it says Remote (remote=1)", tx1)
    r.check(("PTT", "Stop remote") in u1["hints"], "the hint PTT: Stop remote", u1["hints"])
    r.check(tx1.get("slot") == "A", "the keyed slot named (A)", tx1.get("slot"))
    r.check(isinstance(tx2.get("secs"), int) and isinstance(tx1.get("secs"), int) and tx2["secs"] > tx1["secs"],
            "the transmit time counts", f"{tx1.get('secs')} then {tx2.get('secs')}")
    r.check("battery" in (u1.get("bar") or {}), "the battery in the bar while keyed", u1.get("bar"))
    watts = [x for x in u1["lines"] if re.search(r"\b\d+(\.\d+)?\s?W\b", x)]
    r.check(not watts, "no watts anywhere on the screen's readback", watts)
    r.check(u3["id"] == "Home", "home after the unkey", u3["id"])
    s, runs = carrier_s(cap.raw)
    r.check(2.5 <= s <= 5.0, "the carrier on 145.495 MHz for the key-up", f"{s:.2f} s {runs}")
    return f"Transmit screen remote=1, slot A, {tx1.get('secs')}->{tx2.get('secs')} s; carrier {s:.1f} s"


@test("txb_menu_does_nothing_keyed", {"LINK-37": "all"}, 1, 3.0, 6, sdr=False)
def menu_does_nothing_keyed(r):
    """Keyed over the link, MENU pressed remotely: refused `invalid` ("remote keys wait for the unkey", LINK-37), the
    screen stays the Transmit screen, no quick menu. MENU-30 itself (MENU on the radio does nothing) is a person's
    key: the host tests and a hand check have it."""
    r.setup()
    with r.keyup(3.0, "MENU while keyed") as k:
        r.check(k.accepted, "keyed", k.refused)
        r.sleep(0.6)
        u1 = r.ui()
        try:
            r.press("MENU", settle=0.4)
            why = None
        except fl.RadioError as e:
            why = str(e)
        u2 = r.ui()
        r.hold(k)
    r.check(u1["id"] == "Transmit", "the Transmit screen before MENU", u1["id"])
    r.check(why is not None and "unkey" in why, "MENU from the link refused while keyed (LINK-37)", why or "taken")
    r.check(u2["id"] == "Transmit" and not u2["qmenu"], "MENU while keyed: still the Transmit screen, no quick menu",
            f"{u2['id']} qmenu={u2['qmenu']}")
    return f"MENU while keyed: {why}; {u2['id']}"


@test("txb_red_led", {"TX-22.1": "all", "DISP-17.1": "all"}, 1, 3.0, 8, sdr=False)
def red_led(r):
    """LEDs "RX and TX": the red LED's output (PC13) off before, on while keyed, off after the unkey."""
    r.setup()
    r.set(LEDS, "RX and TX")
    before = r.pins(["PC13"])["PC13"]
    with r.keyup(3.0, "red LED") as k:
        r.sleep(0.8)
        during = [r.pins(["PC13"])["PC13"] for _ in range(2)]
        r.hold(k)
    r.sleep(1.0)
    after = r.pins(["PC13"])["PC13"]
    r.check(k.accepted, "keyed", k.refused)
    r.check(before == 0, "PC13 off before", before)
    r.check(all(x == 1 for x in during), "PC13 on while keyed", during)
    r.check(after == 0, "PC13 off after the unkey", after)
    return f"PC13 {before} -> {during} -> {after}"


@test("txb_red_led_off_setting", {"DISP-17.3": "all"}, 1, 3.0, 8, sdr=False)
def red_led_off_setting(r):
    """LEDs Off (0 7 6): the red LED's output stays off through a key-up."""
    r.setup()
    r.set(LEDS, "Off")
    seen = [r.pins(["PC13"])["PC13"]]
    with r.keyup(3.0, "LEDs off") as k:
        r.sleep(0.6)
        seen += [r.pins(["PC13"])["PC13"] for _ in range(3)]
        r.hold(k)
    r.sleep(0.8)
    seen.append(r.pins(["PC13"])["PC13"])
    r.check(k.accepted, "keyed", k.refused)
    r.check(all(x == 0 for x in seen), "PC13 off before, while keyed and after", seen)
    return f"LEDs Off: PC13 {seen}"


@test("txb_backlight_on_transmit", {"DISP-26": "all"}, 2, 6.0, 16, sdr=False)
def backlight_on_transmit(r):
    """0 7 4 3 On: the backlight's compare (C1DT) lit through a key-up; Off: 0 at key-up, lit again after."""
    r.setup()
    r.set(SCREEN_TIMEOUT, "1 min")
    res = {}
    for v in ("On", "Off"):
        r.set(ON_TRANSMIT, v)
        r.press("BACK")  # a key: the screen lit
        before = r.mem32(C1DT)
        with r.keyup(3.0, f"backlight {v}") as k:
            r.sleep(0.8)
            during = [r.mem32(C1DT) for _ in range(2)]
            r.hold(k)
        r.sleep(1.5)
        after = r.mem32(C1DT)
        res[v] = (before, during, after)
        r.check(k.accepted, f"{v}: keyed", k.refused)
        r.check(before > 0, f"{v}: lit before", hex(before))
        if v == "On":
            r.check(all(x > 0 for x in during), "On: lit while keyed", [hex(x) for x in during])
        else:
            r.check(all(x == 0 for x in during), "Off: dark while keyed (C1DT 0)", [hex(x) for x in during])
            r.check(after > 0, "Off: lit again after the unkey", hex(after))
    return "; ".join(f"{v}: {b:#x} -> {[hex(x) for x in d]} -> {a:#x}" for v, (b, d, a) in res.items())


@test("txb_wake_on_transmit", {"DISP-15.2": "part"}, 3, 6.0, 60, sdr=False)
def wake_on_transmit(r):
    """The screen dark (0 7 4 1 at 10 s), a remote key-up: lit with every 0 7 4 4 value. A remote KEY is a PTT press
    (LINK-20.1), and a key wakes the screen either way, PTT included (DISP-15.2); one from the link acts as well
    (decision 162). DISP-15.4 (a transmission no key started) is the host test's (tests/lights.rs) and needs a
    data or VOX key-up on the radio."""
    r.setup()
    r.set(SCREEN_TIMEOUT, "10 s")
    got = {}
    for v in WAKE_VALUES:
        r.set(WAKE_ON, v)
        r.press("BACK")
        end = r.now() + 16
        dark = 0
        while r.now() < end:
            dark = (r.ui().get("state") or {}).get("dark")
            if dark == 1:
                break
            r.sleep(1.0)
        if not r.check(dark == 1, f"{v}: the screen went dark", dark):
            continue
        with r.keyup(2.0, f"wake {v}") as k:
            r.sleep(0.7)
            lit = (r.ui().get("state") or {}).get("dark") == 0
            r.hold(k)
        got[v] = lit
        r.check(k.accepted, f"{v}: keyed", k.refused)
        r.check(lit, f"{v}: lit by the remote PTT press", f"lit={lit}")
    return "; ".join(f"{v}: {'lit' if x else 'dark'}" for v, x in got.items())


@test("txb_tx_bandwidth", {"TX-45": "all"}, 2, 5.0, 12, sdr=False)
def tx_bandwidth(r):
    """REG_43 of the VHF chip while keyed: 0x3028 wide (FM), 0x4008 narrow (NFM)."""
    got = {}
    for mode, want in (("fm", 0x3028), ("nfm", 0x4008)):
        r.setup(mode=mode)
        with r.keyup(2.5, f"REG_43 {mode}") as k:
            r.sleep(0.7)
            v = r.bk(0, 0x43)
            r.hold(k)
        got[mode] = v
        r.check(k.accepted, f"{mode}: keyed", k.refused)
        r.check(v == want, f"{mode.upper()}: bk 0 43 = 0x{want:04X} while keyed", f"0x{v:04X}")
    return ", ".join(f"{m} 0x{v:04X}" for m, v in got.items())


@test("txb_cable_while_keyed", {"TX-43.2": "all", "LINK-12": "part"}, 1, 3.0, 8, sdr=False)
def cable_while_keyed(r):
    """tx-dry answers over the cable while keyed: `bk 0 7D` within a second; PC10 (UART4 TX) high idle and keyed."""
    r.setup()
    idle = r.pins(["PC10"])["PC10"]
    with r.keyup(3.0, "cable while keyed") as k:
        r.sleep(0.6)
        t0 = r.now()
        ok, out = r.cmd_err("bk 0 7d")
        dt = r.now() - t0
        keyed_pc10 = r.pins(["PC10"])["PC10"]
        r.hold(k)
    r.check(k.accepted, "keyed", k.refused)
    r.check(ok and "REG_7D" in out.upper() and dt < 1.0, "`bk 0 7D` answered while keyed, within 1 s",
            f"{dt:.2f} s {out.strip()[:60]}")
    r.check(idle == 1 and keyed_pc10 == 1, "PC10 high idle and keyed", f"{idle}, {keyed_pc10}")
    return f"bk 0 7D answered in {dt:.2f} s keyed; PC10 {idle}/{keyed_pc10}"


@test("txb_gps_query_keyed", {"TERM-36.8": "all"}, 1, 2.5, 8, sdr=False)
def gps_query_keyed(r):
    """`gps query 0` refused while keyed, answered after the unkey."""
    r.setup()
    with r.keyup(2.5, "gps query keyed") as k:
        r.sleep(0.6)
        ok_keyed, out_keyed = r.cmd_err("gps query 0")
        r.hold(k)
    r.sleep(0.8)
    ok_after, out_after = r.cmd_err("gps query 0")
    r.check(k.accepted, "keyed", k.refused)
    # The refusal may come back as the command's error or as its output line ("gps: bench: refused: transmitting:
    # PB10 is held low (GPS-22)"): either way it names the transmission.
    refused = not ok_keyed or "refused" in out_keyed.lower()
    r.check(refused and ("transmitting" in out_keyed.lower() or "GPS-22" in out_keyed), "refused while keyed (GPS-22)",
            out_keyed.strip()[:80])
    r.check(ok_after and "refused" not in out_after.lower(), "answered after the unkey", out_after.strip()[:80])
    return f"keyed: {out_keyed.strip().splitlines()[0][:50]}; after: {(out_after.strip().splitlines() or [''])[0][:50]}"


@test("txb_ptt_from_screens", {"KEY-7.3": "all", "TX-18": "part"}, 4, 8.0, 50)
def ptt_from_screens(r):
    """A remote key-up from a menu (0 8), a list (zones, 0 2 3), an editor (MENU then OK) and with the keys locked:
    the Transmit screen on slot A, the carrier, home after."""
    r.setup()
    out = []
    for what, seq in (("menu", "MENU 0 8"), ("list", "MENU 0 2 3"), ("editor", "MENU MENU"), ("locked", "HASH:long")):
        r.home()
        u0 = r.keys(seq, settle=0.25)
        if what == "locked":
            r.check((u0.get("bar") or {}).get("lock") == 1, "keys locked (hold #)", u0.get("bar"))
        else:
            r.check(u0["id"] != "Home", f"{what}: opened by keys", u0["id"])
        with r.capture(VHF, 4.5, what, key_s=2.0) as cap:
            with r.keyup(2.0, f"PTT from {what}") as k:
                r.sleep(0.6)
                u1 = r.ui()
                r.hold(k)
            r.sleep(0.6)
            u2 = r.ui()
        s, runs = carrier_s(cap.raw)
        r.check(k.accepted, f"{what}: keyed", k.refused)
        r.check(u1["id"] == "Transmit" and (u1.get("transmit") or {}).get("slot") == "A",
                f"{what}: the Transmit screen on A", f"{u1['id']} {u1.get('transmit')}")
        r.check(u2["id"] == "Home", f"{what}: home after the unkey", u2["id"])
        r.check(1.0 <= s <= 3.5, f"{what}: the carrier on 145.495 MHz", f"{s:.2f} s")
        out.append(f"{u0['id']}->{u1['id']}->{u2['id']} {s:.1f} s")
        if what == "locked":
            r.press("HASH", long=True)
    return "; ".join(out)


@test("txb_ptt_slots", {"KEY-7.5": "all", "LINK-20.1": "all"}, 3, 5.0, 25)
def ptt_slots(r):
    """C active on 446.475: a remote key-up keys C (carrier there); A active, `--slot b` keys B on 446.475;
    `--slot c` with A active is refused, no carrier."""
    r.setup()
    r.tune("B", UHF)
    r.tune("C", UHF)
    r.activate("C")
    res = []
    with r.capture(UHF, 4.5, "C active", key_s=2.0) as cap:
        with r.keyup(2.0, "C active") as k:
            r.sleep(0.6)
            u = r.ui()
            r.hold(k)
    s, _ = carrier_s(cap.raw)
    r.check(k.accepted, "C active: keyed", k.refused)
    r.check((u.get("transmit") or {}).get("slot") == "C", "C active: the Transmit screen on C", u.get("transmit"))
    r.check(1.0 <= s <= 3.5, "C active: the carrier on C's 446.475 MHz", f"{s:.2f} s")
    res.append(f"C: {s:.1f} s")
    r.activate("A")
    r.home()
    with r.capture(UHF, 4.5, "slot b", key_s=2.0) as cap:
        with r.keyup(2.0, "--slot b", slot="b") as k:
            r.sleep(0.6)
            u = r.ui()
            r.hold(k)
    s, _ = carrier_s(cap.raw)
    r.check(k.accepted, "--slot b with A active: keyed", k.refused)
    r.check((u.get("transmit") or {}).get("slot") == "B", "--slot b: the Transmit screen on B", u.get("transmit"))
    r.check(1.0 <= s <= 3.5, "--slot b: the carrier on B's 446.475 MHz", f"{s:.2f} s")
    res.append(f"B: {s:.1f} s")
    with r.capture(UHF, 3.0, "slot c refused") as cap:
        with r.keyup(0.0, "--slot c with A active", slot="c", probe_mhz=UHF) as k:
            pass
    s, runs = carrier_s(cap.raw)
    r.check(k.accepted is False, "--slot c with A active: refused", k.refused or "keyed")
    r.check(not runs, "--slot c: no carrier", runs)
    res.append(f"C with A active: {k.refused}")
    return "; ".join(res)


@test("txb_reboot_refused_keyed", {"LINK-24.3": "all"}, 1, 3.5, 10)
def reboot_refused_keyed(r):
    """REBOOT while keyed: answered busy (Appendix G's REBOOT row: `busy` while transmitting), still keyed (readback
    and an unbroken carrier)."""
    r.setup()
    with r.capture(VHF, 6.0, "reboot keyed", key_s=3.5) as cap:
        with r.keyup(3.5, "REBOOT while keyed") as k:
            r.sleep(1.0)
            s, why = r.radio.reboot()
            if s == 0:
                raise Stop("the radio ACCEPTED REBOOT while keyed: it is restarting; the run stops")
            r.sleep(0.3)
            u = r.ui()
            r.hold(k)
    secs, runs = carrier_s(cap.raw)
    r.check(k.accepted, "keyed", k.refused)
    r.check(s == 7, "REBOOT answered busy", why)
    r.check((u.get("state") or {}).get("keyed") == "A" and u["id"] == "Transmit", "still keyed after it",
            f"{u['id']} {u.get('state')}")
    r.check(len(runs) == 1 and secs >= 3.0, "one unbroken carrier for the key-up", f"{runs}")
    return f"REBOOT: {why}; carrier {secs:.1f} s"


@test("txb_session_end_unkeys", {"LINK-20.5": "part"}, 1, 4.0, 10)
def session_end_unkeys(r):
    """Keyed over the cable, then a new HELLO (the session starts over): unkeyed at once, no 400 ms hold."""
    r.setup()
    with r.capture(VHF, 6.5, "session end", key_s=4.0) as cap:
        with r.keyup(4.0, "new session while keyed") as k:
            r.sleep(1.5)
            t_h = r.now()
            r.radio.hello()
            r.sleep(0.8)
            st = r.radio.slot(0)
            u = r.ui()
            r.hold(k)
    secs, runs = carrier_s(cap.raw)
    keyed_for = t_h - k.t_on if k.t_on else None
    r.check(k.accepted, "keyed", k.refused)
    r.check((u.get("state") or {}).get("keyed") == "-", "unkeyed after the new HELLO", u.get("state"))
    r.check(keyed_for is not None and secs <= keyed_for + 0.3,
            "the carrier ends with the session (no 400 ms hold)", f"carrier {secs:.2f} s, HELLO {keyed_for:.2f} s in")
    return f"HELLO {keyed_for:.2f} s in; carrier {secs:.2f} s; active A {st['active']}"


@test("txb_key_off_end_tone", {"LINK-20.6": "all"}, 1, 1.5, 8)
def key_off_end_tone(r):
    """An end tone set, key then KEY off: KEY off answered; the end tone then about 400 ms of carrier on the SDR."""
    setting, slug_ = bench.ENDTONE_SETTINGS[0]
    r.setup()
    r.set(END_TONE, setting)
    item = dict(steps=dict(bench.END_TONES)[slug_], mode="fm")
    sc = dict(r.cfg["sweeps"], **r.cfg["sweeps"]["endtones"])
    with r.capture(VHF, 1.0 + 1.5 + 2.0, "end tone", key_s=1.5 + sc["extra_tail_s"]) as cap:
        with r.keyup(1.5, "KEY off with an end tone") as k:
            r.hold(k)
    a = bench.analyse_endtone(cap.raw, item, sc)
    r.check(k.accepted, "keyed", k.refused)
    r.check(k.off_answered, "KEY off answered")
    r.check(a.get("ok"), f"the {setting} end tone, then the hold", a.get("why") or a.get("text"))
    return f"{setting}: {a.get('text')}"


@test("txb_txtest_armed_and_used", {"TERM-33.1": "part"}, 1, 2.5, 10)
def txtest_armed_and_used(r):
    """`txtest` off; `txtest 4fsk` armed with its `data` line, nothing keys; `txtest off`; armed again, one remote
    key-up sends it (modulation on the SDR) and `txtest` reads off after."""
    r.setup()
    o1 = r.cmd("txtest")
    o2 = r.cmd("txtest 4fsk")
    st = r.radio.slot(0)
    u = r.ui()
    o3 = r.cmd("txtest off")
    r.cmd("txtest 4fsk")
    with r.capture(VHF, 5.0, "txtest 4fsk", key_s=2.5) as cap:
        with r.keyup(2.5, "txtest 4fsk") as k:
            r.hold(k)
    o4 = r.cmd("txtest")
    m = modulation_tail(cap.raw)
    r.check("off" in o1, "`txtest` reads off", o1.strip()[:60])
    r.check("armed" in o2 and "data" in o2, "`txtest 4fsk`: armed, and its data line", o2.strip()[:80])
    r.check((u.get("state") or {}).get("keyed") == "-", "arming keys nothing", u.get("state"))
    r.check("off" in o3, "`txtest off`: off", o3.strip()[:60])
    r.check(k.accepted, "keyed", k.refused)
    r.check(m is not None and m[0] > 300, "the 4FSK on the SDR", m)
    r.check("off" in o4, "used up: `txtest` reads off after the key-up", o4.strip()[:60])
    return f"armed, sent (rms {m[0] if m else 0:.0f} Hz), then off"


def _filters(r):
    return {reg: r.bk(0, reg) for reg in (0x19, 0x7D, 0x50, 0x2B)}


@test("txb_txtest_filters", {"TERM-33.9": "all", "TERM-33.12": "all"}, 3, 7.5, 24, sdr=False)
def txtest_filters(r):
    """txtest's filters while keyed (bk 0 19/7D/50/2B): data and flat set REG_19 bit 15 and REG_7D bit 6, clear REG_50
    bit 15, REG_2B 2:0 100 (data) / 111 (flat); voice changes none; all four as before after the release."""
    r.setup()
    out = []
    for f in ("data", "flat", "voice"):
        before = _filters(r)
        armed = r.cmd(f"txtest 4fsk {f}")
        r.check("armed" in armed, f"{f}: armed", armed.strip()[:60])
        with r.keyup(2.5, f"txtest filters {f}") as k:
            r.sleep(0.6)
            kv = _filters(r)
            r.hold(k)
        r.sleep(0.6)
        after = _filters(r)
        r.check(k.accepted, f"{f}: keyed", k.refused)
        if f == "voice":
            r.check(kv == before, "voice: none of the four changes while keyed",
                    {f"{a:02X}": f"{before[a]:04X}->{kv[a]:04X}" for a in kv})
        else:
            r.check(kv[0x19] >> 15 & 1 == 1, f"{f}: REG_19 bit 15 set", f"0x{kv[0x19]:04X}")
            r.check(kv[0x7D] >> 6 & 1 == 1, f"{f}: REG_7D bit 6 set", f"0x{kv[0x7D]:04X}")
            r.check(kv[0x50] >> 15 & 1 == 0, f"{f}: REG_50 bit 15 clear", f"0x{kv[0x50]:04X}")
            want = 0b100 if f == "data" else 0b111
            r.check(kv[0x2B] & 7 == want, f"{f}: REG_2B bits 2:0 = {want:03b}", f"{kv[0x2B] & 7:03b}")
        r.check(after == before, f"{f}: the four as before after the release",
                {f"{a:02X}": f"{before[a]:04X}->{after[a]:04X}" for a in after if after[a] != before[a]})
        out.append(f"{f}: 2B={kv[0x2B] & 7:03b}")
    return "; ".join(out)


@test("txb_txtest_until_release", {"TERM-33.7": "part", "LINK-20.13": "all"}, 3, 12.0, 36)
def txtest_until_release(r):
    """Each DAC mode (sweep, 4fsk, afsk) armed, a 4 s remote key-up: the signal on the SDR from the start to the
    release (not a 10 s key-up: the duty limit); `txtest` off after."""
    r.setup()
    out = []
    for mode in ("sweep", "4fsk", "afsk"):
        armed = r.cmd(f"txtest {mode}")
        r.check("armed" in armed, f"{mode}: armed", armed.strip()[:60])
        with r.capture(VHF, 6.5, f"txtest {mode}", key_s=4.0) as cap:
            with r.keyup(4.0, f"txtest {mode} until release") as k:
                r.hold(k)
        m = modulation_tail(cap.raw)
        after = r.cmd("txtest")
        r.check(k.accepted, f"{mode}: keyed", k.refused)
        r.check(m is not None and m[0] > 200 and m[1] > 200, f"{mode}: the signal at the start and up to the release",
                None if m is None else f"{m[0]:.0f} / {m[1]:.0f} Hz rms")
        r.check("off" in after, f"{mode}: off after", after.strip()[:50])
        out.append(f"{mode} {m[0] if m else 0:.0f}/{m[1] if m else 0:.0f} Hz")
    return "; ".join(out)


def _probe(r, f, purpose, offset=None, mode="fm", sdr=True):
    """A key-up the radio must refuse, slot A on f (and a shift): (KeyUp, carrier runs or None, the readback after,
    why it wasn't probed or None)."""
    ok, out = r.cmd_err(f"slot A vfo {f:.5f} {mode} low tone off")
    r.cur.setdefault("slots", set()).add("A")
    if not ok:
        return None, None, None, f"the slot can't tune {f:.5f} MHz: {out}"
    if offset is not None:
        ok, out = r.cmd_err(f"slot A offset {offset:+.3f}")
        if not ok:
            return None, None, None, f"slot A offset {offset:+.3f}: {out}"
    st = r.radio.slot(0)
    tx = (st["freq10"] + st["offset"]) / 1e5
    if not st["active"]:
        r.activate("A")
    capture = sdr and tx >= 24.0  # the R820T tunes from 24 MHz
    ctx = r.capture(tx, 3.0, purpose, lead=1.0) if capture else contextlib.nullcontext()
    with ctx as cap:
        with r.keyup(0.0, purpose, probe_mhz=tx) as k:
            pass
    r.sleep(0.3)
    u = r.ui()
    runs = bench.carrier_runs(cap.raw) if capture else None
    return k, runs, u, None


def _refused(r, label, k, runs, u, word=None):
    r.check(k.accepted is False, f"{label}: refused", k.refused or "KEYED (unkeyed at once)")
    if word:
        r.check(word in (k.refused or "").lower(), f"{label}: the reason names {word}", k.refused)
    if runs is not None:
        r.check(not runs, f"{label}: no carrier on the SDR", runs)
    r.check(u["id"] not in ("Transmit", "Emergency"), f"{label}: no Transmit screen, no prompt", u["id"])


@test("txb_refusals_hf", {"TX-4": "part"}, 3, 0.0, 20)
def refusals_hf(r):
    """Remote key-ups on 21.200 (below 24 MHz), 27.185 (CB) and 50.100 MHz (6 m): refused, no carrier (28 MHz's
    allowed side isn't keyed: not an approved frequency)."""
    r.home()
    out = []
    for f in (21.2, 27.185, 50.1):
        k, runs, u, why = _probe(r, f, f"refusal {f:.3f}")
        if why:
            r.check(False, f"{f:.3f} MHz probed", why)
            continue
        _refused(r, f"{f:.3f} MHz", k, runs, u)
        out.append(f"{f:.3f}: {k.refused}")
    return "; ".join(out)


@test("txb_refusal_640", {"TX-5": "all"}, 1, 0.0, 6)
def refusal_640(r):
    """A remote key-up on 600 MHz: tunable (receive goes to 630 MHz) but past every transmit range, Extend TX's
    400-520 MHz included: refused, no carrier. (640 MHz, its first frequency, can't be tuned at all.)"""
    r.home()
    k, runs, u, why = _probe(r, PAST_TX, f"refusal {PAST_TX:.3f}")
    if why:
        raise Skip(why)
    _refused(r, f"{PAST_TX:.3f} MHz", k, runs, u)
    return f"{PAST_TX:.3f}: {k.refused}"


@test("txb_refusal_band_edge", {"TX-6": "part"}, 2, 0.0, 12)
def refusal_band_edge(r):
    """The 144 MHz edge, checked on the final TX frequency, the carrier only (decision 97, as stock and nicFW; no
    half-bandwidth margin): 143.995 simplex, and 144.600 with a -0.605 MHz shift (TX 143.995): refused, no carrier.
    (144.002 is inside the band and keys, as stock.)"""
    r.home()
    out = []
    for f, off in ((143.995, None), (144.6, -0.605)):
        k, runs, u, why = _probe(r, f, f"edge {f:.3f}{'' if off is None else f' {off:+.3f}'}", offset=off)
        if why:
            r.check(False, f"{f:.3f} probed", why)
            continue
        _refused(r, f"TX 143.995 ({f:.3f}{'' if off is None else f' {off:+.3f}'})", k, runs, u)
        out.append(f"{f:.3f}{'' if off is None else f'{off:+.3f}'}: {k.refused}")
    return "; ".join(out)


@test("txb_refusal_125m_extend", {"SAFE-12.4": "all"}, 1, 0.0, 8)
def refusal_125m_extend(r):
    """Extend TX on, a remote key-up on 222.000 MHz: still refused "1.25 m locked", no carrier. Extend TX changes
    only on the radio (`set 0 8 9 On` is refused over the link): a hand step, so the test skips unless it is on."""
    r.home()
    _need_extend(r)
    k, runs, u, why = _probe(r, 222.0, "222.000 Extend TX on")
    if why:
        raise Skip(why)
    _refused(r, "222.000 MHz with Extend TX on", k, runs, u, word="1.25 m")
    return f"222.000 Extend TX on: {k.refused}"


@test("txb_refusal_emergency_toast", {"LINK-20.9": "all"}, 2, 0.0, 12)
def refusal_emergency_toast(r):
    """Remote key-ups on 243.000 MHz and (Extend TX on) 406.025 MHz: refused, no carrier, a toast and no prompt.
    Extend TX is set by hand on the radio (the link can't): without it, 406.025 is left out and said so."""
    r.home()
    out = []
    for f, ext in ((243.0, False), (406.025, True)):
        if ext and r.get(EXTEND_TX) != "On":
            out.append(f"{f:.3f} left out: {EXTEND_HAND}")
            continue
        k, runs, u, why = _probe(r, f, f"emergency {f:.3f}")
        if why:
            r.check(False, f"{f:.3f} probed", why)
            continue
        _refused(r, f"{f:.3f} MHz", k, runs, u, word="emergency")
        r.check(u.get("toast") is not None, f"{f:.3f} MHz: a toast says why", u.get("toast"))
        out.append(f"{f:.3f}: {k.refused}; toast {u.get('toast')}")
    return "; ".join(out)


@test("txb_relay_still", {"WATCH-12": "part"}, 1, 3.0, 8, sdr=False)
def relay_still(r):
    """`gpio` PC4 (the relay) before, while keyed, in the unkey wait and after: unchanged."""
    r.setup()
    r.tune("B", UHF)
    seen = [r.pins(["PC4"])["PC4"]]
    with r.keyup(3.0, "relay") as k:
        r.sleep(0.5)
        seen += [r.pins(["PC4"])["PC4"] for _ in range(2)]
        r.hold(k)
    seen.append(r.pins(["PC4"])["PC4"])  # the unkey wait (400 ms hold + 100 ms)
    r.sleep(1.0)
    seen.append(r.pins(["PC4"])["PC4"])
    r.check(k.accepted, "keyed", k.refused)
    r.check(len(set(seen)) == 1, "PC4 unchanged through the key-up and the unkey wait", seen)
    return f"PC4 {seen}"


@test("txb_battery_save_pins", {"TX-29": "part"}, 1, 4.0, 10, sdr=False)
def battery_save_pins(r):
    """Battery Save Level 3, keyed: the path pins (VHF/UHF/HF RX and TX selects) the same in every sample while
    keyed, VHF TX (PB1) on."""
    r.setup()
    r.set(BATTERY_SAVE, "Level 3")
    samples = []
    with r.keyup(4.0, "battery save keyed") as k:
        r.sleep(0.5)
        while r.now() < k.deadline - 0.4 and len(samples) < 6:
            samples.append(r.pins(PATH_PINS))
            r.sleep(0.4)
        r.hold(k)
    r.check(k.accepted, "keyed", k.refused)
    r.check(len(samples) >= 3, "sampled while keyed", len(samples))
    r.check(all(s == samples[0] for s in samples), "the path pins unchanged while keyed", samples)
    r.check(all(s["PB1"] == 1 for s in samples), "VHF TX select on throughout", [s["PB1"] for s in samples])
    return f"{len(samples)} samples: {samples[0] if samples else '-'}"


@test("txb_rx_both_kept", {"SAFE-24": "part"}, 1, 2.5, 12, sdr=False)
def rx_both_kept(r):
    """A on 2 m, B on 70 cm, `rx both`: PB0 and PE12 on; key A and let go: both on again; an HF slot active: `rx
    both` refused."""
    r.setup()
    r.tune("B", UHF)
    r.cmd("rx both")
    r.rx_both = True
    p0 = r.pins(["PB0", "PE12"])
    with r.keyup(2.5, "rx both") as k:
        r.hold(k)
    r.sleep(1.0)
    p1 = r.pins(["PB0", "PE12"])
    r.cmd("rx one")
    r.rx_both = False
    r.check(k.accepted, "keyed", k.refused)
    r.check(p0 == {"PB0": 1, "PE12": 1}, "`rx both`: PB0 and PE12 on", p0)
    r.check(p1 == {"PB0": 1, "PE12": 1}, "after the unkey: both on again", p1)
    ok, out = r.cmd_err("slot C vfo 28.40000")
    r.cur.setdefault("slots", set()).add("C")
    r.check(ok, "C on 28.400 MHz (no key-up)", out.strip()[:60])
    r.activate("C")
    ok, out = r.cmd_err("rx both")
    refused = not ok or out.strip().startswith("refused")  # the bench command answers its refusal as text
    if not refused:
        r.cmd_err("rx one")
    r.check(refused and "HF slot" in out, "`rx both` refused with an HF slot active", out.strip()[:80])
    return f"rx both {p0} -> after unkey {p1}; HF: {out.strip()[:40]}"


@test("txb_broadcast_muted_keyed", {"TX-24": "part"}, 1, 3.0, 12, sdr=False)
def broadcast_muted_keyed(r):
    """B plays broadcast on 100.1 MHz, A active on 145.495: keyed, the SI4732's input (PA7/PC5) is switched to the
    radio (1/0) and the SI4732 muted (RX_HARD_MUTE 0x4001 = 3, or volume 0x4000 = 0); both back after."""
    r.setup()
    r.tune("B", 100.1)
    r.sleep(1.5)
    u = r.ui()
    b = next((c for c in u["cards"] if c.get("slot") == "B"), {})
    r.need(b.get("kind") == "broadcast", f"B on 100.1 MHz isn't a broadcast card ({b})")
    # the card shows before the SI4732 has powered up: wait until it answers a property read
    end = r.now() + 6.0
    while True:
        try:
            r.si_prop(0x4001)
            break
        except BenchError:
            r.need(r.now() < end, "the SI4732 didn't power up within 6 s of the broadcast card")
            r.sleep(0.5)

    def muted():
        return (r.si_prop(0x4001) or 0) & 3 == 3 or r.si_prop(0x4000) == 0

    p0, m0 = r.pins(["PA7", "PC5"]), muted()
    with r.keyup(3.0, "broadcast keyed") as k:
        r.sleep(0.7)
        p1, m1 = r.pins(["PA7", "PC5"]), muted()
        r.hold(k)
    r.sleep(1.5)
    p2, m2 = r.pins(["PA7", "PC5"]), muted()
    r.check(k.accepted, "keyed", k.refused)
    r.check(p1 == {"PA7": 1, "PC5": 0}, "keyed: PA7/PC5 switched to the radio (1/0)", p1)
    r.check(m1, "keyed: the SI4732 muted", m1)
    r.check(p2 == p0 and m2 == m0, "after: the broadcast's path and mute as before", f"{p0} {m0} -> {p2} {m2}")
    return f"PA7/PC5 {p0} -> {p1} -> {p2}; muted {m0} -> {m1} -> {m2}"


@test("txb_trim_follows", {"CAL-13": "part"}, 2, 5.0, 16)
def trim_follows(r):
    """Crystal trim (0 9 7 2) 0 then +2 ppm: the carrier moves by about 2 ppm of 145.495 MHz (291 Hz, ±30 %) and
    REG_38/39 (the chip's frequency) change with it."""
    r.setup()
    got = {}
    for v in ("0 ppm", "+2 ppm"):
        try:
            r.set(TRIM, v)
        except TermError as e:
            r.need(False, f"crystal trim isn't settable yet ({str(e)[:70]})")
        with r.capture(VHF, 4.5, f"trim {v}", key_s=2.5) as cap:
            with r.keyup(2.5, f"trim {v}") as k:
                r.sleep(0.6)
                regs = (r.bk(0, 0x38), r.bk(0, 0x39))
                r.hold(k)
        got[v] = (offset_hz(cap.raw), regs)
        r.check(k.accepted, f"trim {v}: keyed", k.refused)
    (o0, g0), (o2, g2) = got["0 ppm"], got["+2 ppm"]
    d = None if o0 is None or o2 is None else o2 - o0
    want = 2e-6 * VHF * 1e6
    r.check(d is not None and abs(abs(d) - want) <= 0.3 * want, f"the carrier moved by about {want:.0f} Hz",
            None if d is None else f"{d:+.0f} Hz")
    r.check(g0 != g2, "REG_38/39 follow the trim", f"{g0[0]:04X}/{g0[1]:04X} -> {g2[0]:04X}/{g2[1]:04X}")
    return f"trim +2 ppm moved the carrier {d if d is None else round(d)} Hz"


@test("txb_split_tx", {"VFO-16": "part"}, 1, 2.5, 10)
def split_tx(r):
    """A same-band split set over the terminal (`slot A split 145.575` on A's 145.495 VFO): read back RX 145.495
    and TX 145.575 (split); one remote key-up, gated on that TX: the carrier on 145.575 on the SDR and none on
    145.495 (the same capture moved 80 kHz); slot A put back (`offset off`) after."""
    r.setup()
    r.cmd(f"slot A split {SPLIT_TX:.5f}")
    st = r.radio.slot(0)
    line = r.cmd("slot A").strip()
    ok = st["split"] and st["freq10"] == mhz10(VHF) and st["offset"] == mhz10(SPLIT_TX)
    r.check(ok, f"read back: RX {VHF:.3f}, TX {SPLIT_TX:.3f} (split)", line)
    r.check(f"tx={SPLIT_TX:.5f}" in line, "the slot line says tx=", line)
    if not ok:
        return f"the split didn't take: {line}; not keyed"
    with r.capture(SPLIT_TX, 1.0 + 2.5 + 1.5, "split TX", key_s=2.5) as cap:
        with r.keyup(2.5, "split TX", split_ok=True) as k:
            r.hold(k)
    tx_s, tx_runs = carrier_s(cap.raw)
    rx_s, rx_runs = carrier_s(recentre(cap.raw, SPLIT_TX, VHF))
    r.check(k.accepted, "the remote key-up keyed", k.refused)
    r.check(1.8 <= tx_s <= 3.6, f"the carrier on the TX frequency {SPLIT_TX:.3f} MHz", f"{tx_s:.2f} s {tx_runs}")
    r.check(not rx_runs, f"nothing on the RX frequency {VHF:.3f} MHz", f"{rx_s:.2f} s {rx_runs}")
    return f"{line}; carrier {tx_s:.1f} s on {SPLIT_TX:.3f}, {rx_s:.1f} s on {VHF:.3f}"


CW_CALL_EST = "VE7TEST"  # the plan's estimate (the black radio's callsign); the test reads the radio's own


def _cw_s(call):
    return sum(s for _, s in cw_steps(call))


@test("txb_cw_id", {"TX-26": "part", "TX-40": "part"}, 2, 1.0 + 0.35 + _cw_s(CW_CALL_EST) + 1.0 + 0.35, 40)
def cw_id(r):
    """CW ID every 10 min (0 8 7 2) with the radio's callsign (0 8 7 1) and the Beep end tone, after a restart
    (REBOOT, not keyed: the ID's timer starts at power-on). The first remote unkey: the end tone, then the
    callsign's 700 Hz elements (20 wpm), then the carrier drops (the 400 ms hold) with nothing after; the one key-up
    past the 5 s burst (the duty exception). A second key-up straight after: the end tone and no ID."""
    call = r.get(CALLSIGN)
    r.need(call and call != "Empty", "no callsign on the radio (0 8 7 1)")
    steps = cw_steps(call)
    els = sum(1 for on, _ in steps if on)
    cw_s = sum(s for _, s in steps)
    beep = dict(bench.END_TONES)["beep"]
    tone_s = TONE_LEAD_S + sum(ms for _, ms in beep) / 1000
    r.restart()
    r.setup()
    r.set(END_TONE, "Beep")
    r.set(CW_ID, "Every 10 min")
    extra = tone_s + cw_s
    with r.capture(VHF, 1.0 + 1.0 + extra + 0.5 + 1.0, "first unkey: end tone, CW ID", key_s=1.0, kind="timeout",
                   extra_tail=extra) as cap:
        with r.keyup(1.0, "CW ID (first unkey after power-on)", kind="timeout", extra_tail=extra) as k:
            r.hold(k)
        r.sleep(extra + 0.3)
    tones, runs = tail_tones(cap.raw, extra + 1.0)
    end = [t for t in tones if abs(t[2] - 1000) < 60]
    cw = [t for t in tones if abs(t[2] - CW_HZ) < 60]
    r.check(k.accepted, "the first key-up keyed", k.refused)
    r.check(bool(end), "the end tone (1000 Hz) at the unkey", [round(t[2]) for t in tones][:8])
    r.check(abs(len(cw) - els) <= 1, f"the CW ID: {CW_HZ} Hz elements, {els} for {call}", len(cw))
    r.check(bool(end and cw) and end[-1][1] <= cw[0][0] + 0.02, "the end tone, then the CW ID",
            f"end tone {end[-1][:2] if end else None}, CW from {cw[0][0] if cw else None}")
    gaps = [round(b[0] - a[0], 3) for a, b in zip(cw, cw[1:])]
    r.check(bool(gaps) and abs(min(gaps) - 2 * CW_DIT_S) <= 0.025, "20 wpm: elements 120 ms apart at the closest",
            min(gaps) if gaps else None)
    hold = runs[-1][1] - cw[-1][1] if (runs and cw) else None
    r.check(hold is not None and 0.3 <= hold <= 0.75, "then the carrier drops (the 400 ms hold, tx-dry 0.5 s)",
            None if hold is None else f"{hold:.2f} s after the last element")
    r.check(len(runs) == 1, "one carrier: the ID inside the key-up, nothing keyed after", runs)
    with r.capture(VHF, 1.0 + 1.0 + tone_s + 0.5 + 1.5, "second unkey: no ID", key_s=1.0, extra_tail=tone_s) as cap2:
        with r.keyup(1.0, "CW ID (second unkey, inside 10 min)", extra_tail=tone_s) as k2:
            r.hold(k2)
    tones2, runs2 = tail_tones(cap2.raw, 2.5)
    cw2 = [t for t in tones2 if abs(t[2] - CW_HZ) < 60]
    s2 = max((b - a for a, b in runs2), default=0.0)
    r.check(k2.accepted, "the second key-up keyed", k2.refused)
    r.check(any(abs(t[2] - 1000) < 60 for t in tones2), "the end tone at the second unkey", len(tones2))
    r.check(not cw2, "no CW ID at the second unkey (inside 10 min)", len(cw2))
    r.check(0.8 <= s2 <= 1.0 + tone_s + 0.9, "the second carrier without the ID's length", f"{s2:.2f} s")
    return (f"{call}: end tone then {len(cw)} CW elements, drop {hold if hold is None else round(hold, 2)} s after; "
            f"second unkey {len(cw2)} CW elements, carrier {s2:.2f} s")


# ---- The run ----


def selected(a):
    want = [x.strip() for x in (getattr(a, "tests", None) or "").split(",") if x.strip()]
    names = {t["name"] for t in TESTS} | {t["fn"].__name__ for t in TESTS}
    bad = [w for w in want if w not in names]
    if bad:
        raise SystemExit(f"--tests: no such test {', '.join(bad)} (in {SUITE}: "
                         f"{', '.join(t['fn'].__name__ for t in TESTS)})")
    return [t for t in TESTS if not want or t["name"] in want or t["fn"].__name__ in want]


def run_tests(cfg, a, radio, sdr, clock, sess_dir, threaded=True, log=bench.say):
    """The selected tests inside one snapshot/restore: (extras, rig)."""
    rig = Rig(cfg, a, radio, sdr, clock, threaded=threaded, log=log)
    rig.sess_dir = sess_dir
    _CURRENT.append(rig)
    hello = radio.hello() if hasattr(radio, "hello") else {}
    info = radio.info()
    rig.b.info = info
    rig.b.check_build(hello, info, None)
    tests = selected(a)
    items, notes = [], []
    stopped = None
    rig.cur = dict(name="setup", checks=[])
    try:
        rig.snapshot()
        rig.set(END_TONE, "Off")
        for t in tests:
            res = dict(name=t["name"], label=t["fn"].__name__, rules=t["rules"], checks=[], doc=t["doc"])
            rig.cur = res
            log(f"\n== {t['fn'].__name__}: {t['doc']}")
            t0 = clock.time()
            try:
                res["summary"] = t["fn"](rig) or ""
                res["result"] = "pass" if res["checks"] and all(c["ok"] for c in res["checks"]) else "fail"
            except Skip as e:
                res.update(result="skip", summary=f"skipped: {e}", why=str(e))
            except Stop as e:
                res.update(result="fail", summary=str(e), why=str(e))
                stopped = str(e)
            except SafetyError:
                raise
            except (BenchError, TermError, fl.RadioError, TimeoutError, OSError) as e:
                res["checks"].append(bench._chk("ran to the end", False, f"{type(e).__name__}: {e}"))
                res.update(result="fail", summary=f"stopped: {e}")
            finally:
                tn = []
                if not stopped:
                    rig.tidy(res, tn)
                res["restore_notes"] = tn
                notes += [f"{t['fn'].__name__}: {x}" for x in tn]
            bad = [c for c in res["checks"] if not c["ok"]]
            res["why"] = res.get("why") or "; ".join(f"{c['what']}: got {c['got']}" for c in bad[:3])
            res["ok"] = res["result"] == "pass"
            res["text"] = res.get("summary", "")
            res["wall_s"] = round(clock.time() - t0, 1)
            res["slots"] = sorted(res.get("slots", []))
            log(f"  {res['result'].upper()}: {res['text']}" + (f"  ({res['why']})" if res["why"] and not res["ok"] else ""))
            items.append(res)
            if stopped:
                break
    finally:
        rig.b.emergency_unkey()
        with contextlib.suppress(Exception):
            rig.radio.key_off()
        if rig.snap and not stopped:
            rig.cur = dict(name="restore", checks=[], settings=list(rig.changed), slots={"A", "B", "C"})
            rig.tidy(rig.cur, notes)
            rig.verify(notes)
        _CURRENT.remove(rig)
    ran = [x for x in items if x["result"] != "skip"]
    ex = {f"suite:{SUITE}": dict(name=SUITE, total=len(tests), items=items, complete=len(items) == len(tests),
                                 ok=bool(ran) and all(x["ok"] for x in ran), restore_notes=notes, stopped=stopped)}
    return ex, rig


_CURRENT = []


def _last_ditch():
    for rg in list(_CURRENT):
        with contextlib.suppress(Exception):
            rg.radio.key_off()


atexit.register(_last_ditch)


def run(cfg, a, radio=None, sdr=None, clock=None, threaded=True):
    """`bench.py --suite tx-behaviour`: a session under bench-results with session.json, results.json, summary.html."""
    clock = clock or bench.Clock()
    radio = radio or LinkRadio(cfg["port"])
    sdr = sdr or bench.Sdr(cfg, clock)
    info = radio.info()
    label = a.build or f"{info['version']}-host{bench.host_build()}"
    build = dict(label=label, version=info["version"], host=bench.host_build(), image=None)
    session, sess_dir = bench.new_session(cfg, f"{SUITE}-{label}")
    bench.say(f"session {sess_dir}")
    meta = dict(build=build, source="on air", port=getattr(radio, "spec", "fake"), slot="A", commit=bench.host_build(),
                kinds=["rf"], cfg=dict(sweeps=cfg["sweeps"]), suite=SUITE,
                sim=not isinstance(radio, bench.Radio) or not radio.cable)
    rig = None
    try:
        extras, rig = run_tests(cfg, a, radio, sdr, clock, sess_dir, threaded=threaded)
    except SafetyError as e:
        meta.update(refused=str(e), warnings=[str(e)])
        json.dump(bench.jsonable(meta), open(os.path.join(sess_dir, "session.json"), "w"), indent=1)
        bench.say(f"\n{e}")
        raise
    meta.update(gates=rig.b.gatelog, warnings=rig.b.warnings, restore_notes=extras[f"suite:{SUITE}"]["restore_notes"],
                tx_dry=rig.b.tx_dry, duty=rig.b.duty.summary())
    bench.say(rig.b.duty.report())
    page = bench.write_session(cfg, sess_dir, session, [], extras, meta)
    s = extras[f"suite:{SUITE}"]
    bench.say("\nsummary:")
    for x in s["items"]:
        bench.say(f"  {x['result']:4s}  {x['label']:28s} {x['text'][:90]}")
    if s["restore_notes"]:
        bench.say("restore: " + "; ".join(s["restore_notes"]))
    bench.say(f"\npage: {page}")
    return extras, page, rig


def tests_of(ex, add):
    """bench.rule_tests' hook: each test of a session's suite run, under its own name (bench_rules tags it)."""
    s = ex.get(f"suite:{SUITE}")
    for x in (s or {}).get("items", []):
        if x.get("result") in ("pass", "fail") and x.get("checks"):
            add(x["name"], x["checks"], x.get("summary") or x.get("text") or "")


def estimate(cfg, tests):
    """(wall s, keyed s, counted s) for the tests at the duty limits: each key-up rests 2x its counted time; the
    window caps 25 % of 5 min."""
    d = dict(bench.DEFAULTS["duty"], **cfg.get("duty", {}))
    keyed = sum(t["keyed_s"] for t in tests)
    n = sum(t["keyups"] for t in tests if t["keyed_s"] > 0)
    counted = keyed + n * d["unkey_tail_s"]
    rest = counted * d["rest_factor"]
    wall_tests = sum(t["wall_s"] for t in tests)
    window = counted / d["window_frac"]  # the 25 % cap: the least wall time that much keying can take
    return max(wall_tests + rest * 0.6, window), keyed, counted


def plan(cfg, a):
    tests = selected(a)
    bench.say(f"dry plan for --suite {SUITE} (nothing is touched):")
    bench.say(f"  port {cfg['port']}; keys only on {', '.join(f'{f:.3f}' for f in cfg['test_freqs'])} MHz (refusal "
              "probes on their own frequency, KEY off at once if keyed); HELLO's build byte must say tx-dry + bench")
    bench.say("  save: slots A, B, C (their VFOs), the active slot, the end tone (set Off for the run)")
    if any(t["fn"] is cw_id for t in tests):
        bench.say("  cw_id restarts the radio first (REBOOT, never keyed; settings are saved before the restart, "
                  "LINK-24.1), then sets 0 8 7 2 and the end tone and puts both back")
    for i, t in enumerate(tests, 1):
        rules = ", ".join(f"{k}{'' if v == 'all' else ' (part)'}" for k, v in t["rules"].items())
        bench.say(f" {i:2d}. {t['fn'].__name__} [{rules}]: {t['doc']} — {t['keyups']} key-up"
                  f"{'s' if t['keyups'] != 1 else ''}, {t['keyed_s']:g} s keyed{'' if t['sdr'] else ', no SDR'}")
    wall, keyed, counted = estimate(cfg, tests)
    d = cfg["duty"]
    bench.say(f"  duty: every key-up through the DutyLimiter: at most {d['max_burst_s']:g} s with the "
              f"{d['unkey_tail_s']:g} s tail (the longest 4 s, but cw_id's first: its ID past the burst, the "
              f"timeout exception), rest {d['rest_factor']:g}x, at most "
              f"{d['window_frac']:.0%} of {d['window_s'] / 60:g} min")
    bench.say("  restore after each test and at the end: KEY off, txtest off, rx one, Extend TX off, the key lock, "
              "the settings and slots it changed; the slots checked against the snapshot")
    bench.say(f"  about {wall / 60:.1f} min at the duty limits; {sum(t['keyups'] for t in tests)} key-ups (probes included), "
              f"{keyed:.0f} s keyed ({counted:.0f} s with tails); rtl_sdr "
              f"{'found' if bench.shutil.which('rtl_sdr') else 'NOT found'}")


# ---- The selftest: a fake radio that answers what these tests read ----


class FakeRadio(bench.FakeRadio):
    """bench's FakeRadio plus: the readback and remote keys (a few screens), `gpio`, `bk`, `mem32`, `si`, `gps
    query`, `rx both`, txtest's signals and filters, REBOOT and HELLO while keyed, keying a named slot, the crystal
    trim, a split (`slot A split F`; split_to: a radio that stores another TX than asked), the CW ID (0 8 7 2's
    every 10 min from power-on, VOX-12) after the end tone. `bugs`: names of behaviours to break (the selftest's
    negative checks)."""

    def __init__(self, clock, bugs=(), split_to=None, **kw):
        kw.setdefault("version", "v0.1.0 TX DRY")
        kw.setdefault("build", "tx-dry")
        super().__init__(clock, **kw)
        self.bugs = set(bugs)
        self.split_to = split_to
        self.last_id = None  # the last CW ID (the clock's s); None since power-on
        self.slots[2] = dict(ch=None, freq=446.0, mode="FM", power="low", tx="off", rx="off", offset=0)
        self.set.update({LEDS: "RX, TX and heartbeat", SCREEN_TIMEOUT: "1 min", ON_TRANSMIT: "On",
                         WAKE_ON: "Keys and RX", BATTERY_SAVE: "Level 1", TRIM: "0 ppm", CALLSIGN: "VE7TEST",
                         CW_ID: "Off"})
        self.set0 = dict(self.set)
        self.screen, self.locked, self.toast, self.last_key = "Home", False, None, clock.time()
        self.rxboth = False
        self.filters = None
        self.cable = True
        self.ch_mem = {}

    # -- the slot command's shift, then bench's fake --
    def term(self, line):
        self.calls.append(line)
        w = line.split()
        reads = {"gpio", "bk", "mem32", "si", "gps", "rx"}
        if w[0] in reads or (w[0] == "txtest" and len(w) == 1) or w[0] == "get":
            return self._read(line, w)
        if self.key:
            raise TermError(f"{line}: transmitting: nothing changes while keyed (TX-22)")
        if w[0] == "slot" and len(w) >= 3 and w[2] == "offset":
            s = self.slots["ABC".index(w[1].upper())]
            s["offset"] = 0 if w[3] == "off" else mhz10(float(w[3]))
            s.pop("split", None)
            return self._line("ABC".index(w[1].upper()))
        if w[0] == "slot" and len(w) == 4 and w[2] == "split":
            s = self.slots["ABC".index(w[1].upper())]
            s["split"], s["offset"] = True, mhz10(self.split_to or float(w[3]))
            return self._line("ABC".index(w[1].upper()))
        if w[0] == "slot" and len(w) >= 3 and re.fullmatch(r"\d+(\.\d+)?", w[2] if w[2] != "vfo" else (w[3] if len(w) > 3 else "")):
            f = float(w[3] if w[2] == "vfo" else w[2])
            if f > 700:
                raise TermError(f"{line}: out of the radio's range")
        if w[0] == "slot" and len(w) >= 3 and "vfo" in w[2:]:
            n = "ABC".index(w[1].upper())
            if self.slots[n]["ch"] is not None:
                self.ch_mem[n] = dict(self.slots[n])
        if w[0] == "txtest" and len(w) > 1 and w[1] in ("4fsk", "sweep", "afsk"):
            filt = next((x for x in w[2:] if x in ("voice", "flat", "data")), "data")
            lvl = next((int(x) for x in w[2:] if x.isdigit()), 100)
            self.txtest, self.filters = ("afsk", lvl), filt
            return (f"  txtest: {w[1]} at {lvl} % armed for the next PTT key-up, from the key-up to the release; once\n"
                    f"  {filt}: the chip's filters ...\n")
        if w[0] == "set" and line.startswith(f"set {EXTEND_TX} "):
            raise TermError(f"{line}: Extend TX changes only on the radio")
        if w[0] == "set":
            p = next((k for k in self.set if line.startswith(f"set {k} ")), None)
            if p is None:
                raise TermError(f"{line}: no such setting")
        out = super().term(line)
        self.calls.pop()  # super logged it again
        if w[0] == "txtest" and len(w) > 1 and w[1] == "off":
            self.filters = None
        return out

    def _read(self, line, w):
        if w[0] == "get":
            p = " ".join(w[1:])
            if p not in self.set:
                raise TermError(f"{line}: no such setting")
            return f"{p} x: {self.set[p]}\n"
        if w[0] == "txtest":
            return ("  txtest: 4fsk at 100 % armed for the next PTT key-up\n" if self.txtest
                    else "  txtest: off (nothing armed; PTT transmits the mic as normal)\n")
        if w[0] == "gps":
            if self.key and "gps" not in self.bugs:
                raise TermError(f"{line}: refused: transmitting")
            return "  sent $PCAS06,0*1B\n  answer: $GPTXT,...\n"
        if w[0] == "rx":
            if w[1] == "both":
                if self.slots[self.active]["freq"] < 64:
                    raise TermError(f"{line}: refused: an HF slot is active")
                self.rxboth = True
                return "  rx both: VHF RX (PB0) and UHF RX (PE12) held on, until rx one or a restart\n"
            self.rxboth = False
            return "  rx one: only the receiving band's select on (PB0 or PE12), as normal\n"
        if w[0] == "mem32":
            v = 0 if (self.key and self.set[ON_TRANSMIT] == "Off") else 0x1F4
            return f"  {w[1]}: {v:08X}\n"
        if w[0] == "si":
            p = int(w[2], 16)
            v = (3 if self.key else 0) if p == 0x4001 else 63
            return f"  si 13 00 {p >> 8:02X} {p & 0xFF:02X} -> 80 00 {v >> 8:02X} {v & 0xFF:02X}\n"
        if w[0] == "bk":
            reg = int(w[2], 16)
            return f"  bk{w[1]} REG_{reg:02X} = 0x{self._bk(reg):04X}   x\n"
        if w[0] == "gpio":
            self._check()
            k = self.key
            vhf = k and k["mhz"] < 200
            out = {"A": 0, "B": 0, "C": 1 << 10, "D": 0, "E": 0}
            leds = self.set[LEDS] != "Off" and "led" not in self.bugs
            if k and leds:
                out["C"] |= 1 << 13
            if vhf:
                out["B"] |= 1 << 1
            elif k:
                out["E"] |= 1 << 7
            if self.rxboth or not k:
                out["B"] |= 1
                if self.rxboth:
                    out["E"] |= 1 << 12
            if k or not any(s["freq"] < 108.1 and s["freq"] >= 87.5 for s in self.slots):
                out["A"] |= 1 << 7
            if "relay" in self.bugs and k:
                out["C"] |= 1 << 4

            def b16(v):
                s = f"{v:016b}"
                return " ".join(s[i:i + 4] for i in range(0, 16, 4))
            return "".join(f"  P{p}  in {b16(0)}   out {b16(out[p])}\n" for p in "ABCDE")
        raise TermError(f"{line}: ?")

    def _line(self, n):
        s = self.slots[n]
        if not s.get("split"):
            return super()._line(n)
        off, s["offset"] = s["offset"], 0  # the line shows a split's TX as tx=, not an offset
        try:
            return super()._line(n).rstrip("\n") + f" tx={off / 1e5:.5f}\n"
        finally:
            s["offset"] = off

    def slot(self, n):
        st = super().slot(n)
        st["split"] = bool(self.slots[n].get("split"))
        return st

    def _bk(self, reg):
        k = self.key
        if reg == 0x43:
            if not k:
                return 0x0000
            mode = self.slots[self.active]["mode"]
            return 0x3028 if mode == "FM" else 0x4008
        if reg in (0x38, 0x39):
            f = mhz10(self.slots[self.active]["freq"]) + int(self._trim_ppm() * self.slots[self.active]["freq"] * 10)
            return (f & 0xFFFF) if reg == 0x38 else (f >> 16)
        base = {0x19: 0x1040, 0x7D: 0xE900, 0x50: 0xBB20, 0x2B: 0x0000}
        v = base.get(reg, 0x1234)
        f = k and k["sig"].get("filters")
        if f in ("data", "flat"):
            v = {0x19: v | 0x8000, 0x7D: v | 0x40, 0x50: v & 0x7FFF, 0x2B: (v & ~7) | (4 if f == "data" else 7)}[reg]
        return v

    def _trim_ppm(self):
        return float(self.set[TRIM].split()[0])

    def _dark(self):
        t = int(re.match(r"\d+", self.set[SCREEN_TIMEOUT]).group()) * (60 if "min" in self.set[SCREEN_TIMEOUT] else 1)
        lit_by_tx = self.key and "wake" not in self.bugs  # a remote KEY is a PTT press (DISP-15.2)
        return 0 if (lit_by_tx or self.clock.time() - self.last_key < t) else 1

    def ui_text(self, what=0):
        self._check()
        if not self.key and self.screen == "Transmit":
            self.screen = "Home"
        k = self.key
        L = "ABC"[k["slot"]] if k else "-"
        lines = [f'screen {self.screen} "{self.screen}" title="x"',
                 f"state frame=1 ago=0 dark={self._dark()} locked={int(self.locked)} active={'ABC'[self.active]} "
                 f"keyed={L} remote={int(bool(k))} mode=off fm=0",
                 f"bar lock={int(self.locked)} scan=0 watch=1 inbox=0 alert=0 gps=off bt=0 battery=80 low=0 cutoff=0 "
                 "clock=- utc=0"]
        if self.toast:
            lines.append(f'toast "{self.toast[0]}" "{self.toast[1]}"')
        if self.screen == "Home":
            for n, s in enumerate(self.slots):
                kind = "broadcast" if 87.5 <= s["freq"] <= 108.0 else "slot"
                lines.append(f'card {"ABC"[n]} kind={kind} state=- active={int(n == self.active)}')
        if k:
            lines.append(f"transmit slot={L} remote=1 dial=0 secs={int(self.clock.time() - k['on'])}")
            lines.append('hints "PTT" "Stop remote"')
        else:
            lines.append('hints "MENU" "Menu"')
        return "\n".join(lines) + "\n"

    def press(self, name, long=False):
        self.calls.append(f"press {name}{':long' if long else ''}")
        self.last_key = self.clock.time()
        if self.key:
            if name == "MENU" and "menu" in self.bugs:
                self.screen = "Quick"  # taken while keyed
                return
            raise fl.RadioError(9, b"transmitting: remote keys wait for the unkey")  # LINK-37
        n = name.upper()
        if n == "VM" and not self.locked and self.screen == "Home" and self.active in self.ch_mem:
            vfo = dict(self.slots[self.active])
            self.slots[self.active] = self.ch_mem.pop(self.active)
            self.vfo_b = vfo
            return
        if n == "HASH" and long:
            self.locked = not self.locked
        elif n == "BACK" and long:
            self.screen = "Home"
        elif self.locked:
            return
        elif n == "BACK":
            self.screen = {"Value": "Quick", "Quick": "Home", "Advanced": "Home", "Group": "Advanced",
                           "Zones": "Group"}.get(self.screen, "Home")
        elif n == "MENU":
            self.screen = {"Home": "Quick", "Quick": "Value"}.get(self.screen, self.screen)
        elif n.isdigit():
            self.screen = {"Quick": "Advanced" if n == "0" else "Value", "Advanced": "Group",
                           "Group": "Zones"}.get(self.screen, self.screen)
        self.toast = None

    def reboot(self):
        self.calls.append("REBOOT")
        if self.key and "reboot" not in self.bugs:
            return 7, "busy"
        if not self.key:
            self.last_id = None  # power-on: the ID's timer starts again
            self.screen, self.rxboth = "Home", False
        return 0, ""

    def hello(self):
        self.calls.append("HELLO")
        if self.key:  # LINK-20.5: a new session ends the remote key-up at once, no hold
            k, t = self.key, self.clock.time()
            hold = 0.5 if "session" in self.bugs else 0.0
            self.carriers.append((k["on"], t + hold, k["mhz"], k["kind"], dict(k["sig"], t_off=t)))
            self.key = None
            self.unkey_msgs.append("invalid: the session ended")
        return super().hello()

    def key_on(self, slot_code):
        self.calls.append("KEY on")
        n = self.active if slot_code == fl.ACTIVE else slot_code
        s = self.slots[n]
        if n == 2 and self.active != 2:
            return 9, "invalid: C keys only as the active slot"
        f = s["freq"] + s["offset"] / 1e5
        if s.get("split"):
            f = s["freq"] if "split-rx" in self.bugs else s["offset"] / 1e5
        why = None
        if any(abs(f - x) < 1e-6 for x in self.leaky):
            why = None
        elif f < 64:
            why = "invalid: HF transmit: only 24.89-24.99 and 28.0-29.7 MHz"
        elif f >= PAST_TX:
            why = "invalid: the transmitter can't tune it"
        elif 143.9 <= f < 144.0 or 148.0 < f <= 148.1:
            why = "invalid: outside the band plan (the edge: the carrier only, decision 97)"
        elif 200 <= f < 300 and not (abs(f - 243.0) <= 0.025):
            why = "invalid: 1.25 m locked"
        else:
            why = self._refuse(f)
        if why:
            if "emergency" in why:
                self.toast = ("Emergency frequency", "only PTT on the radio, twice")
            else:
                self.toast = ("Transmit refused", why)
            return 9, why
        t = self.clock.time()
        kind = "afsk" if self.txtest else "carrier"
        sig = dict(kind=kind, tone=s["tx"], mode=s["mode"], level=self.set[bench.SUBTONE_LEVEL],
                   endtone=self.set[END_TONE], afsk=self.txtest[1] if self.txtest else None, filters=self.filters,
                   trim_hz=self._trim_ppm() * f)
        self.key = dict(on=t, last=t, mhz=f, kind=kind, sig=sig, slot=n)
        if self.screen != "Transmit":
            self.screen = "Transmit"
        self.txtest = None
        self.filters = None if "filters-stick" not in self.bugs else self.filters
        return (0, "")

    def key_off(self):
        was = self.key
        if was:
            self._check()
            was = self.key
        t = self.clock.time()
        mode = self.set[CW_ID]
        due = bool(was) and was["kind"] == "carrier" and mode != "Off" and self.set[CALLSIGN] != "Empty" and (
            mode == "On unkey" or "cw-every" in self.bugs or self.last_id is None or t - self.last_id >= 600)
        r = super().key_off()
        if was:
            self.screen = "Home"
        if due:
            # VOX-12/TX-26: after the end tone (or the silent lead-in alone), the callsign at 700 Hz, then the hold.
            on, end, mhz, kind, sig = self.carriers[-1]
            endtone = sig["endtone"]
            seq = TONE_LEAD_S + (sum(ms for _, ms in dict(bench.END_TONES)[dict(bench.ENDTONE_SETTINGS)[endtone]])
                                 / 1000 if endtone != "Off" else 0.0)
            steps = cw_steps(self.set[CALLSIGN])
            cw_s = sum(x for _, x in steps)
            self.carriers[-1] = (on, t + seq + cw_s + 0.5, mhz, kind, dict(sig, cw_t0=t - on + seq, cw=steps))
            self.last_id = t
        return r


class FakeSdr(bench.FakeSdr):
    """bench's FakeSdr; the carrier's offset moved by the fake radio's crystal trim (sig trim_hz); the CW ID's 700 Hz
    elements (sig cw_t0, cw); a carrier up to 300 kHz off the capture's frequency in it at its offset (a split's
    other side)."""

    def _one(self):
        orig = bench.fake_dev

        def dev(sig, tt, rel_off, rng):
            out = orig(sig, tt, rel_off, rng) + sig.get("trim_hz", 0.0)
            if sig.get("cw"):
                t = sig["cw_t0"]
                for on, d in sig["cw"]:
                    if on:
                        m = (tt >= t) & (tt < t + d)
                        out[m] += 1200.0 * np.sin(2 * np.pi * CW_HZ * tt[m])
                    t += d
            return out
        bench.fake_dev = dev
        try:
            return super().finish()
        finally:
            bench.fake_dev = orig

    def finish(self):
        raw = self._one()
        t0, t1, mhz = self.t0, self.t0 + self.seconds, self.mhz
        near = sorted({round(c[2], 5) for c in self.r.carriers
                       if c[1] > t0 and c[0] < t1 and 0.01 <= abs(c[2] - mhz) <= 0.3})
        if not near:
            return raw
        saved = self.r.carriers
        x = raw.astype(np.float64) - 127.5
        try:
            for m in near:
                self.r.carriers, self.mhz = [c for c in saved if abs(c[2] - m) < 0.01], m
                x += recentre(self._one(), m, mhz).astype(np.float64) - 127.5
        finally:
            self.r.carriers, self.mhz = saved, mhz
        raw = np.clip(np.round(x + 127.5), 0, 255).astype(np.uint8)
        raw.tofile(self.path)
        return raw


def selftest(cfg, args, check, tmp):
    """Every test on the fake radio and SDR (virtual clock): all pass, the radio put back, every key-up within the
    duty limits and on an approved frequency or refused; then broken radios: the matching test fails."""
    import copy
    c2 = copy.deepcopy(cfg)
    c2["results"] = os.path.join(tmp, "results-txb")
    clk = bench.FakeClock()
    r = FakeRadio(clk)
    r.set[EXTEND_TX] = "On"  # the hand step (EXTEND_HAND) done before the run
    before = json.dumps(r.slots), dict(r.set), r.active
    a = args(build="selftest-txb", tests=None)
    ex, page, rig = run(c2, a, radio=r, sdr=FakeSdr(r, clk), clock=clk, threaded=False)
    s = ex[f"suite:{SUITE}"]
    bad = [(x["label"], x["result"], x.get("why")) for x in s["items"] if x["result"] != "pass"]
    check(f"suite {SUITE}: every test passes on the fake radio", not bad and len(s["items"]) == len(TESTS),
          bad[:4] or f"{len(s['items'])} tests")
    after = json.dumps(r.slots), dict(r.set), r.active
    check(f"suite {SUITE}: the radio put back (slots, settings, active slot)", after == before and not r.rxboth
          and not r.locked and r.txtest is None, "" if after == before else f"{before} / {after}")
    check(f"suite {SUITE}: every KEY on has its KEY off, nothing keyed at the end",
          r.calls.count("KEY on") <= r.calls.count("KEY off") and r.key is None,
          f"{r.calls.count('KEY on')} on, {r.calls.count('KEY off')} off")
    ok, worst, rests, bursts = bench.duty_ok(rig.b.duty.log, c2["duty"])
    check(f"suite {SUITE}: duty limits held (burst, rest, 25 % window)", ok, f"worst window {worst:.1f} s")
    keyed_f = {round(c[2], 3) for c in r.carriers}
    check(f"suite {SUITE}: carriers only on the approved frequencies", keyed_f <= {VHF, UHF, SPLIT_TX}, keyed_f)
    over = [e for e in rig.b.duty.log if e["counted_s"] > c2["duty"]["max_burst_s"] + 1e-6]
    check(f"suite {SUITE}: one key-up past the burst limit, the CW ID's, as the timeout exception",
          len(over) == 1 and over[0]["kind"] == "timeout" and "CW ID" in over[0]["purpose"],
          [(e["purpose"], e["kind"], round(e["counted_s"], 1)) for e in over])
    res = json.load(open(os.path.join(os.path.dirname(page), "bench", "results.json")))
    want = set().union(*(t["rules"] for t in TESTS))
    check(f"suite {SUITE}: results.json carries each test's rules", want <= set(res["rules"]),
          sorted(want - set(res["rules"]))[:5])
    # Broken radios: the test that measures it fails (and nothing else breaks the run).
    for bug, label in (("led", "red_led"), ("menu", "menu_does_nothing_keyed"), ("gps", "gps_query_keyed"),
                       ("reboot", "reboot_refused_keyed"), ("session", "session_end_unkeys"),
                       ("relay", "relay_still"), ("wake", "wake_on_transmit"), ("split-rx", "split_tx"),
                       ("cw-every", "cw_id")):
        clk = bench.FakeClock()
        rb = FakeRadio(clk, bugs=[bug])
        c3 = copy.deepcopy(c2)
        c3["results"] = os.path.join(tmp, f"results-txb-{bug}")
        ex = _quiet_run(c3, args, bug, label, rb, clk)
        it = ex[f"suite:{SUITE}"]["items"]
        check(f"suite {SUITE}: a radio with the {bug} bug fails {label}", it and it[0]["result"] == "fail",
              it[0]["why"][:90] if it else "not run")
    # A radio that keys a refusal probe: the probe is unkeyed at once and fails.
    clk = bench.FakeClock()
    rl = FakeRadio(clk, leaky=[PAST_TX])
    rl.slots[0]["freq"] = 146.52
    ex = _quiet_run(c2, args, "leaky", "refusal_640", rl, clk)
    it = ex[f"suite:{SUITE}"]["items"]
    k_on = rl.calls.index("KEY on") if "KEY on" in rl.calls else -1
    check(f"suite {SUITE}: a radio that keys {PAST_TX:.0f} MHz: KEY off at once, the test fails",
          it and it[0]["result"] == "fail" and k_on >= 0 and rl.calls[k_on + 1] == "KEY off", it[0]["why"][:80] if it else "")
    # The gate: an unapproved frequency is never keyed.
    clk = bench.FakeClock()
    rg = FakeRadio(clk, ignore_freq=True)
    try:
        _quiet_run(c2, args, "gate", "red_led", rg, clk)
        check(f"suite {SUITE}: a slot off the approved frequencies isn't keyed", "KEY on" not in rg.calls, rg.calls[-4:])
    except SafetyError as e:
        check(f"suite {SUITE}: a slot off the approved frequencies isn't keyed", "KEY on" not in rg.calls, str(e)[:80])
    # A split whose TX isn't approved is never keyed: the test (its read-back) and the gate itself.
    clk = bench.FakeClock()
    rs = FakeRadio(clk, split_to=145.600)
    ex = _quiet_run(c2, args, "split-unapproved", "split_tx", rs, clk)
    it = ex[f"suite:{SUITE}"]["items"]
    check(f"suite {SUITE}: a split that stores TX 145.600: split_tx fails, nothing keyed",
          it and it[0]["result"] == "fail" and "KEY on" not in rs.calls, it[0]["why"][:80] if it else "not run")
    clk = bench.FakeClock()
    rs = FakeRadio(clk)
    rig = Rig(c2, args(build="selftest-txb-gate", tests=None), rs, FakeSdr(rs, clk), clk, threaded=False,
              log=lambda *_: None)
    rig.b.check_build(rs.hello(), rs.info(), None)
    rig.cur = dict(name="gate", checks=[])
    rs.active = 0
    rs.term("slot A vfo 145.49500 fm low tone off")
    rs.term("slot A split 145.60000")
    try:
        with rig.keyup(2.0, "split TX gate"):
            pass
        check(f"suite {SUITE}: the gate refuses a split to 145.600 (TX unapproved)", False, "it keyed")
    except SafetyError as e:
        check(f"suite {SUITE}: the gate refuses a split to 145.600 (TX unapproved)", "KEY on" not in rs.calls,
              str(e)[:80])
    return page


def _quiet_run(cfg, args, bug, label, radio, clk):
    import copy
    c = copy.deepcopy(cfg)
    c["results"] = os.path.join(os.path.dirname(cfg["results"]), f"results-txb-{bug}")
    sdr = FakeSdr(radio, clk)
    sess, d = bench.new_session(c, f"txb-{bug}")
    ex, _ = run_tests(c, args(build=f"selftest-txb-{bug}", tests=label), radio, sdr, clk, d, threaded=False,
                      log=lambda *_: None)
    return ex
