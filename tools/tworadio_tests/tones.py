"""Tone squelch, the tone hint and tone search (chapters 16, 22: TONE-20.4, TSRCH-3, TSRCH-12, TSRCH-14, TSRCH-15).

The card's state reads "heard" while the tone hint shows (TSRCH-13: a carrier the slot's tone squelch keeps shut,
its tone measured); with a broadcast slot on C its card reads muted while a slot has the speaker, so "opens" is
seen as the broadcast muting (when the receiving radio takes the broadcast; else that part is left out).
"""
from .core import VHF, closes, hears, rx_broadcast, rx_quiet, rx_vfo, state, test, tx_on


def setup(t, tone):
    rx_quiet(t, watch="Off")
    probe = rx_broadcast(t, "C", required=False)
    rx_vfo(t, "A", VHF, tone=tone, active=True)
    t.rx.home()
    return probe


def burst(t, secs, what, after_open=2.2):
    """Key `secs`, wait for A to open, then read back for `after_open` s: [(time, readback)], the open time."""
    with t.key(secs, what):
        _, at = hears(t, "A")
        got = t.samples(t.rx, min(after_open, max(0.2, secs - 0.4 - (t.now() - at))))
    return got, at


@test("TONE-20.4", keyups=(3, 3, 3, 3), idle=8)
def dcs_opens_only_on_its_polarity(t):
    """The receiving radio's A on D023N, then D023I; the other radio sends the slot's own code, then the other
    polarity. Its own: the squelch opens (no tone hint; the broadcast mutes: A heard). The other polarity: it stays
    shut (the tone hint shows: the tone squelch kept it shut; the broadcast plays on)."""
    other = {"D023N": "D023I", "D023I": "D023N"}
    for code in ("D023N", "D023I"):
        probe = setup(t, code)
        for sent in (code, other[code]):
            tx_on(t, VHF, sent)
            got, _ = burst(t, 3, f"{sent} into a {code} slot")
            states = [state(u, "A") for _, u in got]
            bc = [state(u, probe) for _, u in got] if probe else []
            if sent == code:
                t.check("heard" not in states, f"{sent} into {code}: opens (no tone hint)", states)
                if probe:
                    t.check("muted" in bc, f"{sent} into {code}: heard (the broadcast mutes)", bc)
            else:
                t.check("heard" in states, f"{sent} into {code}: stays shut (the tone hint shows)", states)
                if probe:
                    t.check("muted" not in bc, f"{sent} into {code}: not heard (the broadcast plays on)", bc)
            closes(t, "A", 3.0)


@test("TSRCH-12", keyups=(4, 3), idle=8, fast=True)
def wrong_tone_is_measured_then_put_back(t):
    """A on T 100.0, the other radio sending 88.5: after the carrier has held the squelch open 0.5 s the tone is
    measured: the hint shows 0.4 to 2.0 s after the squelch opened (0.5 s, then up to 1.2 s of scan), and the audio
    stays muted (the broadcast plays on). Then 100.0 sent: the slot's own detector is back, the tone squelch opens
    (no hint, the broadcast mutes)."""
    probe = setup(t, "100.0")
    tx_on(t, VHF, "88.5")
    with t.key(4, "88.5 into T 100.0"):
        _, at = hears(t, "A")
        h, ht = t.poll(t.rx, lambda u: state(u, "A") == "heard", 2.5)
        t.check(h is not None, "the tone hint shows (the probe measured it)", state(t.rx.raw(), "A"))
        t.note(f"hint {ht - at:.2f} s after the squelch opened")
        t.check(0.4 <= ht - at <= 2.0, "measured after 0.5 s, within 1.2 s more", f"{ht - at:.2f} s")
        if probe:
            got = t.samples(t.rx, 0.8)
            t.check(all(state(u, probe) == "FM" for _, u in got), "the audio stays muted (the broadcast plays on)",
                    [state(u, probe) for _, u in got])
    closes(t, "A", 3.0)
    tx_on(t, VHF, "100.0")
    got, _ = burst(t, 3, "100.0 into T 100.0", after_open=1.5)
    t.check(all(state(u, "A") == "RX" for _, u in got[1:]), "the slot's own tone opens it again (no hint)",
            [state(u, "A") for _, u in got])
    if probe:
        t.check(any(state(u, probe) == "muted" for _, u in got), "heard (the broadcast mutes)",
                [state(u, probe) for _, u in got])


def hint(t, secs=3, what="88.5 into T 100.0"):
    """A key-up with the wrong tone that leaves the hint up; the time A closed after it."""
    tx_on(t, VHF, "88.5")
    with t.key(secs, what):
        hears(t, "A")
        h, _ = t.poll(t.rx, lambda u: state(u, "A") == "heard", 2.5)
        t.need(h is not None, "no tone hint on a wrong tone (TSRCH-12 fails first)")
    c, ct = closes(t, "A", 3.0)
    return ct


