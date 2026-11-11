"""tools/tworadio.py's selftest: two fake Ferrite radios on a fake air, on a virtual clock (bench.py's FakeClock).

A fake radio answers as the harness talks to a real one (LinkRadio's methods: terminal lines, remote keys, the UI
readback in the radio's own line format, SLOT, KEY's remote PTT), and models only what the two-radio tests look at,
as the manual's rules say the firmware does it: squelch on a carrier from the other radio, tone squelch and the tone
hint (TSRCH-12 to TSRCH-15), watch and quick mute with the speaker, the broadcast slot's FM/muted, busy lockout, the
backlight and its wake, Battery Save's hold and cycle on REG_30, a channel scan of a zone with the resume rules,
scan inputs, \\* and # skips and PTT on a hit, the zones page's delete, the crossband repeater (XB-*: `xb`, its
start page and grid by keys and knob, passes with the tone gate, hang, timeout, the pass's end with the end tone
and the CW ID, any key stopping it, SIDE1 listening, the strip and cells, never on after a restart). `bugs` breaks
one thing on purpose, so the selftest sees the test that must catch it fail. The air carries each keyed radio's
carrier on its transmit frequency with its tone, until 400 ms after KEY off (TX-47's hold) or at once on a lost
keepalive, and the repeater's output; `FakeAirSdr` renders it as an RTL-SDR capture would (bench.py's fake_dev).

    tools/tworadio.py --selftest          # the whole run, the gates, the duty limit, restore, the results' shape
    tools/tworadio.py --selftest --sim    # two simulators: connect, the gates refuse, a fixture made and removed
"""
import argparse
import contextlib
import json
import os
import re
import tempfile

import bench
import ferrite_link as fl

BCAST_LO, BCAST_HI = 8_750_000, 10_800_000
TX_BANDS = ((14_400_000, 14_800_000), (42_000_000, 45_000_000))
DEFAULTS = {
    "0 1 1": ("Watch", "A+B"), "0 1 6 1": ("Pause time", "1 min"),
    "0 1 2": ("Resume", "5 s"), "0 1 6 3": ("Scan inputs", "Off"), "0 1 6 4": ("Return to", "Start"),
    "0 1 3": ("Priority check", "Every 2 s"), "0 7 2": ("Layout", "Watch"), "0 7 4 1": ("Timeout", "1 min"),
    "0 7 4 4": ("Wake on", "Keys and RX"), "0 7 7 1": ("Battery Save", "Off"), "0 8 4": ("End tone", "Off"),
    "0 8 6 1": ("Callsign", "Empty"), "0 9 3 1": ("PIN lock", "Off"),
}
XB_TIMEOUTS = (30, 60, 120, 180, 300, 600)
XB_HANGS = (0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0)
XB_NEXT = [(0, 1, 0, 2), (0, 1, 1, 4), (2, 3, 0, 5), (2, 4, 0, 6), (3, 4, 1, 7), (5, 6, 2, 5), (5, 7, 3, 6),
           (6, 7, 4, 7)]
MORSE = {"A": ".-", "B": "-...", "C": "-.-.", "D": "-..", "E": ".", "F": "..-.", "G": "--.", "H": "....", "I": "..",
         "J": ".---", "K": "-.-", "L": ".-..", "M": "--", "N": "-.", "O": "---", "P": ".--.", "Q": "--.-", "R": ".-.",
         "S": "...", "T": "-", "U": "..-", "V": "...-", "W": ".--", "X": "-..-", "Y": "-.--", "Z": "--..", "0": "-----",
         "1": ".----", "2": "..---", "3": "...--", "4": "....-", "5": ".....", "6": "-....", "7": "--...",
         "8": "---..", "9": "----.", "/": "-..-."}


def cw_elements(text, t0, dit=0.06):
    """The CW ID's tone elements (start, end) from t0 at 20 wpm (VOX-12)."""
    out, t = [], t0
    for ch in text.upper():
        for e in MORSE.get(ch, ""):
            n = dit * (3 if e == "-" else 1)
            out.append((t, t + n))
            t += n + dit
        t += 2 * dit
    return out, t


def f10(mhz):
    return int(round(float(mhz) * 100000))


def mhz5(f):
    return f"{f / 100000:.5f}"


def secs_of(v):
    m = re.match(r"(\d+)\s*(s|min)", v)
    return int(m.group(1)) * (60 if m.group(2) == "min" else 1) if m else None


def q(s):
    return '"' + str(s).replace("\\", "\\\\").replace('"', '\\"') + '"'


def band(f):
    return "VHF" if f < 30_000_000 else "UHF" if f >= 30_000_000 else "?"


class Air:
    """Each keyed radio's carrier: its frequency and tone, until its end (None while keyed)."""

    def __init__(self, clock):
        self.clock, self.tx, self.log = clock, {}, []

    def key(self, who, freq, tone, at=None):
        c = dict(freq=freq, tone=tone, start=self.clock.time() if at is None else at, end=None, id=object())
        self.tx[who] = c
        self.log.append(c)
        return c

    def unkey(self, who, hold, at=None):
        c = self.tx.get(who)
        if c and c["end"] is None:
            c["end"] = (self.clock.time() if at is None else at) + hold

    def carrier(self, freq, me, now):
        for who, c in self.tx.items():
            if who.split(":")[0] != me.split(":")[0] and c["freq"] == freq and c["start"] <= now \
                    and (c["end"] is None or now < c["end"]):
                return c
        return None

    def keyed(self):
        return [w for w, c in self.tx.items() if c["end"] is None]


