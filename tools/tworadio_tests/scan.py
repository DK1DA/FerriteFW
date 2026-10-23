"""Scanning a test zone on the receiving radio while the other radio keys (chapter 14: SCAN-5, SCAN-19, SCAN-20.2,
SCAN-22; KEY-18).

The zone (made for the test, removed after): TR 2M on 145.495 and TR 70CM on 446.475 (the approved frequencies,
so a PTT on any hit transmits only there), two receive-only quiet channels, and TR RPT receiving 144.895 with a
+0.6 MHz shift (its input is 145.495, SCAN-22). The scan starts on TR 2M (channel mode, SIDE2: the current zone,
0 1 3). Settings for the test: scan inputs off, priority check off, the resume rule; scan target (Current zone) and
Return to (Start) at their defaults (not settable yet).
"""
from .core import UHF, VHF, Channels, left, rx_quiet, state, stopped_at, test, tx_on

ZONE_CHANS = (
    (981, "TR 2M", VHF, ""),
    (982, "TR 70CM", UHF, ""),
    (983, "TR Q1", "147.585", "rxonly=on"),
    (984, "TR Q2", "445.925", "rxonly=on"),
    (985, "TR RPT", "144.895", "offset=+0.6"),
)


def scan_fixture(t, resume="Carrier drop"):
    rx_quiet(t)
    for path, want in (("0 1 3", "Current zone"), ("0 1 8 6", "Start")):  # not settable yet: their defaults
        got = t.rx.get(path)
        t.need(got == want, f"{path} is {got!r}, not {want!r}")
    ch = Channels(t, ZONE_CHANS)
    for path, v in (("0 1 8 5", "Off"), ("0 1 5", "Off"), ("0 1 4", resume)):
        t.rx.set(path, v)
    t.defer(lambda: stop_scan(t))
    return ch


def start_scan(t, ch):
    """A new scan of the test zone from TR 2M (each new scan takes the settings and clears its nuisance skips)."""
    stop_scan(t)
    ch.enter("A", 981)
    t.rx.keys("SIDE2")  # the scan screen opens; home with the scan running (hold BACK)
    t.rx.home()
    u, _ = t.poll(t.rx, lambda u: u.cards["A"].get("scan") == 1, 2.0)
    t.need(u is not None, "SIDE2 didn't start a scan of the test zone")


def stop_scan(t):
    u = t.rx.raw()
    if u.id != "Home" or u.cards.get("A", {}).get("scan") == 1:
        u = t.rx.home()
    if u.cards.get("A", {}).get("scan") == 1:
        t.rx.keys("SIDE2")
        t.rx.home()
        t.poll(t.rx, lambda u: u.cards["A"].get("scan") == 0, 2.0)


def dropped(t, ev, stop):
    """When the carrier left the air: the card closing, when it showed the stop open (`stop`, the readback at the
    stop), else KEY off plus the 400 ms carrier hold."""
    if stop is not None and stop.cards["A"].get("rx") == 1:
        c, ct = t.poll(t.rx, lambda u: u.cards["A"].get("rx") == 0, 1.5)
        if c is not None:
            return ct
    return ev.t_off + 0.45


