"""tools/tworadio.py's selftest: two fake Ferrite radios on a fake air, on a virtual clock (bench.py's FakeClock).

A fake radio answers as the harness talks to a real one (LinkRadio's methods: terminal lines, remote keys, the UI
readback in the radio's own line format, SLOT, KEY's remote PTT), and models only what the two-radio tests look at,
as the manual's rules say the firmware does it: squelch on a carrier from the other radio, tone squelch and the tone
hint (TSRCH-12 to TSRCH-15), watch and quick mute with the speaker, the broadcast slot's FM/muted, busy lockout, the
backlight and its wake, Battery Save's hold and cycle on REG_30, a channel scan of a zone with the resume rules,
scan inputs, \\* and # skips and PTT on a hit, the zones page's delete. `bugs` breaks one thing on purpose, so the
selftest sees the test that must catch it fail. The air carries each keyed radio's carrier on its transmit
frequency with its tone, until 400 ms after KEY off (TX-47's hold) or at once on a lost keepalive.

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
    "0 1 1": ("Watch", "A+B"), "0 1 2 2": ("Pause time", "1 min"), "0 1 4": ("Scan target", "Current zone"),
    "0 1 5": ("Resume", "5 s"), "0 1 6 3": ("Scan inputs", "Off"), "0 1 6 4": ("Return to", "Start"),
    "0 1 6 5": ("Priority check", "Every 2 s"), "0 7 2": ("Home layout", "Watch"), "0 7 4 1": ("Timeout", "1 min"),
    "0 7 4 4": ("Wake screen on", "Keys and RX"), "0 7 7": ("Battery Save", "Off"), "0 8 5 4": ("End tone", "Off"),
}


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
        self.clock, self.tx = clock, {}

    def key(self, who, freq, tone):
        self.tx[who] = dict(freq=freq, tone=tone, start=self.clock.time(), end=None, id=object())

    def unkey(self, who, hold):
        c = self.tx.get(who)
        if c and c["end"] is None:
            c["end"] = self.clock.time() + hold

    def carrier(self, freq, me, now):
        for who, c in self.tx.items():
            if who != me and c["freq"] == freq and c["start"] <= now and (c["end"] is None or now < c["end"]):
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
        self.build, self.bench, self.version = build, bench_bit, "v0.1.0-fake"
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
        g = self.set["0 1 5"]
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
        self.scan[i] = dict(pos=pos, idx=idx, state="walk", t_next=self.t + 0.05, stop=None, drop=None, start=n0,
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
        sc["state"], sc["t_next"] = "walk", self.t + 0.05

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
               f"remote={int(self.keyed is not None)}",
               f"bar lock=0 scan={int(any(self.scan))} watch=0 inbox=0 alert=0 gps=off bt=0 battery=80 low=0 cutoff=0 "
               "clock=- utc=0"]
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

    # ---- keys ----

    def keys(self, seq, long=False):
        for word in seq.split():
            name, _, how = word.partition(":")
            self.press(name, long=(how or ("long" if long else "short")) == "long")

    def press(self, name, long=False):
        self._req(f"KEY {name}{':long' if long else ''}")
        if self.keyed:
            raise fl.RadioError(9, b"transmitting: remote keys wait for the unkey")
        self.last_key = self.t
        name = name.upper()
        sid, data = self.stack[-1]
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
            self.stack.append(("Advanced", None))
        elif sid == "Advanced" and name == "0":
            self.stack.append(("Status", None))
        elif sid == "Advanced" and name == "2":
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
            sc["state"], sc["t_next"] = "walk", self.t + 0.05
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
                self.paused_until = self.t + (secs_of(self.set["0 1 2 2"]) or 60)
                self.toast = ("Watch paused", self.set["0 1 2 2"], self.t + 2)
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
            if p in ("0 1 2 2", "0 1 4", "0 1 6 4"):  # as the firmware (Oct 14, 2026)
                self.err(f"{DEFAULTS[p][0]} can't be set yet: it keeps its default until its feature lands")
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
        if w[0] == "bk" and len(w) == 3:
            return f"  bk{w[1]} REG_{w[2].upper()} = 0x{self.reg30():04X}   ...\n"
        self.err(f"{w[0]}: no such command")

    def reg30(self):
        lvl = self.set["0 7 7"]
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
                self.err(f'ch {n}: no such channel; make it with ch {n} "Name" 146.52')
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
        if self.build not in ("tx", "tx-dry"):
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
                               zones=self.zones, mute=sorted(self.mute), scan=[bool(x) for x in self.scan]),
                          sort_keys=True, default=str)


# ---- the selftest ----


def world(bugs_rx=(), bugs_tx=(), ble_rx=False, build_tx="tx-dry", build_rx="tx-dry", hold_tx=False):
    clk = bench.FakeClock()
    air = Air(clk)
    tx = FakeFerrite("black", clk, air, build=build_tx, bugs=bugs_tx, hold_answers=hold_tx)
    rx = FakeFerrite("clear", clk, air, spec="ble:fake" if ble_rx else None, build=build_rx, ble=ble_rx, bugs=bugs_rx)
    rx.slots[2]["freq"] = f10(100.5)  # C on a broadcast station (the audio probe)
    rx.vfo[2]["freq"] = f10(100.5)
    return clk, air, tx, rx


def args(tr, **kw):
    a = tr.main_args([])
    a.commit, a.build = "selftest", "selftest"
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
    run = tr.Run(args(tr), dict(tx=tx, rx=rx), clk, threaded=False, log=quiet)
    results, left = run.run(tests)
    bad = [f"{r['name']}: {r['result']}: {r.get('why')}" for r in results if r["result"] != "pass"]
    check("every two-radio test passes on the fake pair", not bad, "; ".join(bad)[:900])
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
    rxk = [k for k in run.keyers["rx"].keyups]
    check("the receiving radio keyed twice: the busy probe (refused) and SCAN-19's PTT",
          [(k.kind, k.accepted) for k in rxk] == [("probe", False), ("burst", True)], [(k.kind, k.accepted) for k in rxk])

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
                            ("resume_3s", "resume_rules", "rx")):
        clk, air, tx, rx = world(bugs_rx=(bug,))
        run = tr.Run(args(tr), dict(tx=tx, rx=rx), clk, threaded=False, log=quiet)
        res, left = run.run([t for t in tests if t.name == name])
        r = res[0]
        extra = ""
        if bug == "no_busy":
            k = run.keyers["rx"].keyups[0]
            extra = f"; the probe keyed and was unkeyed at once: {k.accepted and k.t_off is not None}"
        check(f"bug {bug}: {name} fails", r["result"] == "fail" and not air.keyed(), f"{r['result']}: {r.get('why')}{extra}")

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
                 ("0 1 5", "Carrier drop"), ("0 1 5", "3 s"), ("0 1 5", "5 s"), ("0 1 5", "Stop on hit"),
                 ("0 1 6 3", "Off"), ("0 1 6 3", "On"), ("0 1 6 5", "Off"),
                 ("0 7 2", "Watch"), ("0 7 2", "Single + scope"), ("0 7 4 1", "5 s"), ("0 7 4 4", "Keys and RX"),
                 ("0 7 4 4", "Keys, RX and TX"), ("0 7 4 4", "Keys"), ("0 7 7", "Level 1"), ("0 8 5 4", "Off"))


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
        p1, spec1 = tr.uitest.start_sim(600)
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
            for path, v in (("0 1 5", "Carrier drop"), ("0 1 6 3", "On")):
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
            res["defaults"] = [t.rx.get(p) for p in ("0 1 4", "0 1 6 4", "0 1 2 2")]
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
        check("scan target, return to and pause time at the defaults the tests expect",
              res.get("defaults") == ["Current zone", "Start", "1 min"], res.get("defaults"))
        check("MENU 0 0 is the Status page with its Watch row", res.get("status", ("",))[0] == "Status"
              and res["status"][1], res.get("status"))
        dump1 = rx.cmd("dump")
        diff = tr.uitest.dump_diff(dump0, dump1)
        check("the simulator's dump is the same after (channels, zone, settings, slots put back)", not diff, diff[:6])
        check("no restore notes", not notes, notes)
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
