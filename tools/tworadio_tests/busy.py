"""Busy lockout (chapter 15, TX-20.2, TX-21): the receiving radio's own PTT refused on a busy channel."""
from .core import VHF, Channels, hears, rx_quiet, test, tx_on


KEY_S = 4.5  # the carrier: long enough for the receiving radio's readback and its probe's gates over Bluetooth


@test("TX-20.2", "TX-21", keyups=(KEY_S,), rx_keys=(0.6,), idle=4)
def busy_lockout_refuses_ptt(t):
    """A channel with busy lockout and T 100.0 on the receiving radio's A; the other radio keys on it sending 88.5,
    the wrong tone (busy lockout has no wrong-tone mode: any carrier is busy). The receiving radio's PTT (a remote
    key-up, which meets every check a PTT does) is refused with "channel busy" and the refused screen. It is a key-up
    the radio must refuse: gated and duty-limited like any, and KEY off at once if it keys."""
    rx_quiet(t)
    ch = Channels(t, (), loose=((986, "TR BUSY", VHF, "busy=on tone=100.0"),))
    ch.enter("A", 986)
    t.has(t.rx.cmd("ch 986"), "busy=on", "channel 986 has busy lockout")
    line = t.rx.slot("A")
    t.check(line.startswith("A* ch 986"), "the receiving radio's active slot A is on channel 986 (the probe keys "
            "the active slot)", line)
    tx_on(t, VHF, "88.5")
    with t.key(KEY_S, "a carrier with the wrong tone on the busy channel") as k:
        hears(t, "A")
        t.need(k.t_off is None and k.deadline - t.now() > 1.0, "the carrier would end before the probe (the "
               f"receiving radio's readback took {t.rx.rb:.1f} s): no busy channel to probe")
        with t.rx_probe("PTT on the busy channel") as ev:
            pass
        # The carrier still up when the probe's KEY on went: else a keyed probe says nothing of busy lockout.
        carrier = k.t_off is None or (ev.t_sent is not None and k.t_off > ev.t_sent)
        t.need(not (ev.accepted and not carrier), "the other radio's carrier ended before the probe's KEY on (its "
               "gates' SLOT reads): the probe keyed a free channel, which says nothing of busy lockout")
        t.check(not ev.accepted and ev.refused and "busy" in ev.refused, "the key-up refused: channel busy",
                ev.refused or f"it KEYED (unkeyed at once), the carrier on {k.t_off is None}")
        u = t.rx.raw()
        t.eq(u.id, "TxRefused", "the refused screen, as for PTT")
        t.has((u.d.get("refused") or {}).get("why", ""), "busy", "the refused screen's reason")
    t.rx.home()