@test("SCAN-5", "SCAN-20.2", keyups=(4, 4.5, 1.5, 3, 3), idle=40, fast=True)
def resume_rules(t):
    """Each resume rule (0 1 4), the other radio keyed on TR 2M: carrier drop: held while the carrier lasts, on 2 s
    after it drops (1.6 to 2.6 s); 3 s: on 3 s after the stop with the carrier still there; 5 s with the carrier
    gone after 1.5 s: on 2 s after the drop, before the 5 s (SCAN-20.2); stop on hit: the scan ends there; the
    zone's own rule (stop) beats the radio's (3 s)."""
    ch = scan_fixture(t, "Carrier drop")
    tx_on(t, VHF)
    start_scan(t, ch)
    with t.key(4, "resume: carrier drop") as ev:
        u, st = stopped_at(t, "A", VHF, 2.0)
        t.need(u is not None, "the scan never stopped on the carrier")
        g, _ = left(t, "A", VHF, max(0.2, ev.t_on + 3.6 - t.now()))
        t.check(g is None, "carrier drop: held while the carrier lasts", g and g.cards["A"])
    ct = dropped(t, ev, u)
    g, gt = left(t, "A", VHF, 4.0)
    t.check(g is not None, "carrier drop: the scan goes on", t.rx.raw().cards["A"])
    if g is not None:
        t.note(f"carrier drop: on {gt - ct:.2f} s after the drop")
        t.check(1.6 <= gt - ct <= 2.6, "on 2 s after the carrier drops", f"{gt - ct:.2f} s")

    t.rx.set("0 1 4", "3 s")
    start_scan(t, ch)
    with t.key(4.5, "resume: 3 s"):
        u, st = stopped_at(t, "A", VHF, 1.2)
        t.need(u is not None, "the scan never stopped on the carrier")
        # The scan leaves for only one lap of the zone (~280 ms on the radio) and stops on the same carrier again:
        # the compact readback's state word ("scanning" while it steps) catches the lap where a frequency read
        # (left(), ~100 ms a readback) missed it (the two-radio run, Oct 15, 2026).
        g, gt = t.poll(t.rx, lambda u: state(u, "A") == "scanning", max(0.2, st + 3.8 - t.now()), light=True)
        t.check(g is not None, "3 s: the scan goes on with the carrier still there", state(t.rx.state(), "A"))
        if g is not None:
            t.check(2.5 <= gt - st <= 3.8, "3 s after the stop", f"{gt - st:.2f} s")
            a, _ = t.poll(t.rx, lambda u: state(u, "A") == "RX", 1.0, light=True)
            t.check(a is not None, "and stops on the carrier again", state(t.rx.state(), "A"))

    t.rx.set("0 1 4", "5 s")
    start_scan(t, ch)
    with t.key(1.5, "resume: 5 s, a short carrier") as ev:
        u, st = stopped_at(t, "A", VHF, 1.3)
        t.need(u is not None, "the scan never stopped on the carrier")
    ct = dropped(t, ev, u)
    g, gt = left(t, "A", VHF, 5.0)
    t.check(g is not None, "5 s: the scan goes on", t.rx.raw().cards["A"])
    if g is not None:
        t.check(1.6 <= gt - ct <= 2.6 and gt - st < 4.6, "on 2 s after the drop, before its 5 s",
                f"{gt - ct:.2f} s after the drop, {gt - st:.2f} s after the stop")

    t.rx.set("0 1 4", "Stop on hit")
    start_scan(t, ch)
    with t.key(3, "resume: stop on hit"):
        u, _ = stopped_at(t, "A", VHF, 2.0)
        t.need(u is not None, "the scan never stopped on the carrier")
        e, _ = t.poll(t.rx, lambda u: u.cards["A"].get("scan") == 0, 1.5)
        t.check(e is not None, "stop on hit: the scan ends", t.rx.raw().cards["A"])
    got = t.samples(t.rx, 2.5, every=0.25)
    t.check(all(u.cards["A"].get("scan") == 0 and u.cards["A"].get("name") == "TR 2M" for _, u in got),
            "and stays on TR 2M after the carrier", [u.cards["A"] for _, u in got[-1:]])

    t.rx.set("0 1 4", "3 s")
    ch.zone_rule("resume=stop")
    start_scan(t, ch)
    with t.key(3, "the zone's own rule: stop on hit"):
        u, _ = stopped_at(t, "A", VHF, 2.0)
        t.need(u is not None, "the scan never stopped on the carrier")
        e, _ = t.poll(t.rx, lambda u: u.cards["A"].get("scan") == 0, 1.5)
        t.check(e is not None, "the zone's stop on hit beats the radio's 3 s: the scan ends", t.rx.raw().cards["A"])


@test("SCAN-22", keyups=(3, 3), idle=10)
def scan_inputs_stop_on_a_repeater_input(t):
    """TR 2M skipped, so 145.495 is only TR RPT's input (144.895 +0.6). Scan inputs off (0 1 8 5): the carrier on
    145.495 never stops the scan; on: the scan stops on 145.495, the input of TR RPT."""
    ch = scan_fixture(t)
    t.rx.cmd("ch 981 skip on")
    tx_on(t, VHF)
    start_scan(t, ch)
    with t.key(3, "scan inputs off"):
        u, _ = stopped_at(t, "A", VHF, 2.4)
        t.check(u is None, "scan inputs off: no stop on 145.495", u and u.cards["A"])
    t.rx.set("0 1 8 5", "On")
    start_scan(t, ch)
    with t.key(3, "scan inputs on"):
        u, _ = stopped_at(t, "A", VHF, 2.4)
        t.check(u is not None, "scan inputs on: stops on TR RPT's input, 145.495", t.rx.raw().cards["A"])


