"""Busy lockout (chapter 15, TX-20.2, TX-21): the receiving radio's own PTT refused on a busy channel."""
from .core import VHF, Channels, hears, rx_quiet, test, tx_on


@test("TX-20.2", "TX-21", keyups=(4,), rx_keys=(0.6,), idle=4)
def busy_lockout_refuses_ptt(t):
    """A channel with busy lockout and T 100.0 on the receiving radio's A; the other radio keys on it sending 88.5,
    the wrong tone (busy lockout has no wrong-tone mode: any carrier is busy). The receiving radio's PTT (a remote
    key-up, which meets every check a PTT does) is refused with "channel busy" and the refused screen. It is a key-up
    the radio must refuse: gated and duty-limited like any, and KEY off at once if it keys."""
    rx_quiet(t)
    ch = Channels(t, (), loose=((986, "TR BUSY", VHF, "busy=on tone=100.0"),))
    ch.enter("A", 986)
    t.has(t.rx.cmd("ch 986"), "busy=", "channel 986 has busy lockout")
    tx_on(t, VHF, "88.5")
    with t.key(4, "a carrier with the wrong tone on the busy channel"):
        hears(t, "A")
        with t.rx_probe("PTT on the busy channel") as ev:
            pass
        t.check(not ev.accepted and ev.refused and "busy" in ev.refused, "the key-up refused: channel busy",
                ev.refused or "it KEYED (unkeyed at once)")
        u = t.rx.raw()
        t.eq(u.id, "TxRefused", "the refused screen, as for PTT")
        t.has((u.d.get("refused") or {}).get("why", ""), "busy", "the refused screen's reason")
    t.rx.home()
