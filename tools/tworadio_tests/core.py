"""The two-radio test framework (tools/tworadio.py runs it): the registry, the shared set-ups and the channel fixture.

A test is a function taking the context `t` (tworadio.Ctx), tagged with the manual's sub-rule IDs it checks
(decision 160), and with what it keys, so `--dry-plan` can say how long the run keys and takes:

    from .core import test, VHF

    @test("HOME-6", keyups=(3,), idle=6)
    def receive_shows_rx(t):
        rx_vfo(t, "A", VHF)
        tx_on(t, VHF)
        with t.key(3, "carrier on A"):
            u, _ = t.poll(t.rx, lambda u: u.cards["A"]["rx"] == 1, 2.0)
            ...

`t.tx` keys (only through `t.key`, every key-up past the gates and bench.py's DutyLimiter), `t.rx` receives; both are
read back (LINK-19) and driven by remote keys and terminal lines (LINK-34). `part=("WATCH-3.2",)` marks rules the test
checks only part of (recorded ◐ with "covers part of the rule"; the entry stays by hand for the rest). `fast=True`:
the test times things to a few tenths of a second, so the receiving radio must be on the cable (or the simulator):
over Bluetooth (a readback about 1 s) it is skipped, and `--swap` runs it the other way round. `keyups` lists the
planned key-ups (seconds each) and `idle` the seconds of waiting without keying, for the plan only.

Every setting a test changes goes through `side.set()` (the runner puts it back); slots are put back from the
snapshot; channels and a zone made by `Channels` are removed by its `close`, which the test defers.
"""
import re

REGISTRY = []

VHF = "145.495"  # the two approved test frequencies (bench.py's test_freqs): the only ones ever keyed
UHF = "446.475"
BCAST = "100.5"  # a broadcast FM slot: the receiving radio's audio probe (BCR-24, BCR-26.5)


class Test:
    def __init__(self, fn, rules, part, keyups, idle, fast, rx_keys, relays=(), sdr=False, long=False):
        self.fn, self.rules, self.part = fn, list(rules), list(part)
        self.keyups, self.idle, self.fast, self.rx_keys = list(keyups), idle, fast, list(rx_keys)
        self.relays, self.sdr, self.long = list(relays), sdr, long
        self.name = fn.__name__
        self.module = fn.__module__.rsplit(".", 1)[-1]
        self.doc = (fn.__doc__ or "").strip().split("\n")[0]

    @property
    def keyed_s(self):
        return sum(self.keyups) + sum(self.rx_keys)


def test(*rules, part=(), keyups=(), idle=0.0, fast=False, rx_keys=(), relays=(), sdr=False, long=False):
    """Register a test: the rules it checks (all of `part` among them: those it checks only part of), the key-ups
    of the transmitting radio it plans (seconds each), the seconds it waits without keying, `fast` (needs the
    receiving radio's readback quick: the cable), `rx_keys` (key-ups of the receiving radio, seconds each:
    refusal probes count 0.6). Crossband: `relays` (how much longer than each key-up the receiving radio's
    repeater stays keyed, for the plan; one value for all), `sdr` (measures the air: skipped without --sdr),
    `long` (a key-up over the 5 s burst limit: skipped without --long)."""
    def deco(fn):
        REGISTRY.append(Test(fn, list(rules) + [p for p in part if p not in rules], part, keyups, idle, fast, rx_keys,
                             relays, sdr, long))
        return fn
    return deco


# ---- set-ups ----

def tx_on(t, mhz, txtone="off"):
    """The transmitting radio's slot A on its VFO at an approved frequency, low power, sending `txtone`, active."""
    t.tx.cmd(f"slot A vfo {mhz} fm low txtone {txtone} rxtone off offset off")
    t.tx.cmd("slot A active")
    t.tx.home()


def rx_vfo(t, L, mhz, tone="off", active=False):
    """The receiving radio's slot L on its VFO at `mhz` (both tones `tone`)."""
    t.rx.cmd(f"slot {L} vfo {mhz} fm low tone {tone} offset off")
    if active:
        t.rx.cmd(f"slot {L} active")