@test("SCAN-19", keyups=(3,), rx_keys=(1.5,), idle=8)
def ptt_on_a_hit_transmits_there(t):
    """The scan from TR 2M stops on TR 70CM (the other radio keyed on 446.475); in the 2 s resume wait after the
    carrier drops the receiving radio's PTT (a remote key-up): the scan ends and the slot moves onto TR 70CM and
    transmits there (446.475), not where the scan started (145.495). Every channel in the zone transmits only on an
    approved frequency or not at all (receive-only), so the gate allows this key-up of a scanning slot."""
    ch = scan_fixture(t, "Carrier drop")
    tx_on(t, UHF)
    start_scan(t, ch)
    with t.key(3, "a hit on TR 70CM"):
        u, _ = stopped_at(t, "A", UHF, 2.5)
        t.need(u is not None, "the scan never stopped on TR 70CM")
    t.sleep(0.6)  # the carrier's 400 ms hold: now in the resume wait
    with t.rx_keyup(1.5, "PTT on the hit", scan_freqs=ch.tx_freqs()) as ev:
        t.check(ev.accepted, "the receiving radio keyed", ev.refused)
        u = t.rx.raw()
        t.eq(u.state.get("keyed"), "A", "transmitting on A")
        b = t.rx.slot_bin(0)
        t.eq(b["freq10"], 44647500, "on the hit's frequency, 446.475")
        t.eq(None if b["channel"] is None else b["channel"] + 1, 982, "on the hit's channel, TR 70CM")
        t.eq(b["scanning"], False, "the scan ended")


@test("KEY-18", keyups=(4, 4), idle=10)
def star_and_hash_skip_a_hit(t):
    """Stopped on TR 2M with the carrier still there: \\* on home leaves it and the scan doesn't stop there again this
    scan. A new scan, on its scan screen: stopped on the hit the hints offer "* Skip" and "# Skip always"; # leaves it
    and sets the channel's Skip flag (every scan, SCAN-17)."""
    ch = scan_fixture(t, "Carrier drop")
    tx_on(t, VHF)
    start_scan(t, ch)
    with t.key(4, "* on a hit"):
        u, _ = stopped_at(t, "A", VHF, 1.5)
        t.need(u is not None, "the scan never stopped on the carrier")
        t.rx.keys("STAR")
        g, _ = left(t, "A", VHF, 1.0)
        t.check(g is not None, "* leaves the hit", t.rx.raw().cards["A"])
        again, _ = stopped_at(t, "A", VHF, 1.5)
        t.check(again is None, "and the scan doesn't stop there again (skipped for this scan)", again and again.cards["A"])
    stop_scan(t)
    ch.enter("A", 981)
    u = t.rx.keys("SIDE2")  # a new scan; its screen stays up
    t.eq(u.id, "Scan", "SIDE2: the scan screen")
    with t.key(4, "# on a hit"):
        u, _ = t.poll(t.rx, lambda u: u.hint("#") == "Skip always", 2.0)
        t.check(u is not None, "a new scan stops there again: \"# Skip always\" on the scan screen", t.rx.raw().hints)
        if u is not None:
            t.eq(u.hint("*"), "Skip", "and \"* Skip\"")
            u = t.rx.keys("HASH")
            t.check(u.toast and "skipped" in (u.toast["head"] + " " + u.toast["body"]).lower(), "# skips it: the toast",
                    u.toast)
            g, _ = t.poll(t.rx, lambda u: u.hint("#") != "Skip always", 1.0)
            t.check(g is not None, "# leaves the hit", t.rx.raw().hints)
    t.rx.home()
    t.has(t.rx.cmd("ch 981"), " skip", "# set TR 2M's Skip flag")