@test("TSRCH-14", keyups=(3, 3, 3, 3, 3), idle=40)
def tone_hint_clears(t):
    """A on T 100.0. The hint after a wrong-tone carrier clears 30 s after the carrier was last heard (28 to 32.5 s
    after the squelch closed), at once when the tone squelch opens (the right tone sent), at once when the slot's
    receive tone changes, and at once when its frequency changes."""
    setup(t, "100.0")
    ct = hint(t)
    still, _ = t.poll(t.rx, lambda u: state(u, "A") != "heard", 25.0 - (t.now() - ct), every=1.0)
    t.check(still is None, "the hint stays up to 25 s after the carrier", state(t.rx.raw(), "A"))
    gone, gt = t.poll(t.rx, lambda u: state(u, "A") != "heard", 9.0, every=0.25)
    t.check(gone is not None, "it clears", state(t.rx.raw(), "A"))
    if gone is not None:
        t.note(f"hint cleared {gt - ct:.1f} s after the carrier")
        t.check(28.0 <= gt - ct <= 32.5, "30 s after the carrier was last heard", f"{gt - ct:.1f} s")
    # the tone squelch opening
    hint(t)
    tx_on(t, VHF, "100.0")
    with t.key(3, "100.0 into T 100.0 with the hint up"):
        hears(t, "A")
        u, _ = t.poll(t.rx, lambda u: state(u, "A") == "RX", 1.0)
        t.check(u is not None, "the tone squelch opens: the hint clears", state(t.rx.raw(), "A"))
    closes(t, "A", 3.0)
    # the receive tone changing
    hint(t)
    t.rx.cmd("slot A rxtone 107.2")
    t.check(state(t.rx.raw(), "A") != "heard", "a new receive tone: cleared at once", state(t.rx.raw(), "A"))
    t.rx.cmd("slot A rxtone 100.0")
    # the frequency changing
    hint(t)
    t.rx.cmd("slot A vfo 145.500")
    t.check(state(t.rx.raw(), "A") != "heard", "a new frequency: cleared at once", state(t.rx.raw(), "A"))
    rx_vfo(t, "A", VHF, tone="100.0")


@test("TSRCH-15", keyups=(4.5, 3, 3), idle=8)
def one_measurement_per_carrier(t):
    """A on T 100.0, 88.5 sent: the hint shows; the receive tone changed to 107.2 mid-carrier clears it, and no
    second measurement comes while that carrier lasts. The next carrier is measured again. With no receive tone
    nothing is measured (the state stays RX)."""
    setup(t, "100.0")
    tx_on(t, VHF, "88.5")
    with t.key(4.5, "one carrier, the receive tone changed under it"):
        _, at = hears(t, "A")
        h, _ = t.poll(t.rx, lambda u: state(u, "A") == "heard", 2.5)
        t.need(h is not None, "no tone hint on a wrong tone (TSRCH-12 fails first)")
        t.rx.cmd("slot A rxtone 107.2")
        rest = max(0.3, at + 4.0 - t.now())
        got = t.samples(t.rx, rest)
    t.check(got and all(state(u, "A") != "heard" for _, u in got), "no second measurement on the same carrier",
            [state(u, "A") for _, u in got])
    closes(t, "A", 3.0)
    with t.key(3, "a new carrier, 88.5 into T 107.2"):
        hears(t, "A")
        h, _ = t.poll(t.rx, lambda u: state(u, "A") == "heard", 2.5)
        t.check(h is not None, "the next carrier is measured", state(t.rx.raw(), "A"))
    closes(t, "A", 3.0)
    t.rx.cmd("slot A rxtone off")
    got, _ = burst(t, 3, "88.5 into a slot with no receive tone")
    t.check(all(state(u, "A") == "RX" for _, u in got), "no receive tone: nothing measured (RX)",
            [state(u, "A") for _, u in got])


@test("TSRCH-3", part=("TSRCH-2",), keyups=(4, 4, 4), idle=10)
def tone_search_finds_the_tone(t):
    """Tone search (MENU 0 3 5) on A at 145.495; the other radio sends 88.5 there: the screen's result (the
    readback's `tsearch found=`) reads "T 88.5" within 3 s of the carrier, with no RX tone on the slot and with
    one (T 100.0, which the search must not just repeat); 67.0 and 250.3, the table's ends (decision 177's scale);
    then D023N. The receiving session (Oct 15, 2026) found
    nothing: the Radio task dropped the screen's command."""
    rx_quiet(t)
    for rxtone, sent, want in (("off", "88.5", "T 88.5"), ("100.0", "88.5", "T 88.5"), ("off", "67.0", "T 67.0"),
                               ("off", "250.3", "T 250.3"), ("off", "D023N", "D023N")):
        rx_vfo(t, "A", VHF, tone=rxtone, active=True)
        t.rx.home()
        u = t.rx.keys("MENU 0 3 5")
        t.need(u.id == "ToneSearch", f"MENU 0 3 5 isn't tone search: {u.id}")
        t.eq((u["tsearch"] or {}).get("found"), "", f"rx tone {rxtone}: searching before the carrier")
        tx_on(t, VHF, sent)
        with t.key(4, f"{sent} into tone search (rx tone {rxtone})"):
            got, at = t.poll(t.rx, lambda u: (u["tsearch"] or {}).get("found") == want, 3.5)
            t.check(got is not None, f"rx tone {rxtone}: {sent} found", t.rx.raw()["tsearch"])
        t.rx.home()