def rx_quiet(t, watch="Off"):
    """The receiving radio from home with no scan running, the active slot unmuted, no latched monitor, watch set
    as asked (0 1 1). Returns the slots whose scans were stopped (the runner's slot restore doesn't restart them)."""
    stopped = t.rx.stop_scans()
    t.rx.set("0 1 1", watch)
    u = t.rx.home()
    if u.active.get("state") == "MON":
        t.rx.keys("SIDE1")
    t.rx.unmute()
    return stopped


def rx_broadcast(t, L="C", required=True):
    """Slot L on a broadcast FM station: its card reads FM while no slot is heard and muted while one is
    (BCR-26.4, BCR-26.5), the receiving radio's one readback of whether a slot's audio has the speaker. When the
    slot doesn't become the broadcast slot, or it never plays (another slot heard all along), the test is skipped,
    or (required=False) None is returned."""
    ok, out = t.rx.cmd_err(f"slot {L} vfo {BCAST}")  # refused on the APRS slot (APRS-19) or a scanning one
    why = out.strip() if not ok else ""
    if ok:
        u, _ = t.poll(t.rx, lambda u: u.cards.get(L, {}).get("kind") == "broadcast", 3.0)
        why = "" if u is not None else f"not a broadcast slot: {t.rx.raw().cards.get(L)}"
    if not why:
        u, _ = t.poll(t.rx, lambda u: u.cards[L].get("state") == "FM", 6.0, every=0.2)
        why = "" if u is not None else f"it never plays (a slot heard all along?): {t.rx.raw().cards.get(L)}"
    if why and not required:
        t.note(f"slot {L} on {BCAST} MHz: {why}: the audio half of the checks left out")
        return None
    t.need(not why, f"slot {L} on {BCAST} MHz: {why}")
    return L


def status_page(t):
    """The receiving radio's Status page (MENU 0 0: quick settings, Advanced, Status), from home; MENU again if a
    toast took the first press."""
    u = None
    for _ in range(2):
        t.rx.home()
        u = t.rx.keys("MENU")
        if u.id == "Quick":
            return t.rx.keys("0 0")
    return u


def hears(t, L, timeout=2.5):
    """Inside a key-up: the receiving radio's slot L opens its squelch (rx=1) within `timeout`, else the test is
    skipped: the radios don't hear each other (closer, or the other way round)."""
    u, at = t.poll(t.rx, lambda u: u.cards.get(L, {}).get("rx") == 1, timeout)
    t.need(u is not None, f"the receiving radio's slot {L} never opened on the other radio's carrier "
                          f"(a metre apart? {t.rx.raw().cards.get(L)})")
    return u, at


def closes(t, L, timeout=3.0):
    """After the unkey: slot L's squelch closes (rx=0) within `timeout`; (readback, time) or (None, None)."""
    return t.poll(t.rx, lambda u: u.cards.get(L, {}).get("rx") == 0, timeout)


def state(u, L):
    return (u.cards.get(L) or {}).get("state", "")


# ---- channels and a zone, made for a test and removed after ----