class FakeFerrite:
    """One fake radio (see the module's doc). `lat`: seconds each request takes (cable 0.02, Bluetooth 0.5)."""

    def __init__(self, name, clock, air, spec=None, build="tx-dry", bench_bit=True, ble=False, bugs=(),
                 hold_answers=False):
        self.name, self.clock, self.air = name, clock, air
        self.spec = spec or f"fake:{name}"
        self.ble, self.sim, self.cable = ble, False, not ble
        self.build, self.bench, self.version = build, bench_bit and build != "tx+dry", "v0.1.0-fake"
        self.dry_tx = build == "tx+dry"  # TX-48: Dry TX on a tx build; a test turns it off "on the radio"
        self.answer_s = 2.0 if ble else 0.6
        self.lat = 0.5 if ble else 0.02
        self.bugs, self.hold = set(bugs), hold_answers
        self.slots = [dict(freq=f10(146.52), mode="FM", power="low", rxtone="off", txtone="off", offset=0, ch=None)
                      for _ in range(3)]
        self.slots[1]["freq"] = f10(446.0)
        self.slots[2]["freq"] = f10(162.55)
        self.vfo = [dict(s) for s in self.slots]
        self.lastch = [None, None, None]
        self.active = 0
        self.set = {p: v for p, (_, v) in DEFAULTS.items()}
        self.channels = {7: dict(name="Club", freq=f10(146.94), offset=-60_000, rxtone="100.0", txtone="100.0",
                                 power="low", zone=1, busy=False, skip=False, rxonly=False)}
        self.zones = [dict(name="Repeaters", resume="radio")]
        self.mute, self.monitor = set(), set()
        self.stack = [("Home", None)]
        self.toast = None
        self.last_key = clock.time()
        self.last_sq = clock.time()
        self.paused_until = None
        self.live = [dict(open=False, since=None, closed=None, carrier=None, heard=None, heard_until=None,
                          measured=False, tone_ok=False) for _ in range(3)]
        self.scan = [None, None, None]
        self.audio = None
        self.bcast_back = None
        self.keyed = None
        self.unkey_msgs = []
        self.t = clock.time()
        self.log = []  # every request, for the selftest's checks
        self.refused_until = 0.0
        self.xbcfg = dict(side1=0, side2=1, power="Low", timeout=180, hang=1.0, gate=True, id=False, speaker=False)
        self.xb = None
        self.fail_after = None  # a fault: after this many more requests every one times out (a stalled link)

    # ---- the world ----

    def tx_freq(self, s):
        return s["freq"] + s["offset"]

    def watch_set(self):
        return {"Off": set(), "A+B": {0, 1}, "A+B+C": {0, 1, 2}}[self.set["0 1 1"]]

    def watched(self, i):
        w = self.watch_set()
        paused = self.paused_until is not None and self.t < self.paused_until
        return i == self.active or (i in w and not paused)

    def is_bcast(self, i):
        return BCAST_LO <= self.slots[i]["freq"] <= BCAST_HI and self.slots[i]["ch"] is None

    def tone_match(self, want, got):
        if want == "off":
            return True
        if "dcs_either" in self.bugs and want.startswith("D") and got.startswith("D"):
            return want[:4] == got[:4]
        return want == got

    def advance(self):
        now = self.clock.time()
        while self.t + 0.01 <= now:
            self.t += 0.01
            self.step()
        self.t = max(self.t, now)

    def step(self):
        now = self.t
        if self.keyed and now - self.keyed["alive"] > 2.0:  # LINK-20: no keepalive for 2 s, a cut
            self.air.unkey(self.name, 0.0)
            self.keyed = None
            self.unkey_msgs.append("invalid: keepalive lost")
            self.stack = [x for x in self.stack if x[0] != "Transmit"]
        if self.xb:
            self.xb_step()  # XB-13: watch and scan pause while it runs
            return
        if self.screen() == "ToneSearch":
            self.tsearch_step(self.stack[-1][1])  # TSRCH-3: its own exclusive mode; watch and scans pause
            return
        audible = []
        for i, s in enumerate(self.slots):
            lv = self.live[i]
            sc = self.scan[i]
            if self.is_bcast(i):
                continue
            listening = (sc["pos"][sc["idx"]][0] if sc else s["freq"])
            c = self.air.carrier(listening, self.name, now) if (sc or self.watched(i)) and not self.keyed else None
            if sc:
                self.scan_step(i, c)
                lv["open"] = sc["state"] != "walk" and c is not None
                continue
            was = lv["open"]
            lv["open"] = c is not None
            if lv["open"] and not was:
                lv["since"], lv["carrier"], lv["measured"] = now, c["id"], False
                self.last_sq = now
            if was and not lv["open"]:
                lv["closed"] = now
                self.last_sq = now
                if lv["heard"] is not None:
                    lv["heard_until"] = now + 30.0
            lv["tone_ok"] = lv["open"] and self.tone_match(s["rxtone"], c["tone"])
            # The tone hint (TSRCH-12 to TSRCH-15): a carrier the tone squelch keeps shut for 0.5 s, measured once.
            if lv["tone_ok"] and s["rxtone"] != "off":
                lv["heard"] = None  # the tone squelch opened
            elif lv["open"] and s["rxtone"] != "off" and not lv["measured"] and now - lv["since"] >= 0.8 \
                    and i not in self.monitor:
                lv["heard"], lv["measured"], lv["heard_until"] = c["tone"], True, None
            if lv["heard"] is not None and not lv["open"] and lv["heard_until"] is not None and now >= lv["heard_until"] \
                    and "hint_sticks" not in self.bugs:
                lv["heard"] = None
            if lv["tone_ok"] and i not in self.mute:
                audible.append(i)
            if lv["open"] and i != self.active and "follow" in self.bugs and self.watched(i):
                self.active = i
        audio = self.active if self.active in audible else next(iter(audible), None)
        if self.audio is not None and audio is None:
            self.bcast_back = now + 2.15  # BCR-24: 2 s after the slot's audio ends (its 150 ms tail)
        if audio is not None:
            self.bcast_back = None
        self.audio = audio
        rx_wake = self.set["0 7 4 4"] != "Keys"
        if rx_wake and any(self.live[i]["open"] and self.watched(i) for i in range(3)):
            self.last_key = max(self.last_key, now - 0.001)

    # ---- scanning (SCAN-5, SCAN-11, SCAN-17, SCAN-19, SCAN-20.2, SCAN-22) ----

    def resume_rule(self, zone):
        z = self.zones[zone - 1]["resume"] if zone and zone <= len(self.zones) else "radio"
        if z != "radio":
            return "carrier" if z == "carrier" else "stop" if z == "stop" else int(z.rstrip("s"))
        g = self.set["0 1 2"]
        return "carrier" if g == "Carrier drop" else "stop" if g == "Stop on hit" else secs_of(g)

    def start_scan(self, i):
        s = self.slots[i]
        n0 = s["ch"]
        zone = self.channels[n0]["zone"]
        pos = []
        for n in sorted(k for k, c in self.channels.items() if c["zone"] == zone):
            c = self.channels[n]
            if not c["skip"]:
                pos.append((c["freq"], n))
            if self.set["0 1 6 3"] == "On" and c["offset"]:
                pos.append((c["freq"] + c["offset"], n))
        if not pos:
            return
        idx = next((k for k, p in enumerate(pos) if p[1] == n0), 0)
        self.scan[i] = dict(pos=pos, idx=idx, state="walk", t_next=self.t + 0.07, stop=None, drop=None, start=n0,
                            zone=zone, skips=set())

    def scan_step(self, i, c):
        sc = self.scan[i]
        now = self.t
        cur = sc["pos"][sc["idx"]]
        if sc["state"] == "walk":
            if c is not None and cur[1] not in sc["skips"]:
                sc["state"], sc["stop"], sc["drop"] = "stop", now, None
                if self.resume_rule(sc["zone"]) == "stop":
                    self.end_scan(i, cur[1])
                return
            if now >= sc["t_next"]:
                self.next_pos(i)
            return
        rule = self.resume_rule(sc["zone"])
        wait = 3.0 if "resume_3s" in self.bugs else 2.0
        if c is None:
            sc["drop"] = sc["drop"] if sc["drop"] is not None else now
            if now - sc["drop"] >= wait:
                self.next_pos(i)
                return
        else:
            sc["drop"] = None
        if isinstance(rule, int) and now - sc["stop"] >= rule:
            self.next_pos(i)

    def next_pos(self, i):
        sc = self.scan[i]
        sc["idx"] = (sc["idx"] + 1) % len(sc["pos"])
        sc["state"], sc["t_next"] = "walk", self.t + 0.07

    def end_scan(self, i, on_ch):
        self.scan[i] = None
        self.put_on_channel(i, on_ch)

    def put_on_channel(self, i, n):
        c = self.channels[n]
        s = self.slots[i]
        if s["ch"] is None:
            self.vfo[i] = dict(s)
        s.update(ch=n, freq=c["freq"], offset=c["offset"], rxtone=c["rxtone"], txtone=c["txtone"], power=c["power"])
        self.lastch[i] = n

    def to_vfo(self, i):
        if self.slots[i]["ch"] is not None:
            self.slots[i] = dict(self.vfo[i], ch=None)

    # ---- the link ----

    def _req(self, what):
        self.advance()
        if self.fail_after is not None:
            self.fail_after -= 1
            if self.fail_after < 0:
                self.clock.sleep(6.0 if self.ble else 2.0)
                raise TimeoutError(f"no answer to 0x33 (fake {self.name}: stalled, {what})")
        self.log.append(what)
        self.clock.sleep(self.lat)
        self.advance()

    def close(self):
        pass

    def screen(self):
        return self.stack[-1][0]

    def ui_text(self):
        self._req("UI")
        now = self.t
        timeout = secs_of(self.set["0 7 4 1"]) or 60
        dark = int(now - self.last_key > timeout)
        sid = self.screen()
        keyed = "ABC"[self.keyed["slot"]] if self.keyed else "-"
        out = [f'screen {sid} {q(sid)} title=""',
               f"state frame=1 ago=0 dark={dark} locked=0 active={'ABC'[self.active]} keyed={keyed} "
               f"remote={int(self.keyed is not None)} mode={'xb' if self.xb else 'off'} fm=0",
               f"bar lock=0 scan={int(any(self.scan))} watch={self.bar_watch(sid)} inbox=0 alert=0 gps=off bt=0 "
               f"battery=80 low=0 cutoff=0 dry={int(self.dry_tx)} clock=- utc=0"]
        if self.toast and now < self.toast[2]:
            out.append(f"toast {q(self.toast[0])} {q(self.toast[1])}")
        hints = [("MENU", "Menu")]
        if sid == "Home":
            for i in range(3):
                out.append(self.card(i))
            if any(self.live[i]["open"] and i != self.active and self.watched(i) for i in range(3)):
                hints.append(("#", "Pause"))
            else:
                hints.append(("#", "Status"))
        elif sid == "Status":
            w = self.set["0 1 1"] + (" · paused" if self.paused_until and now < self.paused_until else "")
            out.append(f'row 0 "Watch" {q(w)}')
        elif sid == "Zones":
            sel = self.stack[-1][1]["sel"]
            for k, z in enumerate(self.zones):
                n = sum(1 for c in self.channels.values() if c["zone"] == k + 1)
                out.append(f'row 0 {q(z["name"])} "{n}"' + (" sel" if k == sel else "") + ' kind=zone chips=""')
        elif sid == "TxRefused":
            out.append(f'refused slot={"ABC"[self.active]} why={q(self.stack[-1][1])}')
        elif sid == "Transmit":
            out.append(f"transmit slot={keyed} remote=1 dial=0 secs=0")
        elif sid == "Confirm":
            out.append('ask "Delete zone?" goes="" kept=""')
        elif sid == "Crossband":
            hints = self.xb_lines(out, self.stack[-1][1])
        elif sid == "ToneSearch":
            d = self.stack[-1][1]
            on = self.air.carrier(self.slots[d["slot"]]["freq"], self.name, now) is not None
            out.append(f'tsearch slot={"ABC"[d["slot"]]} found={q(d["found"])} open={int(on)} '
                       f's="{"S9" if on else "S1"}" ask=0 hits={q(",".join(d["hits"]))}')
        elif sid == "Scan":
            sc = self.scan[self.active]
            hints = ([("*", "Skip"), ("#", "Skip always"), ("^v", "Go on")] if sc and sc["state"] != "walk"
                     else [("^v", "Direction"), ("0-9", "Zones"), ("SIDE2", "Stop")])
        out.append("hints " + " ".join(f"{q(k)} {q(v)}" for k, v in hints))
        return "\n".join(out) + "\n"

    def card(self, i):
        s, lv, sc = self.slots[i], self.live[i], self.scan[i]
        freq = sc["pos"][sc["idx"]][0] if sc else s["freq"]
        if self.is_bcast(i):
            st = "FM" if (self.audio is None and (self.bcast_back is None or self.t >= self.bcast_back)) else "muted"
            return (f'card {"ABC"[i]} active={int(i == self.active)} head="VFO · Broadcast" state="{st}" '
                    f'freq={mhz5(freq)} name="" mode="FM" tone="" shift="" power="LOW" s="" meter=0 rx=0 scan=0 '
                    "kind=broadcast")
        if i in self.mute:
            st = "RX muted" if lv["open"] else "Muted"
        elif i != self.active and i not in self.watch_set() and not sc:
            st = "off"
        elif lv["heard"] is not None:
            st = "heard"
        elif lv["open"]:
            st = "RX"
        elif sc:
            st = "scanning"
        elif self.paused_until and self.t < self.paused_until and i != self.active:
            st = "paused"
        else:
            st = "idle"
        head = ("SCAN" if sc else f"{s['ch']:03d}" if s["ch"] is not None else "VFO") + f" · {band(freq)}"
        name = self.channels[s["ch"]]["name"] if s["ch"] is not None and not sc else ""
        meter = 5 if lv["open"] else 0
        return (f'card {"ABC"[i]} active={int(i == self.active)} head={q(head)} state={q(st)} freq={mhz5(freq)} '
                f'name={q(name)} mode="FM" tone="" shift="" power="LOW" s="{"S9" if meter else ""}" meter={meter} '
                f'rx={int(lv["open"])} scan={int(sc is not None)} kind=slot')

    def slot_bin(self, n):
        self._req("SLOT")
        s = self.slots[n]
        return dict(active=self.active == n, channel=None if s["ch"] is None else s["ch"] - 1, freq10=s["freq"],
                    mode=0, power=["low", "mid", "high"].index(s["power"]), rx_tone=0, tx_tone=0, offset=s["offset"],
                    split=False, aprs=False, scanning=self.scan[n] is not None)

    # ---- the crossband repeater (chapter 19, XB-20) ----

    @property
    def callsign(self):
        c = self.set["0 8 6 1"]
        return "" if c == "Empty" else c

    def port_word(self):
        return "bt" if self.ble else "cable"

    def bar_watch(self, sid):
        """SBAR-16: DW/TW where watch matters (home); never while the repeater runs."""
        if sid != "Home" or self.xb:
            return 0
        n = len(self.watch_set() | {self.active})
        return n if self.set["0 1 1"] != "Off" and n >= 2 else 0

    def xb_side(self, k):
        return self.slots[self.xbcfg["side1" if k == 0 else "side2"]]

    def xb_grid(self):
        c = self.xbcfg
        to = c["timeout"]
        return [("Power", c["power"]), ("Timeout", f"{to} s" if to < 60 else f"{to // 60} min"),
                ("Hang", f"{c['hang']:.1f} s"), ("Tone gate", "On" if c["gate"] else "Off (carrier)"),
                ("ID", "CW 10 m" if c["id"] else "Off"), ("Speaker", "On" if c["speaker"] else "Off")]

    def xb_why_not(self):
        """XB-2's refusal, or None."""
        f1, f2 = self.xb_side(0)["freq"], self.xb_side(1)["freq"]
        if (f1 >= 30_000_000) == (f2 >= 30_000_000):
            return "one chip"
        if self.build not in ("tx", "tx-dry", "tx+dry") or not all(any(lo <= f <= hi for lo, hi in TX_BANDS) for f in (f1, f2)):
            return "Side 1 can't transmit there"
        if self.xbcfg["id"] and not self.callsign:
            return "ID needs a callsign (setup)"
        return None

    def xb_lines(self, out, data):
        x, now = self.xb, self.t
        if x:
            st = x["state"]
            passing = st in (1, 2, 3)
            pass_s = int(now - x["pass_start"]) if passing else x["last_len"]
            out.append(f"xb running=1 state={st} by={x['by']} side1=A side2=B pass={pass_s} passes={x['passes']} "
                       f"listen={x['listen']} ab={len(x['strip'][0])} ba={len(x['strip'][1])}")
            busy = x["busy"] + (now - x["pass_start"] if passing else 0)
            out.append(f'row 0 "Passes" "{x["passes"]} · {int(busy) // 60} min"')
            out.append(f'row 0 "Duty, 10 min" "{int(busy * 100 / 600)}% of 50%"')
            nid = 0 if x["last_id"] is None else max(0, 600 - (now - x["last_id"]))
            nxt = f"CW in {-(-int(nid) // 60)} min" if self.xbcfg["id"] else "Off"
            out.append(f'row 0 "Next ID" "{nxt}"')
            last = "now" if passing else "none yet" if x["last_end"] is None else f"{int(now - x['last_end'])} s ago"
            out.append(f'row 0 "Last pass" "{last}"')
            return [("Any key", "Stop"), ("SIDE1", "Listen")]
        cell = data["cell"]
        out.append(f"xb running=0 cell={cell}")
        for k in range(2):
            sl = self.xbcfg["side1" if k == 0 else "side2"]
            out.append(f'row 0 "Side {k + 1}" "{"ABC"[sl]} {mhz5(self.slots[sl]["freq"])}"'
                       + (" sel" if cell == k else ""))
        for i, (n, v) in enumerate(self.xb_grid()):
            out.append(f'row 0 {q(n)} {q(v)}' + (" sel" if cell == 2 + i else ""))
        why = self.xb_why_not()
        out.append(f'check {"fail" if why else "ok"} {q(why or "two chips")}')
        return [("Knob", "Change"), ("#", "Start"), ("BACK", "Back")]

    def xb_knob(self, cell, d):
        c = self.xbcfg
        if cell == 2:
            c["power"] = "Mid" if d > 0 else "Low"
        elif cell == 3:
            i = XB_TIMEOUTS.index(c["timeout"]) if c["timeout"] in XB_TIMEOUTS else 3
            c["timeout"] = XB_TIMEOUTS[max(0, min(len(XB_TIMEOUTS) - 1, i + d))]
        elif cell == 4:
            i = XB_HANGS.index(c["hang"]) if c["hang"] in XB_HANGS else 2
            c["hang"] = XB_HANGS[max(0, min(len(XB_HANGS) - 1, i + d))]
        elif cell in (5, 6, 7):
            k = {5: "gate", 6: "id", 7: "speaker"}[cell]
            c[k] = not c[k]

    def xb_start(self, by):
        why = self.xb_why_not()
        if why:
            self.toast = ("Can't start:", why, self.t + 2)
            return why
        self.xb = dict(state=0, by=by, since=[None, None], lock=None, passes=0, busy=0.0, strip=[set(), set()],
                       last_end=None, last_len=0, last_id=None, listen=0, pass_start=None, drop=None, frm=0)
        if self.screen() != "Crossband":
            self.stack = [("Home", None), ("Crossband", dict(cell=0))]
        return None

    def xb_stop(self, by):
        x = self.xb
        if x["state"] in (1, 2, 3):
            self.xb_end(cut=True)
        self.xb = None
        self.toast = ("Repeater stopped", "from the link" if by.startswith(("keys", "cmd")) else "", self.t + 2)

    def xb_heard(self, k):
        s = self.xb_side(k)
        c = self.air.carrier(s["freq"], self.name, self.t)
        if c is None:
            return None
        if self.xbcfg["gate"] and s["rxtone"] != "off" and not self.tone_match(s["rxtone"], c["tone"]):
            return None
        return c

    def xb_step(self):
        x, now, cfg = self.xb, self.t, self.xbcfg
        heard = [self.xb_heard(0), self.xb_heard(1)]
        if x["state"] in (1, 2, 3):
            heard[1 - x["frm"]] = None  # XB-5: the transmitting side's input is ignored
        for k in (0, 1):
            if heard[k] is None:
                x["since"][k] = None
                if x["lock"] == k:
                    x["lock"] = None  # XB-9: the timed-out side's signal dropped
            elif x["since"][k] is None:
                x["since"][k] = now
        st = x["state"]
        if st == 0:
            for k in (0, 1):
                if heard[k] is not None and x["lock"] != k and now - x["since"][k] >= 0.03:
                    self.xb_pass(k, heard[k])
                    break
        elif st in (1, 2, 3):
            k = x["frm"]
            if heard[k] is not None:
                x["drop"], x["state"] = None, k + 1
                if now - x["pass_start"] >= cfg["timeout"]:
                    self.xb_end(timed_out=True)
            else:
                x["drop"] = now if x["drop"] is None else x["drop"]
                hang = 0.0 if "xb_no_hang" in self.bugs else cfg["hang"]
                if now - x["drop"] >= 0.1 + hang - 1e-9:
                    self.xb_end()
                elif now - x["drop"] >= 0.1:
                    x["state"] = 3
        elif st == 4 and x["lock"] is None:
            x["state"] = 0

    def xb_pass(self, k, c):
        x, now = self.xb, self.t
        out = self.xb_side(1 - k)
        tone = c["tone"] if "xb_wrong_tone" in self.bugs else out["txtone"]
        self.air.key(self.name + ":xb", out["freq"], tone, at=now)
        x.update(state=k + 1, frm=k, pass_start=now, drop=None)
        x["passes"] += 1
        x["strip"][k].add(int(now // 60))

    def xb_end(self, timed_out=False, cut=False):
        x, now = self.xb, self.t
        c = self.air.tx.get(self.name + ":xb")
        hold = 0.5  # 400 ms carrier hold, the 100 ms unkey wait
        if c is not None and not cut and not timed_out:
            t = now
            et = self.set["0 8 4"]
            if et != "Off":
                c["endtone"], c["t_off"] = et, now
                slug = dict(bench.ENDTONE_SETTINGS)[et]
                t += 0.2 + sum(ms for _, ms in dict(bench.END_TONES)[slug]) / 1000
            if self.xbcfg["id"] and self.callsign and (x["last_id"] is None or now - x["last_id"] >= 600):
                c["cw"], t = cw_elements(self.callsign, t + 0.2)
                x["last_id"] = now
            hold += t - now
        self.air.unkey(self.name + ":xb", hold, at=now)
        x["busy"] += now - x["pass_start"]
        x["last_len"], x["last_end"] = int(now - x["pass_start"]), now
        x["state"], x["drop"] = (4, None) if timed_out else (0, None)
        if timed_out:
            x["lock"] = x["frm"]

    def restart(self):
        """REBOOT: the radio comes back home; the repeater is never on at power-up (XB-1)."""
        self._req("REBOOT")
        if self.keyed:
            self.air.unkey(self.name, 0.0)
            self.keyed = None
        if self.xb and "xb_on_at_boot" not in self.bugs:
            if self.xb["state"] in (1, 2, 3):
                self.xb_end(cut=True)
            self.xb = None
        self.clock.sleep(6.0)
        self.advance()
        if not self.xb:
            self.stack = [("Home", None)]
        self.toast = None

    def cmd_xb(self, w):
        sides = f"A {mhz5(self.xb_side(0)['freq'])} <-> B {mhz5(self.xb_side(1)['freq'])}"
        by = f"cmd:{self.port_word()}"
        if len(w) == 1:
            return (f"Crossband: running, {sides}, started by {self.xb['by']}" if self.xb
                    else f"Crossband: off ({sides})")
        if w[1] == "start":
            if self.xb:
                self.err("xb start: already running; xb stop stops it")
            if self.keyed:
                self.err("xb start: transmitting: send it after the unkey")
            self.stack = [("Home", None), ("Crossband", dict(cell=0))]
            why = self.xb_start(by)
            if why:
                self.err(f"xb start: can't start: {why}")
            return f"Crossband started: {sides}"
        if w[1] == "stop":
            if not self.xb:
                self.err("xb stop: the repeater isn't running")
            self.xb_stop(by)
            return "Crossband stopped"
        self.err("xb, xb start or xb stop: the crossband repeater")

    # ---- keys ----

    def keys(self, seq, long=False):
        for word in seq.split():
            name, _, how = word.partition(":")
            how = how.lower() or ("long" if long else "short")
            self.press(name, long=how == "long", how=how)

    def press(self, name, long=False, how=None):
        how = how or ("long" if long else "short")
        self._req(f"KEY {name}:{how}")
        if self.keyed:
            raise fl.RadioError(9, b"transmitting: remote keys wait for the unkey")
        self.last_key = self.t
        name = name.upper()
        if name == "#":
            name = "HASH"
        sid, data = self.stack[-1]
        if self.xb:
            # XB-20.5: any key but SIDE1 stops it and does nothing else; SIDE1 held listens (XB-15)
            if name in ("SIDE1", "PF1"):
                if how in ("down", "up"):
                    self.xb["listen"] = int(how == "down")
                return
            if how != "up" and "xb_key_ignored" not in self.bugs:
                self.xb_stop(f"keys:{self.port_word()}")
            return
        if how == "up":
            return
        if sid == "Crossband" and name in ("LEFT", "RIGHT", "UP", "DOWN"):
            data["cell"] = XB_NEXT[data["cell"]][("LEFT", "RIGHT", "UP", "DOWN").index(name)]
            return
        if sid == "Crossband" and name == "HASH":
            self.xb_start(f"keys:{self.port_word()}")
            return
        if name == "BACK" and long:
            self.stack = [("Home", None)]
            return
        if name == "BACK":
            if len(self.stack) > 1:
                self.stack.pop()
            return
        if sid == "Home":
            self.home_key(name)
        elif sid == "Quick" and name == "0":
            self.stack.append(("More", None))
        elif sid == "More" and name == "0":
            self.stack.append(("Status", None))
        elif sid == "More" and name == "3":
            self.stack.append(("Tools", None))
        elif sid == "Tools" and name == "5":
            self.stack.append(("Crossband", dict(cell=0)))
        elif sid == "Tools" and name == "6":
            self.stack.append(("ToneSearch", dict(slot=self.active, found="", since=None, hits=[])))
        elif sid == "More" and name == "2":
            self.stack.append(("Group", None))
        elif sid == "Group" and name == "3":
            self.stack.append(("Zones", dict(sel=0)))
        elif sid == "Zones" and name in ("UP", "DOWN") and self.zones:
            data["sel"] = (data["sel"] + (-1 if name == "UP" else 1)) % len(self.zones)
        elif sid == "Zones" and name == "STAR" and self.zones:
            self.stack.append(("Confirm", dict(zone=data["sel"])))
        elif sid == "Confirm" and name in ("MENU", "OK"):
            k = data["zone"]
            del self.zones[k]
            for c in self.channels.values():
                if c["zone"] == k + 1:
                    c["zone"] = None
                elif c["zone"] and c["zone"] > k + 1:
                    c["zone"] -= 1
            self.stack.pop()
            self.stack[-1][1]["sel"] = 0
        elif sid == "Entry":
            if name.isdigit():
                data["n"] += name
            elif name in ("MENU", "OK"):
                n = int(data["n"] or 0)
                self.stack.pop()
                if n in self.channels:
                    self.put_on_channel(self.active, n)
        elif sid == "TxRefused":
            self.stack.pop()
        elif sid == "Scan":
            self.scan_key(name)

    def tsearch_step(self, d):
        """Tone search (TSRCH-3): the slot's carrier read for 0.3 s (two agreeing detector reads) gives its tone, as
        the card would show it ("T 88.5", "D023N"); it stays after the carrier."""
        c = self.air.carrier(self.slots[d["slot"]]["freq"], self.name, self.t)
        if c is None or c["tone"] in (None, "off", ""):
            d["since"] = None
            return
        d["since"] = self.t if d["since"] is None else d["since"]
        if self.t - d["since"] >= 0.3:
            word = c["tone"] if c["tone"].startswith("D") else f"T {c['tone']}"
            if word != d["found"]:
                d["found"] = word
                d["hits"].insert(0, word)
                del d["hits"][3:]

    def scan_key(self, name):
        """The scan screen: \\* skips the hit for this scan, # for every scan (the channel's Skip flag), SIDE2 stops."""
        a = self.active
        sc = self.scan[a]
        if sc is None:
            return
        stopped = sc["state"] != "walk"
        if name == "STAR" and stopped:
            sc["skips"].add(sc["pos"][sc["idx"]][1])
            self.next_pos(a)
        elif name == "HASH" and stopped:
            n = sc["pos"][sc["idx"]][1]
            self.channels[n]["skip"] = True
            self.toast = (f"{n:03d} skipped", "for every scan", self.t + 2)
            sc["pos"] = [p for p in sc["pos"] if p[1] != n] or sc["pos"]
            sc["idx"] = sc["idx"] % len(sc["pos"])
            sc["state"], sc["t_next"] = "walk", self.t + 0.07
        elif name == "SIDE2":
            self.scan[a] = None
            self.stack = [("Home", None)]

    def home_key(self, name):
        a = self.active
        sc = self.scan[a]
        if name in ("MENU", "OK"):
            self.stack.append(("Quick", None))
        elif name == "STAR" and sc:
            if sc["state"] != "walk":
                n = sc["pos"][sc["idx"]][1]
                if len(sc["skips"]) < 8:
                    sc["skips"].add(n)
                self.next_pos(a)
        elif name == "STAR":
            if a in self.mute:
                self.mute.discard(a)
                self.toast = ("A unmuted", "", self.t + 2)
            else:
                self.mute.add(a)
                self.toast = (f"{'ABC'[a]} muted", "\\* again unmutes", self.t + 2)
        elif name == "HASH":
            if any(self.live[i]["open"] and i != a and self.watched(i) for i in range(3)):
                self.paused_until = self.t + (secs_of(self.set["0 1 6 1"]) or 60)
                self.toast = ("Watch paused", self.set["0 1 6 1"], self.t + 2)
            else:
                self.stack.append(("Status", None))
        elif name == "SIDE2":
            if sc:
                self.scan[a] = None
            elif self.slots[a]["ch"] is not None:
                self.start_scan(a)
                self.stack.append(("Scan", None))  # the scan screen; hold BACK goes home, the scan running
        elif name == "VM":
            s = self.slots[a]
            if s["ch"] is not None:
                self.to_vfo(a)
            else:
                n = self.lastch[a] if self.lastch[a] in self.channels else min(self.channels)
                self.put_on_channel(a, n)
        elif name.isdigit() and self.slots[a]["ch"] is not None:
            self.stack.append(("Entry", dict(n=name)))
        elif name == "SLOT":
            self.active = (a + 1) % 3

    def knob(self, n):
        self._req("KNOB")
        sid, data = self.stack[-1]
        if sid == "Crossband" and not self.xb:
            for _ in range(abs(n)):
                self.xb_knob(data["cell"], 1 if n > 0 else -1)

    # ---- the terminal ----

    def err(self, why):
        raise fl.RadioError(9, why.encode())

    def slot_line(self, i):
        s = self.slots[i]
        where = f'ch {s["ch"]} {q(self.channels[s["ch"]]["name"])}' if s["ch"] is not None else "vfo"
        tone = f"tone={s['txtone']}" if s["txtone"] == s["rxtone"] else f"txtone={s['txtone']} rxtone={s['rxtone']}"
        off = f" offset={'+' if s['offset'] > 0 else '-'}{mhz5(abs(s['offset']))}" if s["offset"] else ""
        return (f"{'ABC'[i]}{'*' if i == self.active else ' '} {where} {mhz5(s['freq'])} mode={s['mode']} "
                f"power={s['power']} {tone}{off}{' scanning' if self.scan[i] else ''}")

    def ch_line(self, n):
        c = self.channels[n]
        off = "simplex" if not c["offset"] else f"{'+' if c['offset'] > 0 else '-'}{mhz5(abs(c['offset']))}"
        tone = f"tone={c['txtone']}" if c["txtone"] == c["rxtone"] else f"txtone={c['txtone']} rxtone={c['rxtone']}"
        out = f'ch {n} {q(c["name"])} {mhz5(c["freq"])} offset={off} mode=FM {tone} power={c["power"]}'
        out += f" zone={c['zone']}" if c["zone"] else ""
        out += " busy=on" if c["busy"] else ""
        out += "".join(f" {k}" for k in ("skip", "rxonly") if c[k])
        return out

    def cmd(self, line):
        self._req(f"CMD {line}")
        w = line.split()
        if w[0] == "slot":
            return self.cmd_slot(w, line)
        if w[0] == "get":
            p = " ".join(w[1:])
            if p not in self.set:
                self.err(f"get {p}: not a setting here (fake)")
            return f"{p} {DEFAULTS[p][0]}: {self.set[p]}" + (" (not the default)" if self.set[p] != DEFAULTS[p][1] else "")
        if w[0] == "set":
            p = next((k for k in sorted(self.set, key=len, reverse=True) if line.startswith(f"set {k} ")), None)
            if p is None:
                self.err(f"{line}: not a setting here (fake)")
            if self.keyed:
                self.err("transmitting")
            if p in ("0 1 6 1", "0 1 6 4"):  # as the firmware (Oct 14, 2026)
                self.err(f"{DEFAULTS[p][0]} can't be set yet")
            self.set[p] = line[len(f"set {p} "):].strip()
            return f"{p} {DEFAULTS[p][0]}: {self.set[p]}"
        if w[0] == "ch":
            return self.cmd_ch(w, line)
        if w[0] == "zone":
            n = int(w[1])
            m = re.match(r'zone \d+ "([^"]*)"\s*(.*)$', line)
            if n > len(self.zones) + 1:
                self.err(f"zone {n}: zones are made in order; the radio has {len(self.zones)}, so the next is zone "
                         f"{len(self.zones) + 1}")
            kv = dict(re.findall(r"(\w+)=(\S+)", m.group(2)))
            if n == len(self.zones) + 1:
                self.zones.append(dict(name=m.group(1), resume="radio"))
            z = self.zones[n - 1]
            z["name"] = m.group(1)
            if "resume" in kv:
                z["resume"] = kv["resume"]
            return f"Zone {n} saved"
        if w[0] == "xb":
            return self.cmd_xb(w)
        if w[0] == "bk" and len(w) == 3:
            return f"  bk{w[1]} REG_{w[2].upper()} = 0x{self.reg30():04X}   ...\n"
        self.err(f"{w[0]}: no such command")

    def reg30(self):
        lvl = self.set["0 7 7 1"]
        if lvl == "Off" or any(lv["open"] for lv in self.live) or any(self.scan):
            return 0xBFF1
        hold = max(self.last_key, self.last_sq) + 10.0
        if self.t < hold:
            return 0xBFF1
        return 0xBFF1 if int((self.t - hold) / 0.3) % 2 == 0 else 0x0000

    def cmd_slot(self, w, line):
        if len(w) == 1:
            return "\n".join(self.slot_line(i) for i in range(3)) + "\n"
        i = "ABC".index(w[1].upper())
        if len(w) == 2:
            return self.slot_line(i)
        if self.keyed:
            self.err(f"slot {w[1]}: transmitting: nothing changes while keyed (TX-22)")
        if self.scan[i]:
            self.err(f"slot {w[1]}: the slot is scanning")
        s = self.slots[i]
        k = 2
        while k < len(w):
            x = w[k].lower()
            if x == "vfo":
                self.to_vfo(i)
                s = self.slots[i]
            elif x == "active":
                self.active = i
            elif x in ("fm", "nfm", "am"):
                s["mode"] = x.upper()
            elif x in ("low", "mid", "high"):
                s["power"] = x
            elif x in ("tone", "txtone", "rxtone"):
                k += 1
                for key in (("txtone", "rxtone") if x == "tone" else (x,)):
                    s[key] = w[k]
                self.retune(i)
            elif x == "offset":
                k += 1
                s["offset"] = 0 if w[k] in ("off", "simplex") else f10(w[k])
            elif x[0].isdigit():
                if "sides_ignored" in self.bugs:  # a fault: the line answered, the slot left as it was
                    k += 1
                    continue
                if s["ch"] is not None:
                    self.err(f"slot {w[1]}: the slot is on a channel: add vfo to tune its VFO")
                s["freq"] = f10(x)
                self.retune(i)
            else:
                self.err(f"{x}: not a slot word")
            k += 1
        return self.slot_line(i)

    def retune(self, i):
        lv = self.live[i]
        lv["heard"] = None  # TSRCH-14: a new frequency or receive tone clears the hint at once

    def cmd_ch(self, w, line):
        n = int(w[1])
        if len(w) == 2:
            if n not in self.channels:
                self.err(f'ch {n}: no such channel')
            return self.ch_line(n)
        if w[2] == "delete":
            self.channels.pop(n, None)
            return f"Channel {n} deleted"
        if not w[2].startswith('"'):
            c = self.channels[n]
            field, v = w[2], w[3]
            if field in ("skip", "rxonly", "busy"):
                c[field] = v == "on"
            return self.ch_line(n)
        m = re.match(r'ch \d+ "([^"]*)" (\S+)\s*(.*)$', line)
        kv = dict(re.findall(r"(\w+)=(\S+)", m.group(3)))
        tone = kv.get("tone", "off")
        self.channels[n] = dict(name=m.group(1), freq=f10(m.group(2)),
                                offset=0 if kv.get("offset", "simplex") == "simplex" else f10(kv["offset"]),
                                rxtone=tone, txtone=tone, power=kv.get("power", "low"),
                                zone=int(kv["zone"]) if "zone" in kv else None, busy=kv.get("busy") == "on",
                                skip=kv.get("skip") == "on", rxonly=kv.get("rxonly") == "on")
        return f"Channel {n} saved"

    # ---- KEY's remote PTT ----

    def key_on(self, code):
        self._req("KEY on")
        if self.hold:
            return None
        i = self.active
        sc = self.scan[i]
        if sc:  # SCAN-19: onto the hit if stopped on it (or in its resume wait), else back where the scan started
            n = sc["pos"][sc["idx"]][1] if sc["state"] != "walk" else sc["start"]
            self.end_scan(i, n)
        s = self.slots[i]
        f = self.tx_freq(s)
        if self.build not in ("tx", "tx-dry", "tx+dry"):
            return 9, "invalid: receive-only build"
        if not any(lo <= f <= hi for lo, hi in TX_BANDS):
            return 9, "invalid: outside the band plan"
        busy = s["ch"] is not None and self.channels[s["ch"]]["busy"] and self.live[i]["open"]
        if busy and "no_busy" not in self.bugs:
            self.stack.append(("TxRefused", "channel busy"))
            return 9, "invalid: channel busy"
        if s["ch"] is not None and self.channels[s["ch"]]["rxonly"]:
            self.stack.append(("TxRefused", "receive-only channel"))
            return 9, "invalid: receive-only channel"
        self.keyed = dict(slot=i, alive=self.t, freq=f)
        self.air.key(self.name, f, s["txtone"])
        self.stack.append(("Transmit", None))
        return 0, ""

    def key_alive(self):
        self._req("KEY alive")
        if self.keyed:
            self.keyed["alive"] = self.t

    def poll_unkey(self):
        self.advance()
        return self.unkey_msgs.pop(0) if self.unkey_msgs else None

    def key_off(self):
        self._req("KEY off")
        if self.keyed:
            self.air.unkey(self.name, 0.4)  # TX-47: a let-go unkey holds the carrier 400 ms
            self.keyed = None
            self.stack = [x for x in self.stack if x[0] != "Transmit"] or [("Home", None)]
        return True

    # ---- for the selftest ----

    def state(self):
        return json.dumps(dict(slots=self.slots, active=self.active, set=self.set, channels=self.channels,
                               zones=self.zones, mute=sorted(self.mute), scan=[bool(x) for x in self.scan],
                               xbcfg=self.xbcfg, xb=bool(self.xb)),
                          sort_keys=True, default=str)


# ---- the SDR ----


class FakeAirSdr(bench.FakeSdr):
    """An RTL-SDR on the fake air: every carrier keyed on the tuned frequency (the radios' key-ups and the
    repeater's output) with its tone (CTCSS or DCS, bench.fake_dev), the end tone at its unkey and the CW ID's 700 Hz
    elements, FM onto the carrier as bench.FakeSdr renders a bare one."""

    def __init__(self, air, clock, seed=11, radios=()):
        super().__init__(None, clock, seed)
        self.air, self.r, self.radios = air, self, list(radios)

    @property
    def carriers(self):
        out = []
        for c in self.air.log:
            end = c["end"] if c["end"] is not None else self.t0 + self.seconds + 1.0
            sig = dict(kind="carrier", tone=c["tone"], t_off=c.get("t_off", end), endtone=c.get("endtone", "Off"),
                       mode="FM", cw=c.get("cw"))
            out.append((c["start"], end, c["freq"] / 1e5, "carrier", sig))
        return out

    def finish(self):
        import numpy as np
        import sdrtx
        for r in self.radios:
            r.advance()  # each fake radio's world up to now: the repeater's pass ends as it would have
        fs = sdrtx.FS
        cs = [c for c in self.carriers
              if c[1] > self.t0 and c[0] < self.t0 + self.seconds and abs(c[2] - self.mhz) < 0.01]
        n = int(self.seconds * fs)
        fd = fs // sdrtx.DEC
        td = self.t0 + np.arange(n // sdrtx.DEC + 1) / fd
        dev = np.zeros(len(td))
        on = np.zeros(len(td), bool)
        for c in cs:
            m = (td >= c[0]) & (td < c[1])
            on |= m
            if m.any():
                dev[m] += bench.fake_dev(c[4], td[m] - c[0], c[4]["t_off"] - c[0], self.rng)
            for a, b in c[4].get("cw") or ():
                k = (td >= a) & (td < b) & m
                dev[k] += 1000.0 * np.sin(2 * np.pi * 700.0 * (td[k] - a))
        raw = np.empty(2 * n, np.uint8)
        chunk, ph = 1 << 20, 0.0
        for a in range(0, n, chunk):
            idx = np.arange(a, min(a + chunk, n))
            j = idx // sdrtx.DEC
            d = np.where(on[j], dev[j] + 700.0, 0.0) - sdrtx.OFF
            p = ph + 2 * np.pi * np.cumsum(d) / fs
            ph = p[-1] % (2 * np.pi)
            x = np.where(on[j], 60.0, 0.0) * np.exp(1j * p) \
                + self.rng.normal(0, 1.5, len(idx)) + 1j * self.rng.normal(0, 1.5, len(idx))
            raw[2 * a: 2 * idx[-1] + 2: 2] = np.clip(np.round(x.real + 127.5), 0, 255)
            raw[2 * a + 1: 2 * idx[-1] + 2: 2] = np.clip(np.round(x.imag + 127.5), 0, 255)
        raw.tofile(self.path)
        return raw


# ---- the selftest ----


def world(bugs_rx=(), bugs_tx=(), ble_rx=False, build_tx="tx-dry", build_rx="tx-dry", hold_tx=False):
    clk = bench.FakeClock()
    air = Air(clk)
    tx = FakeFerrite("black", clk, air, build=build_tx, bugs=bugs_tx, hold_answers=hold_tx)
    rx = FakeFerrite("clear", clk, air, spec="ble:fake" if ble_rx else None, build=build_rx, ble=ble_rx, bugs=bugs_rx)
    rx.slots[2]["freq"] = f10(100.5)  # C on a broadcast station (the audio probe)
    rx.vfo[2]["freq"] = f10(100.5)
    rx.set["0 8 6 1"] = "VA3FAK"  # the repeater's ID (XB-12) needs the callsign set on the radio
    return clk, air, tx, rx


def fake_run(tr, a, clk, air, tx, rx, quiet, **kw):
    """A Run on the fake pair with the fake SDR on the air and the captures in a temporary directory."""
    run = tr.Run(args(tr, **kw), dict(tx=tx, rx=rx), clk, threaded=False, log=quiet)
    run.sdr = FakeAirSdr(air, clk, radios=(tx, rx))
    run.cap_dir = tempfile.mkdtemp(prefix="tworadio-selftest-caps-")
    return run


def args(tr, **kw):
    a = tr.main_args([])
    a.commit, a.build = "selftest", "selftest"
    a.journal = os.path.join(tempfile.mkdtemp(prefix="tworadio-selftest-journal-"), "pending.json")
    for k, v in kw.items():
        setattr(a, k, v)
    return a


def selftest(a, tr):
    ok = True
    lines = []

    def check(what, cond, got=""):
        nonlocal ok
        ok &= bool(cond)
        lines.append(f"  [{'pass' if cond else 'FAIL'}] {what}{': ' + str(got) if got != '' else ''}")
        tr.say(lines[-1])

    quiet = lambda *_: None  # noqa: E731
    tests = tr.load_tests()

    # 1. The whole run on a fake pair: every test passes, everything is put back, the duty limit held.
    clk, air, tx, rx = world()
    before = (tx.state(), rx.state())
    run = fake_run(tr, a, clk, air, tx, rx, quiet)
    results, left = run.run(tests)
    long_names = {t.name for t in tests if t.long}
    bad = [f"{r['name']}: {r['result']}: {r.get('why')}" for r in results
           if r["result"] != "pass" and not (r["name"] in long_names and r["result"] == "skip")]
    check("every two-radio test passes on the fake pair (the long ones skip without --long)", not bad,
          "; ".join(bad)[:900])
    check("nothing left changed (the run's own check)", not left, left)
    check("both fakes as they were (slots, settings, channels, zones, mute)", (tx.state(), rx.state()) == before,
          "" if (tx.state(), rx.state()) == before else f"{tx.state()[:300]} / {rx.state()[:300]}")
    check("the air is quiet at the end (no carrier keyed)", not air.keyed(), air.keyed())
    for name, f in (("tx", tx), ("rx", rx)):
        ons, offs = f.log.count("KEY on"), f.log.count("KEY off")
        check(f"{name}: every KEY on has a KEY off after it", ons <= offs and (not ons or f.log[::-1].index("KEY off")
                                                                              < f.log[::-1].index("KEY on")), (ons, offs))
    d = run.duty["tx"]
    s = d.summary()
    longest = max((e["counted_s"] for e in d.log), default=0)
    check("duty: no key-up over 5 s with its tail", longest <= 5.0 + 1e-6, f"{longest:.2f} s")
    rests = [b["t_on"] - (a_["t_on"] + a_["counted_s"] * 3) for a_, b in zip(d.log, d.log[1:]) if a_["counted_s"] > 0]
    check("duty: a rest of 2x after each key-up", all(r >= -0.05 for r in rests), f"min {min(rests):.2f} s")
    check("duty: at most 25 % of any 5 min keyed", s["peak_window_frac"] <= 0.25 + 1e-6, f"{s['peak_window_frac']:.0%}")
    check("the transmitting radio keyed only 145.495 and 446.475",
          all(e["purpose"] for e in d.log) and all(g["ok"] for g in run.keyers["tx"].gatelog),
          [g for g in run.keyers["tx"].gatelog if not g["ok"]])
    rxk = [k for k in run.keyers["rx"].keyups if k.kind in ("probe", "burst")]
    check("the receiving radio keyed twice: the busy probe (refused) and SCAN-19's PTT",
          [(k.kind, k.accepted) for k in rxk] == [("probe", False), ("burst", True)], [(k.kind, k.accepted) for k in rxk])
    relays = [k for k in run.keyers["rx"].keyups if k.kind == "relay"]
    dr = run.duty["rx"]
    check("crossband: each pass the repeater made is on its own duty log (kind relay), within 5 s with its tail",
          len(relays) >= 8 and all(e["counted_s"] <= 5.0 + 1e-6 for e in dr.log if e["kind"] == "relay"),
          (len(relays), max((e["counted_s"] for e in dr.log), default=0)))
    check("crossband: the repeater's duty at most 25 % of any 5 min", dr.summary()["peak_window_frac"] <= 0.25 + 1e-6,
          f"{dr.summary()['peak_window_frac']:.0%}")
    check("crossband: no repeater left running, its grid put back",
          not rx.xb and rx.xbcfg == json.loads(before[1])["xbcfg"], (bool(rx.xb), rx.xbcfg))
    caps = [c for r in results for c in r.get("captures", [])]
    check("the SDR captures of passing tests are removed", not caps and not os.listdir(run.cap_dir), caps[:3])

    # 1b. The long ones (--long): XB-9's timeout and XB-12's ID, the duty limit's timeout exception on both radios.
    clk, air, tx, rx = world()
    run = fake_run(tr, a, clk, air, tx, rx, quiet, long=True)
    res, left = run.run([t for t in tests if t.long])
    check("--long: the long crossband tests pass", all(r["result"] == "pass" for r in res) and not left,
          [(r["name"], r["result"], r.get("why")) for r in res])
    kinds = sorted({e["kind"] for d in run.duty.values() for e in d.log if e["counted_s"] > 5.0})
    check("--long: only the timeout exception passes 5 s", kinds in ([], ["timeout"]), kinds)
    plan_long = tr.plan_lines(tests, args(tr, long=True, sdr=True))
    check("--dry-plan --long: the long tests are planned", any("a_pass_times_out" in x for x in plan_long), "")

    # 2. Results in uitest's shape; never recorded (a selftest), and what record_results would write for a radio run.
    started = __import__("datetime").datetime.now()
    summary = tr.summary_of(args(tr), run, results, left, started, True)
    summary["port"] = "fake"
    tmp = a.results if a.results != tr.RESULTS else tempfile.mkdtemp(prefix="tworadio-selftest-")
    sess = tr.uitest.new_session(tmp, "tworadio-selftest")
    path, page = tr.write_results(summary, sess, [d.report()])
    import record_results
    say_rec = record_results.record_paths([path], a.docs, dry=True)
    check("record_results skips the selftest's results", any("skipped" in x and "selftest" in x for x in say_rec), say_rec)
    radio_like = dict(summary, sim=False, port="tx=/dev/x rx=ble:y")
    rows = record_results.rows_of(radio_like, "20261014-120000", record_results.split_rules(a.docs),
                                  record_results.tworadio_tags(), [])
    marks = {rid: row.split(" | ")[2] for rid, _, _, row in rows}
    check("a radio run's rows: ✓ for whole rules, ◐ for part (WATCH-3.2, MODE-5)",
          marks.get("WATCH-3.2") == "◐" and marks.get("MODE-5") == "◐" and marks.get("HOME-6") == "✓"
          and all(r.split(" | ")[3].startswith("tworadio ") for _, _, _, r in rows), marks)
    check("the results page is written", os.path.getsize(page) > 1000, page)

    # 3. Bugs the tests must catch (each a test that must fail on a broken receiver).
    for bug, name, side in (("dcs_either", "dcs_opens_only_on_its_polarity", "rx"),
                            ("follow", "watched_slot_plays_and_active_stays", "rx"),
                            ("no_busy", "busy_lockout_refuses_ptt", "rx"),
                            ("hint_sticks", "tone_hint_clears", "rx"),
                            ("resume_3s", "resume_rules", "rx"),
                            ("xb_wrong_tone", "relays_2m_to_70cm", "rx"),
                            ("xb_no_hang", "hang_and_carrier_hold_on_air", "rx"),
                            ("xb_key_ignored", "a_key_or_xb_stop_cuts_a_pass", "rx"),
                            ("xb_on_at_boot", "never_on_after_a_restart", "rx")):
        clk, air, tx, rx = world(bugs_rx=(bug,))
        run = fake_run(tr, a, clk, air, tx, rx, quiet)
        res, left = run.run([t for t in tests if t.name == name])
        r = res[0]
        extra = ""
        if bug == "no_busy":
            k = run.keyers["rx"].keyups[0]
            extra = f"; the probe keyed and was unkeyed at once: {k.accepted and k.t_off is not None}"
        check(f"bug {bug}: {name} fails", r["result"] == "fail" and not air.keyed(), f"{r['result']}: {r.get('why')}{extra}")

    # 3a. A run killed after a test changed a setting (pkill: no restore ran): the next run puts it back before
    # its snapshot, so the radio's own value is restored, not the test's (Oct 15, 2026: watch left Off).
    clk, air, tx, rx = world()
    jr = os.path.join(tempfile.mkdtemp(prefix="tworadio-selftest-journal-"), "pending.json")
    run = fake_run(tr, a, clk, air, tx, rx, quiet, journal=jr)
    was = rx.set.get("0 1 1", DEFAULTS["0 1 1"][1])
    run.ctx().rx.set("0 1 1", "Off")  # the test's change; then the process dies
    check("killed: the change is on disk", os.path.exists(jr) and "Off" == rx.set.get("0 1 1"), (jr, rx.set.get("0 1 1")))
    run2 = fake_run(tr, a, clk, air, tx, rx, quiet, journal=jr)
    run2.run([])
    check("killed: the next run puts it back before its snapshot, and the journal goes",
          rx.set.get("0 1 1", DEFAULTS["0 1 1"][1]) == was and not os.path.exists(jr), (rx.set.get("0 1 1"), was))

    # 3b. A set-up that didn't take: the repeater's sides left on the radio's own slots. The test fails at its
    # set-up's readback, before anything keys.
    clk, air, tx, rx = world(bugs_rx=("sides_ignored",))
    run = fake_run(tr, a, clk, air, tx, rx, quiet)
    res, left = run.run([t for t in tests if t.name == "relays_2m_to_70cm"])
    r = res[0]
    check("fault: the repeater's sides don't take: the test fails before any key-up",
          r["result"] == "fail" and "side 1" in r.get("why", "") and "KEY on" not in tx.log + rx.log,
          f"{r['result']}: {r.get('why')}")

    # 3c. A Bluetooth time-out mid-run: the run stops at once (KEY off to both, no later test), and is not recorded.
    pick = [t for t in tests if t.name in ("receive_shows_rx_and_meter", "busy_lockout_refuses_ptt",
                                           "receiving_wakes_the_screen")]
    clk, air, tx, rx = world(ble_rx=True)
    fake_run(tr, a, clk, air, tx, rx, quiet).run(pick[:1])
    n1 = len(rx.log)
    clk, air, tx, rx = world(ble_rx=True)
    run = fake_run(tr, a, clk, air, tx, rx, quiet)
    rx.fail_after = n1 + 8  # in the second test
    res, left = run.run(pick)
    check("fault: a Bluetooth time-out mid-run stops the run in that test (no later test run)",
          len(res) == 2 and res[-1]["result"] == "error" and "link lost" in res[-1].get("why", "")
          and (run.stopped or "").startswith("link lost") and run.link_lost == "rx",
          (len(res), [(x["name"], x["result"], x.get("why")) for x in res], run.stopped))
    check("fault: after the time-out, KEY off to both and the air quiet",
          not air.keyed() and not tx.keyed and "KEY off" in tx.log[len(tx.log) - tx.log[::-1].index("KEY on"):],
          (air.keyed(), tx.log[-3:]))
    check("fault: what couldn't be put back is reported", any("rx: not put back" in x for x in left), left)
    summ = tr.summary_of(args(tr), run, res, left, __import__("datetime").datetime.now(), False)
    summ.update(port="tx=/dev/x rx=ble:y", sim=False)
    check("fault: a run stopped on a link error is never recorded", not record_results.radio_run(summ)[0],
          record_results.radio_run(summ))

    # 4. The gates: nothing keyed, nothing changed.
    clk, air, tx, rx = world(build_rx="tx")
    run = tr.Run(args(tr), dict(tx=tx, rx=rx), clk, threaded=False, log=quiet)
    try:
        run.run(tests)
        check("gate: a radio on the tx build (full drive) is refused before anything", False, "it ran")
    except tr.SafetyError as e:
        check("gate: a radio on the tx build (full drive) is refused before anything",
              "LINK-35" in str(e) and not any(x.startswith(("CMD set", "CMD slot A", "KEY")) for x in tx.log + rx.log),
              str(e)[:120])
    for what, setup, want in (
            ("145.500 MHz (not approved)", lambda f: f.slots[0].update(freq=f10(145.5)), "frequency approved"),
            ("a +0.6 MHz shift (TX 146.095)", lambda f: f.slots[0].update(freq=f10(145.495), offset=60_000),
             "frequency approved"),
            ("power high", lambda f: f.slots[0].update(freq=f10(145.495), power="high"), "power low"),
            ("not a bench build", lambda f: setattr(f, "bench", False), "tx-dry bench build")):
        clk, air, tx, rx = world()
        setup(tx)
        k = tr.Keyer(tx, "tx", bench.DutyLimiter(None, clk, out=quiet), clk, quiet, threaded=False)
        try:
            with k.keyup(2, "gate test"):
                pass
            check(f"gate: {what} refused", False, "it keyed")
        except tr.SafetyError as e:
            check(f"gate: {what} refused, KEY on never sent", want in str(e) and "KEY on" not in tx.log, str(e)[:100])
    # TX-48: a tx build reporting Dry TX (tx+dry, no bench bit) keys; turned off on the radio, it is refused.
    clk, air, tx, rx = world(build_tx="tx+dry")
    tx.slots[0]["freq"] = f10(145.495)
    k = tr.Keyer(tx, "tx", bench.DutyLimiter(None, clk, out=quiet), clk, quiet, threaded=False)
    try:
        with k.keyup(2, "dry tx"):
            pass
        check("gate: a tx build reporting Dry TX (tx+dry) is accepted and keyed", "KEY on" in tx.log, tx.log[-3:])
    except tr.SafetyError as e:
        check("gate: a tx build reporting Dry TX (tx+dry) is accepted and keyed", False, str(e)[:100])
    tx.dry_tx = False
    try:
        with k.keyup(2, "dry tx off"):
            pass
        check("gate: Dry TX turned off on the radio is refused", False, "it keyed")
    except tr.SafetyError as e:
        check("gate: Dry TX turned off on the radio is refused, KEY on never sent again",
              "Dry TX still on" in str(e) and tx.log.count("KEY on") == 1, str(e)[:100])
    clk, air, tx, rx = world(build_rx="tx+dry")
    run = tr.Run(args(tr), dict(tx=tx, rx=rx), clk, threaded=False, log=quiet)
    try:
        run.verify()
        check("gate: verify accepts a tx-dry radio and a tx+dry one (TX-48)", True)
    except tr.SafetyError as e:
        check("gate: verify accepts a tx-dry radio and a tx+dry one (TX-48)", False, str(e)[:100])
    clk, air, tx, rx = world(hold_tx=True)
    tx.slots[0]["freq"] = f10(145.495)
    k = tr.Keyer(tx, "tx", bench.DutyLimiter(None, clk, out=quiet), clk, quiet, threaded=False)
    try:
        with k.keyup(2, "unanswered"):
            pass
        check("gate: KEY on unanswered on the cable stops the run", False, "went on")
    except tr.SafetyError as e:
        check("gate: KEY on unanswered on the cable stops the run, KEY off sent",
              "wasn't answered" in str(e) and tx.log[-1] == "KEY off", str(e)[:100])
    clk, air, tx, rx = world()
    tx.slots[0]["freq"] = f10(145.495)
    k = tr.Keyer(tx, "tx", bench.DutyLimiter(None, clk, out=quiet), clk, quiet, threaded=False)
    try:
        with k.keyup(6, "too long"):
            pass
        check("duty: a 6 s key-up refused", False, "keyed")
    except bench.BenchError as e:
        check("duty: a 6 s key-up refused before KEY on", "burst limit" in str(e) and "KEY on" not in tx.log, str(e)[:90])
    # The key-up's own end: the test body overrunning its key-up doesn't keep it keyed.
    with k.keyup(2, "overrun") as ku:
        c = tr.Ctx.__new__(tr.Ctx)
        c.run = type("R", (), {"keyers": {"tx": k}})()
        c.clock = clk
        tr.Ctx.sleep(c, 6.0)
        check("a key-up ends at its deadline whatever the test does (KEY off at 2 s)",
              ku.t_off is not None and ku.t_off - ku.t_on <= 2.11 and not air.keyed(),
              None if ku.t_off is None else f"{ku.t_off - ku.t_on:.2f} s")

    # 5. The receiving radio on Bluetooth: the fast tests skip, the rest pass; --swap's roles.
    clk, air, tx, rx = world(ble_rx=True)
    run = tr.Run(args(tr), dict(tx=tx, rx=rx), clk, threaded=False, log=quiet)
    res, left = run.run([t for t in tests if t.module == "watch"])
    by = {r["name"]: r["result"] for r in res}
    check("RX on Bluetooth: the fast test skips (BCR-24), the others pass",
          by.get("broadcast_mutes_at_once_and_returns") == "skip"
          and all(v == "pass" for n, v in by.items() if n != "broadcast_mutes_at_once_and_returns"), by)
    sw = tr.main_args(["--tx", "A", "--rx", "B", "--swap"])
    check("--swap: the --rx radio keys, the --tx radio receives", tr.roles(sw) == ("B", "A"), tr.roles(sw))

    # 6. The plan.
    plan = tr.plan_lines(tests, args(tr))
    check("--dry-plan: the estimated keyed time and the duty report", any("estimated:" in x and "keyed" in x for x in plan)
          and any(x.startswith("TX duty:") for x in plan), plan[-2:])
    tr.say(f"selftest: {'all pass' if ok else 'FAILED'}")
    return ok


# Every setting the tests set, with each value they use (checked against a simulator's terminal).
SETTINGS_USED = (("0 1 1", "Off"), ("0 1 1", "A+B"),
                 ("0 1 2", "Carrier drop"), ("0 1 2", "3 s"), ("0 1 2", "5 s"), ("0 1 2", "Stop on hit"),
                 ("0 1 6 3", "Off"), ("0 1 6 3", "On"), ("0 1 3", "Off"),
                 ("0 7 2", "Watch"), ("0 7 2", "Single + scope"), ("0 7 4 1", "5 s"), ("0 7 4 4", "Keys and RX"),
                 ("0 7 4 4", "Keys, RX and TX"), ("0 7 4 4", "Keys"), ("0 7 7 1", "Level 1"), ("0 8 4", "Off"))


def selftest_sim(a, tr):
    """Two simulators: the gates refuse them (they say tx, not tx-dry: nothing keyed), and on the receiving one a
    test's fixture (a zone, channels, V/M and a channel number, settings) is made and removed, the slots put back:
    `dump` the same before and after."""
    import subprocess
    ok = True

    def check(what, cond, got=""):
        nonlocal ok
        ok &= bool(cond)
        tr.say(f"  [{'pass' if cond else 'FAIL'}] {what}{': ' + str(got) if got != '' else ''}")

    procs = []
    try:
        p1, spec1, _bt = tr.uitest.start_sim(600)  # the cable port; its Bluetooth one unused
        procs.append(p1)
        exe = os.path.join(tr.FERRITE, "target", "debug", "ferrite-sim")
        port2 = tr.uitest.free_port()
        p2 = subprocess.Popen([exe, "--term-headless", "600", "--term-tcp", str(port2)], stdout=subprocess.DEVNULL,
                              stderr=subprocess.DEVNULL)
        procs.append(p2)
        import time as _t
        _t.sleep(3.0)
        spec2 = f"tcp:127.0.0.1:{port2}"
        tx, rx = tr.LinkRadio(spec1), tr.LinkRadio(spec2)
        clk = bench.Clock()
        run = tr.Run(args(tr), dict(tx=tx, rx=rx), clk, threaded=True, log=lambda *_: None)
        try:
            run.run(tr.load_tests())
            check("the gates refuse the simulators (they say tx, not tx-dry)", False, "it ran")
        except tr.SafetyError as e:
            check("the gates refuse the simulators (LINK-35: not tx-dry): nothing changed or keyed", "LINK-35" in str(e),
                  str(e)[:140])
        dump0 = rx.cmd("dump")
        run.snapshot()
        c = run.ctx()
        from tworadio_tests import core as trc
        from tworadio_tests import scan as trs
        res = dict(ok=True)

        def body(t):
            trc.rx_quiet(t, watch="A+B")
            ch = trc.Channels(t, trs.ZONE_CHANS, loose=((986, "TR BUSY", trc.VHF, "busy=on tone=100.0"),))
            res["busy"] = t.rx.cmd("ch 986")
            for path, v in (("0 1 2", "Carrier drop"), ("0 1 6 3", "On")):
                t.rx.set(path, v)
            u = ch.enter("A", 981)
            res["entered"] = u.active.get("name")
            res["txf"] = sorted(ch.tx_freqs())
            try:
                trs.start_scan(t, ch)
                res["scanning"] = True
            except tr.Skip as e:
                res["scanning"] = str(e)
            trs.stop_scan(t)
            res["stopped"] = t.rx.raw().cards["A"].get("scan") == 0
            # the terminal words the tests use, as this firmware takes them
            for line in ("ch 981 skip on", f'zone {ch.zone} "{ch.ZONE}" resume=stop', "slot B vfo 145.495 fm low "
                         "tone D023I offset off", "slot B rxtone 107.2", "slot B txtone 88.5", "slot B vfo 145.500"):
                ok, out = t.rx.cmd_err(line)
                if not ok:
                    res.setdefault("refused", []).append(f"{line}: {out}")
            ok, out = t.tx.cmd_err("slot A vfo 446.475 fm low txtone D023N rxtone off offset off")
            if not ok:
                res.setdefault("refused", []).append(f"tx slot: {out}")
            for path, v in SETTINGS_USED:
                try:
                    t.rx.set(path, v)
                    got = t.rx.get(path)
                    if got != v:
                        res.setdefault("refused", []).append(f"{path}: set {v!r}, reads {got!r}")
                except tr.Fail as e:
                    res.setdefault("refused", []).append(str(e))
            trc.rx_broadcast(t, "C", required=False)  # the simulator's own signals on A and B keep it muted
            res["broadcast"] = t.rx.raw().cards["C"].get("kind")
            res["defaults"] = [t.rx.get(p) for p in ("0 1 6 4", "0 1 6 1")]
            s = trc.status_page(t)
            res["status"] = (s.id, (s.row("Watch") or {}).get("value"))
            t.rx.home()

        try:
            body(c)
        finally:
            notes = []
            for fn in reversed(c.defers):
                fn()
            run.restore(c, notes)
        check("fixture on the simulator: slot A on TR 2M by V/M and its number", res.get("entered") == "TR 2M",
              res.get("entered"))
        check("the zone's transmit frequencies: 145.495 and 446.475 only (receive-only ones left out)",
              res.get("txf") == [14549500, 44647500], res.get("txf"))
        check("SIDE2 scans the test zone, home with it running; SIDE2 again stops it",
              res.get("scanning") is True and res.get("stopped"), res)
        check("a busy-lockout channel with T 100.0 made by `ch`", "busy=" in res.get("busy", "") and "tone=100.0" in
              res.get("busy", ""), res.get("busy"))
        check("every setting value and terminal line the tests use is taken (and reads back as given)",
              not res.get("refused"), res.get("refused"))
        check("slot C on 100.5 MHz is the broadcast slot (the audio probe)", res.get("broadcast") == "broadcast",
              res.get("broadcast"))
        check("return to and pause time at the defaults the tests expect",
              res.get("defaults") == ["Start", "1 min"], res.get("defaults"))
        check("MENU 0 0 is the Status page with its Watch row", res.get("status", ("",))[0] == "Status"
              and res["status"][1], res.get("status"))
        dump1 = rx.cmd("dump")
        diff = tr.uitest.dump_diff(dump0, dump1)
        check("the simulator's dump is the same after (channels, zone, settings, slots put back)", not diff, diff[:6])
        check("no restore notes", not notes, notes)
        # The crossband fixture against the simulator's own firmware code: the start page by MENU 0 3 4, its grid by
        # the cells and the knob, `xb start` and its readback, SIDE1 held, a remote key stopping it, the grid put back.
        from tworadio_tests import crossband as trx
        dump0 = rx.cmd("dump")
        c = run.ctx()
        xr = {}

        def xbody(t):
            trx.setup(t, hang="0.0 s", timeout="30 s", ident="Off", gate="Off (carrier)")
            xr["grid"] = trx.cells(t.rx.ui())
            u = trx.start(t)
            xr["run"] = trx.xb(u)
            xr["screen"], xr["hints"] = u.id, u.hints
            xr["xb"] = t.rx.cmd("xb").strip()
            xr["listen"] = trx.xb(t.rx.keys("SIDE1:down")).get("listen")
            xr["unlisten"] = trx.xb(t.rx.keys("SIDE1:up")).get("listen")
            u = t.rx.keys("5")
            xr["stopped"] = (trx.xb(u).get("running"), u.id)
            u = trx.start(t, keys=True)
            xr["by_keys"] = trx.xb(u).get("by")
            trx.stop(t)

        try:
            xbody(c)
        except (tr.Fail, tr.Skip) as e:
            xr["error"] = str(e)
        finally:
            notes = []
            for fn in reversed(c.defers):
                fn()
            run.restore(c, notes)
        check("crossband on the simulator: the grid set by keys and the knob", not xr.get("error") and
              xr.get("grid", {}).get("Hang") == "0.0 s" and xr["grid"].get("Timeout") == "30 s", xr)
        check("crossband on the simulator: xb start, its screen and readback (by cmd:cable), xb says running",
              xr.get("run", {}).get("running") == 1 and xr["run"].get("by") == "cmd:cable" and xr.get("screen") ==
              "Crossband" and ("Any key", "Stop") in xr.get("hints", []) and xr.get("xb", "").startswith(
                  "Crossband: running"), xr)
        check("crossband on the simulator: SIDE1 held listens; a remote key stops it; # starts it by keys",
              (xr.get("listen"), xr.get("unlisten"), xr.get("stopped"), xr.get("by_keys")) ==
              (1, 0, (0, "Crossband"), "keys:cable"), xr)
        diff = tr.uitest.dump_diff(dump0, rx.cmd("dump"))
        check("crossband on the simulator: the grid and slots put back (dump the same), no notes", not diff and
              not notes, (diff[:6], notes))
        tx.close()
        rx.close()
    finally:
        for p in procs:
            with contextlib.suppress(Exception):
                p.terminate()
    tr.say(f"selftest --sim: {'all pass' if ok else 'FAILED'}")
    return ok


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.parse_args()
    raise SystemExit("run it as tools/tworadio.py --selftest")
