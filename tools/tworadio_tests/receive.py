"""Receiving a carrier on the active slot: the card, the screen's wake, Battery Save's hold, the scope layout
(chapters 6, 12, 31, 33)."""
import re

from .core import VHF, closes, hears, rx_quiet, rx_vfo, state, test, tx_on


@test("HOME-6", keyups=(3,), idle=4)
def receive_shows_rx_and_meter(t):
    """The other radio keys on the receiving radio's active A: A's card reads RX with the S-meter up; the
    transmitting radio shows its transmit screen (TX, keyed=A); after the unkey both are back (RX gone, the meter
    back to its idle reading before the key-up: the noise floor, S1 on the black radio, not necessarily 0)."""
    rx_quiet(t)
    rx_vfo(t, "A", VHF, active=True)
    tx_on(t, VHF)
    u = t.rx.home()
    t.need(u.cards["A"]["rx"] == 0, f"{VHF} MHz is busy here before the key-up: {u.cards['A']}")
    idle = u.cards["A"]["meter"]
    with t.key(3, "a carrier on A") as k:
        # The transmitting radio first, right after KEY on: read after the receiving radio's (on Bluetooth that
        # takes 2 to 3 s) it came after the key-up's own end, and `keyed=-` was the truth (Oct 14, 2026).
        v = t.tx.raw()
        on = k.t_off is None
        t.check(on, "the transmitting radio's readback inside its key-up", f"KEY off sent {k.t_off}")
        t.eq(v.state.get("keyed"), "A", "the transmitting radio: keyed on A")
        t.eq(v.state.get("remote"), 1, "the transmitting radio: keyed remotely (LINK-20)")
        t.eq(v.id, "Transmit", "the transmitting radio shows its transmit screen (TX)")
        u, _ = hears(t, "A")
        t.eq(state(u, "A"), "RX", "A's card: RX")
        t.check(u.cards["A"]["meter"] > idle, "A's S-meter shows the signal", f"{u.cards['A']} (idle {idle})")
    u, _ = closes(t, "A")
    t.check(u is not None, "RX goes after the unkey", t.rx.raw().cards["A"])
    t.check(u.cards["A"]["meter"] <= idle, f"the S-meter drops back to its idle reading ({idle})", u.cards["A"]["meter"])
    t.eq(t.tx.raw().state.get("keyed"), "-", "the transmitting radio is back to receive")


@test("DISP-15.3", keyups=(3, 3, 3), idle=24)
def receiving_wakes_the_screen(t):
    """Backlight timeout 5 s (0 7 4 1); for each Wake screen on (0 7 4 4) the screen is let go dark (no keys), then
    the other radio keys on A: with "Keys and RX" and "Keys, RX and TX" the screen lights and stays lit while the
    signal lasts; with "Keys" it stays dark."""
    rx_quiet(t)
    rx_vfo(t, "A", VHF, active=True)
    tx_on(t, VHF)
    t.rx.set("0 7 4 1", "5 s")
    for wake, lit in (("Keys and RX", True), ("Keys, RX and TX", True), ("Keys", False)):
        t.rx.set("0 7 4 4", wake)
        t.rx.home()
        u, _ = t.poll(t.rx, lambda u: u.state.get("dark") == 1, 9.0, every=0.5)
        t.need(u is not None, "the screen never went dark with a 5 s timeout")
        with t.key(3, f"wake on: {wake}"):
            hears(t, "A")
            got = t.samples(t.rx, 1.8)
        darks = [u.state.get("dark") for _, u in got]
        settled = [d for (at, _), d in zip(got, darks) if at - got[0][0] >= 0.3] or darks
        if lit:
            t.check(settled and all(d == 0 for d in settled), f"{wake}: lit while the signal lasts", darks)
        else:
            t.check(all(d == 1 for d in darks), f"{wake}: stays dark", darks)


def reg30(t):
    """The VHF chip's REG_30 (`bk 0 30`, a bench build's): its value, or None when the chip doesn't answer."""
    ok, out = t.rx.cmd_err("bk 0 30")
    m = re.search(r"= 0x([0-9A-Fa-f]{4})", out) if ok else None
    return int(m.group(1), 16) if m else None


def reads(t, secs, every=0.25):
    out, end = [], t.now() + secs
    while t.now() < end:
        out.append((t.now(), reg30(t)))
        t.sleep(every)
    return out


@test("SQL-8", keyups=(4,), idle=30)
def battery_save_holds_the_receiver(t):
    """Battery Save Level 1, watch off, A on 2 m: idle past the 10 s hold the VHF chip cycles (REG_30 reads its
    receive value only part of the time, `bk 0 30`); the other radio keys 4 s: awake throughout; after the
    squelch closes it stays awake for 10 s (every read in the first 9.5 s its receive value), then cycles again."""
    rx_quiet(t, watch="Off")
    rx_vfo(t, "A", VHF, active=True)
    tx_on(t, VHF)
    t.rx.set("0 7 7", "Level 1")
    t.rx.home()
    t.need(reg30(t) is not None, "`bk 0 30` doesn't answer (a bench build has it)")
    t.sleep(11)  # past the hold a key press started
    idle = [v for _, v in reads(t, 2.0, 0.15)]
    with t.key(4, "a carrier while Battery Save runs"):
        hears(t, "A")
        on = [v for _, v in reads(t, 2.0, 0.15)]
    awake = max(set(on), key=on.count) if on else None
    t.check(on and all(v == awake for v in on), "awake while the squelch is open (one REG_30 value)", on)
    t.need(any(v != awake for v in idle), f"idle before the key-up the chip never read asleep ({idle}): can't tell")
    c, ct = closes(t, "A", 3.0)
    t.check(c is not None, "the squelch closes after the unkey")
    after = reads(t, 13.0)
    held = [v for at, v in after if at - ct <= 9.5]
    late = [v for at, v in after if at - ct >= 10.5]
    t.check(held and all(v == awake for v in held), "held on for 10 s after the squelch closed", held)
    t.check(any(v != awake for v in late), "then the sleep cycle again", late)


@test("MODE-5", part=("MODE-5",), keyups=(2, 2), idle=6, fast=True)
def scope_layout_hears_at_once(t):
    """Single + scope (0 7 2) with A active on 2 m: the carrier shows RX on A as soon as in the watch layout (within
    0.15 s of it: the strip keeps listening on A's own frequency while it sweeps). The broadcast and the clicks of
    MODE-5's other cases stay by ear."""
    rx_quiet(t)
    rx_vfo(t, "A", VHF, active=True)
    tx_on(t, VHF)
    delay = {}
    for layout in ("Watch", "Single + scope"):
        t.rx.set("0 7 2", layout)
        t.rx.home()
        t.sleep(1.0)
        with t.key(2, f"layout {layout}") as ev:
            u, at = hears(t, "A")
            delay[layout] = at - ev.t_on
        closes(t, "A")
    t.note(f"RX seen {delay['Watch']:.2f} s after the key-up in the watch layout, {delay['Single + scope']:.2f} s "
           "with single + scope")
    t.check(delay["Single + scope"] <= delay["Watch"] + 0.15, "single + scope hears as soon (within 0.15 s)", delay)