class Channels:
    """A test zone holding test channels (`ch N "name" FREQ k=v ...`, numbers refused if in use), and channels in
    no zone; `enter(L, n)` puts slot L on channel n by the keys (V/M to channel mode, the number, OK); `close` (the
    test defers it) deletes the channels and the zone (\\* on the zones page, CHAN-12), as tools/uitests/zones.py
    does."""

    ZONE = "TWO-RADIO TEST"

    def __init__(self, t, zoned, loose=(), zone_kv=""):
        self.t, self.rx = t, t.rx
        self.zone, self.made = None, []
        self.chans = {}
        for n, *_ in list(zoned) + list(loose):
            ok, out = self.rx.cmd_err(f"ch {n}")
            t.need(not ok and "no such channel" in out, f"channel {n} is in use on the receiving radio: {out.strip()}")
        t.defer(self.close)
        if zoned:
            ok, out = self.rx.cmd_err(f'zone 254 "{self.ZONE}"')
            m = re.search(r"the next is zone (\d+)", out)
            t.need(m, f"no room for a test zone: {out.strip()}")
            self.zone = int(m.group(1))
            self.rx.cmd(f'zone {self.zone} "{self.ZONE}"' + (f" {zone_kv}" if zone_kv else ""))
        for n, name, mhz, kv in zoned:
            self.make(n, name, mhz, kv + f" zone={self.zone}")
        for n, name, mhz, kv in loose:
            self.make(n, name, mhz, kv)

    def make(self, n, name, mhz, kv):
        self.rx.cmd(f'ch {n} "{name}" {mhz} power=low {kv}'.strip())
        self.made.append(n)
        self.chans[n] = dict(name=name, mhz=mhz, kv=kv)

    def zone_rule(self, kv):
        self.rx.cmd(f'zone {self.zone} "{self.ZONE}" {kv}')

    def enter(self, L, n):
        """Slot L active, on channel n, in channel mode: V/M if on its VFO, then the number and OK (NAV-4)."""
        rx = self.rx
        rx.cmd(f"slot {L} active")
        u = rx.home()
        if u.active.get("head", "").startswith("VFO"):
            u = rx.keys("VM")
        u = rx.keys(" ".join(str(n)) + " MENU")
        u = rx.home()
        self.t.eq(u.active.get("name"), self.chans[n]["name"], f"slot {L} on channel {n} by V/M and its number")
        return u

    def tx_freqs(self):
        """Every frequency a slot on these channels could transmit on: each channel's, unless receive-only."""
        out = set()
        for n in self.made:
            line = self.rx.cmd(f"ch {n}")
            if " rxonly" in line:
                continue
            m = re.search(r"\s(\d+\.\d+)\s", line)
            split = re.search(r"\stx=(\d+\.\d+)", line)
            off = re.search(r"offset=([+-]\d+(?:\.\d+)?)", line)
            if not m:
                out.add(None)  # unreadable: the gate refuses
                continue
            f = float(split.group(1)) if split else float(m.group(1)) + (float(off.group(1)) if off else 0.0)
            out.add(round(f * 100000))
        return out

    def close(self):
        t, rx = self.t, self.rx
        notes = []
        # A slot still on a test channel goes to its VFO first (V/M's path), so none sits on a deleted channel;
        # the runner then puts every slot back as it was.
        for line in rx.cmd("slot").splitlines():
            m = re.match(r"\s*([ABC])\*?\s+ch (\d+)", line)
            if m and int(m.group(2)) in self.made:
                ok, out = rx.cmd_err(f"slot {m.group(1)} vfo")
                if not ok:
                    notes.append(f"slot {m.group(1)} off channel {m.group(2)}: {out}")
        for n in self.made:
            ok, out = rx.cmd_err(f"ch {n} delete")
            if not ok:
                notes.append(f"ch {n} delete: {out}")
        self.made = []
        if self.zone is not None:
            try:
                rx.home()
                u = rx.keys("MENU 0 2 3")
                for _ in range(260):
                    if u.sel and u.sel.get("kind") == "zone" and u.sel["name"] == self.ZONE:
                        break
                    u = rx.keys("UP")
                else:
                    raise RuntimeError("the test zone isn't in the zones list")
                u = rx.keys("STAR")
                if u.id == "Confirm":
                    rx.keys("MENU")
                if any(r["name"] == self.ZONE for r in rx.ui().rows):
                    notes.append("the test zone is still there")
            except Exception as e:
                notes.append(f"deleting the test zone: {e}")
            self.zone = None
        for n in notes:
            t.note(n)
        rx.home()


# ---- reading a scan ----

def stopped_at(t, L, mhz, timeout):
    """A scanning slot L stopped on `mhz`: its card on that frequency with the squelch open, or on it for three
    readbacks over at least 0.5 s (a channel scan steps every 50 ms). (readback, time) or (None, None)."""
    want = f"{float(mhz):.5f}"
    end = t.now() + timeout
    streak = []
    while t.now() < end:
        u = t.rx.raw()
        c = u.cards.get(L, {})
        now = t.now()
        if c.get("freq") == want:
            streak.append(now)
            if c.get("rx") == 1 or (len(streak) >= 3 and streak[-1] - streak[0] >= 0.5):
                return u, streak[0]
        else:
            streak = []
        t.sleep(0.02)
    return None, None


def left(t, L, mhz, timeout):
    """After a stop on `mhz`: the first readback with slot L elsewhere (still scanning). (readback, time)."""
    want = f"{float(mhz):.5f}"
    return t.poll(t.rx, lambda u: u.cards.get(L, {}).get("freq") != want, timeout)
