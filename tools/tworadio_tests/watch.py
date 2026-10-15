"""Watch, quick mute and the broadcast slot (chapters 6, 13, 20: WATCH-*, HOME-15, BCR-24, BCR-26.5).

The receiving radio: A active on 2 m, B watched on 70 cm (watch A+B), C on a broadcast FM station. The broadcast
card is the readback's one view of the speaker: it reads FM while no slot is heard and muted while one is
(BCR-26.5), so "B is heard" is "C reads muted while only B receives".
"""
from .core import UHF, VHF, closes, hears, rx_broadcast, rx_quiet, rx_vfo, state, status_page, test, tx_on


def watch_setup(t):
    rx_quiet(t, watch="A+B")
    rx_vfo(t, "A", VHF, active=True)
    rx_vfo(t, "B", UHF)
    rx_broadcast(t, "C")
    return t.rx.home()


def heard(t, timeout=1.5):
    """The broadcast card reads muted (a slot has the speaker) within `timeout`: (readback, time)."""
    return t.poll(t.rx, lambda u: state(u, "C") == "muted", timeout)


@test("WATCH-4", "WATCH-5", "BCR-26.5", part=("WATCH-3.2",), keyups=(4,), idle=5)
def watched_slot_plays_and_active_stays(t):
    """The other radio keys on the watched B while A is closed: B's card reads RX (a non-active card in RX is drawn
    with the green border, home.rs), the broadcast card reads muted (B has the speaker: WATCH-3.2's first case,
    WATCH-5's "plays", BCR-26.5), A stays the active slot on home and on the Status page, and after B closes."""
    watch_setup(t)
    tx_on(t, UHF)
    with t.key(4, "a carrier on the watched B"):
        u, _ = hears(t, "B", timeout=3.0)
        t.eq(state(u, "B"), "RX", "B's card: RX (highlighted)")
        t.eq(u.cards["B"]["active"], 0, "B isn't the active card")
        m, _ = heard(t)
        t.check(m is not None, "the broadcast card reads muted: B is heard", state(t.rx.raw(), "C"))
        t.eq(t.rx.raw().state.get("active"), "A", "A stays active")
        s = status_page(t)
        t.eq((s.id, s.state.get("active")), ("Status", "A"), "on the Status page: A still active")
        u = t.rx.home()
        t.eq(u.state.get("active"), "A", "back home: A")
    closes(t, "B", 3.0)
    u = t.rx.raw()
    t.eq((u.state.get("active"), u.cards["A"]["active"]), ("A", 1), "after B closes: A still active")


@test("BCR-24", keyups=(3,), idle=6, fast=True)
def broadcast_mutes_at_once_and_returns(t):
    """The broadcast plays on C; the other radio keys on the active A: the broadcast card reads muted as A opens
    (within 0.3 s of the first readback showing RX), and FM again 2 s after A's audio ends (after A's squelch
    closes and its 150 ms tail: 1.6 to 2.6 s)."""
    watch_setup(t)
    tx_on(t, VHF)
    with t.key(3, "a carrier on A"):
        _, at = hears(t, "A")
        m, mt = heard(t, 1.0)
        t.check(m is not None and mt - at <= 0.3, "muted at once", None if m is None else f"{mt - at:.2f} s")
    c, ct = closes(t, "A", 3.0)
    t.need(c is not None, "A didn't close after the unkey")
    f, ft = t.poll(t.rx, lambda u: state(u, "C") == "FM", 4.0)
    t.check(f is not None, "the broadcast comes back", state(t.rx.raw(), "C"))
    t.note(f"broadcast back {ft - ct:.2f} s after A closed")
    t.check(1.6 <= ft - ct <= 2.6, "back 2 s after A's audio ended", f"{ft - ct:.2f} s")


@test("HOME-15", "WATCH-3.3", part=("WATCH-3.2",), keyups=(3, 3), idle=6)
def muted_active_slot_listens_and_lets_b_play(t):
    """* mutes A (Muted). The other radio keys on A: A reads RX muted with its S-meter up, and nothing is heard
    (the broadcast stays FM: WATCH-3.3). Then on B: B is heard while A is muted (the broadcast mutes: WATCH-3.2's
    muted case). \\* again unmutes A."""
    watch_setup(t)
    u = t.rx.keys("STAR")
    t.check("muted" in state(u, "A").lower(), "* on home: A muted", state(u, "A"))
    t.defer(lambda: t.rx.unmute())
    tx_on(t, VHF)
    with t.key(3, "a carrier on the muted A"):
        hears(t, "A")
        got = t.samples(t.rx, 1.5)
    t.check(all(state(u, "A") == "RX muted" for _, u in got), "A reads RX muted while it receives",
            [state(u, "A") for _, u in got])
    t.check(any(u.cards["A"]["meter"] > 0 for _, u in got), "with its S-meter", [u.cards["A"]["meter"] for _, u in got])
    t.check(all(state(u, "C") == "FM" for _, u in got), "A isn't heard: the broadcast plays on",
            [state(u, "C") for _, u in got])
    closes(t, "A", 3.0)
    tx_on(t, UHF)
    with t.key(3, "a carrier on B while A is muted"):
        hears(t, "B", timeout=3.0)
        m, _ = heard(t)
        t.check(m is not None, "B is heard while A is muted (the broadcast mutes)", state(t.rx.raw(), "C"))
        t.eq(state(t.rx.raw(), "A"), "Muted", "A still Muted")
    closes(t, "B", 3.0)
    u = t.rx.keys("STAR")
    t.check("muted" not in state(u, "A").lower(), "* again: A unmuted", state(u, "A"))


@test("WATCH-6", keyups=(4,), idle=68)
def hash_pauses_watch_while_b_receives(t):
    """The other radio keys on the watched B: the hint bar says "# Pause"; # stays home, says so in a toast and the
    Status page's Watch row reads paused. The pause (0 1 2 2, 1 min: not settable yet) then runs out by itself (B's
    card no longer "paused") before the next test."""
    watch_setup(t)
    pause = t.rx.get("0 1 2 2")
    secs = {"30 s": 30, "1 min": 60, "5 min": 300}.get(pause, 300)
    tx_on(t, UHF)
    with t.key(4, "a carrier on the watched B"):
        hears(t, "B", timeout=3.0)
        u = t.rx.ui()
        t.eq(u.hint("#"), "Pause", "the hint bar: # Pause")
        u = t.rx.keys("HASH")
        t.eq(u.id, "Home", "# stays home")
        t.check(u.toast and "paused" in (u.toast["head"] + " " + u.toast["body"]).lower(), "a toast: watch paused",
                u.toast)
        s = status_page(t)
        t.has((s.row("Watch") or {}).get("value", ""), "paused", "Status: Watch")
        t.rx.home()
    u, _ = t.poll(t.rx, lambda u: state(u, "B") != "paused", secs + 6.0, every=1.0)
    t.check(u is not None, f"the pause runs out ({pause})", state(t.rx.raw(), "B"))
