"""Remote key-ups that must be refused (chapters 15, 29, 32: TX-20, TX-35, LINK-20, SND-4). They go only through
`T.keyup_refused` (needs=("keyup",): the tx-dry build or the simulator); on the radio they are asked only on
145.495 or 446.475 MHz, simplex (the bench's keying frequencies), so a check that needs another frequency (160 MHz
out of band, an emergency frequency) runs in the simulator only. (After settings.py: TX-35.5 turns Extend TX on in
the simulator, and its warning shows once.)
"""
from .core import test

TEST_MHZ = "145.495"


def refused_line(u):
    return u["refused"] or {}


def am_slot(t):
    """Slot A on the test frequency in AM (the BK4829 has no AM modulator: refused before any key-up)."""
    t.cmd(f"slot A vfo {TEST_MHZ} am offset off")
    t.cmd("slot A active")
    return t.home()


@test("LINK-20.3", "SND-4", needs=("keyup",), pages=("Home", "TxRefused"))
def remote_keyup_refusal_shows(t):
    """A refused remote key-up shows on the radio as a PTT's does: the TX refused screen with the reason (and the
    menu path that fixes it where one does), never the Transmit screen; a refused transmit beeps (the amp, PE4,
    on a bench build). Here: AM on 145.495 MHz (and 160 MHz out of band in the simulator)."""
    stopped = t.stop_scans()
    try:
        am_slot(t)
        beep = []
        refused, why = t.keyup_refused()
        if t.r.bench and not t.sim:
            for _ in range(4):
                beep.append(t.pin("PE4"))
        u = t.wait(lambda u: u.id == "TxRefused", timeout=2, what="the refused screen")
        r = refused_line(u)
        t.eq(r.get("why"), "AM is receive only", "the reason")
        t.eq(u.state["keyed"], "-", "nothing keyed")
        t.check(why, "the link answer says why too", why)
        if beep:
            t.check(1 in beep, "the amp on for the refusal's beep (PE4)", beep)
        if t.sim:
            t.home()
            t.cmd("slot A vfo 160.000 fm offset off")
            t.home()
            t.keyup_refused()
            u = t.wait(lambda u: u.id == "TxRefused", timeout=2, what="160 MHz: the refused screen")
            r = refused_line(u)
            t.eq(r.get("why"), "outside the band plan", "out of band")
            t.check(r.get("fix", "").startswith("MENU 0"), "the menu path that fixes it", r)
    finally:
        t.put_back(stopped)


def free_channel(t):
    used = {int(x.split()[1]) for x in t.cmd("dump").splitlines() if x.startswith("ch ")}
    for n in range(999, 1, -1):
        if n not in used:
            return n
    t.need(False, "no free channel number")


@test("TX-20.1", needs=("keyup",), pages=("Home", "TxRefused"))
def keyup_refusals_by_case(t):
    """Remote key-ups refused with the TX refused screen (reason + the fixing path), never the Transmit screen: an
    AM slot; a receive-only channel (a channel made for the test on 145.495, deleted after); in the simulator also
    160 MHz (outside the band plan)."""
    stopped = t.stop_scans()
    n = None
    try:
        am_slot(t)
        t.keyup_refused()
        u = t.wait(lambda u: u.id == "TxRefused", timeout=2, what="AM: refused")
        t.eq(refused_line(u)["why"], "AM is receive only", "AM")
        t.home()
        n = free_channel(t)
        t.cmd(f'ch {n} "UT RX ONLY" {TEST_MHZ} rxonly=on')
        t.cmd("slot A vfo 146.52 fm")
        t.home()
        t.keys("VM")
        t.keys(" ".join(str(n)) + " MENU", settle=0.4)
        t.check(f"ch {n}" in t.slot("A"), f"A on channel {n}", t.slot("A"))
        t.keyup_refused()
        u = t.wait(lambda u: u.id == "TxRefused", timeout=2, what="RX-only channel: refused")
        r = refused_line(u)
        t.eq(r["why"], "receive-only channel", "the reason")
        t.check(r.get("fix"), "the path that fixes it (the channel editor)", r)
        t.eq(u.state["keyed"], "-", "nothing keyed")
        t.home()
        t.keys("VM")
        if t.sim:
            t.cmd("slot A vfo 160.000 fm offset off")
            t.home()
            t.keyup_refused()
            u = t.wait(lambda u: u.id == "TxRefused", timeout=2, what="160 MHz: refused")
            t.eq(refused_line(u)["why"], "outside the band plan", "out of band")
    finally:
        t.home()
        if "ch " in t.slot("A"):
            t.keys("VM")
        if n is not None:
            t.cmd_err(f"ch {n} delete")
        t.put_back(stopped)


def extend_tx(t, on):
    """Extend TX on (its warning, OK) or off, by keys (the simulator only: not a test's setting on the radio)."""
    t.home()
    u = t.keys("MENU 0 8 9")
    if (u.sel["value"] == "On") == on:
        return
    u = t.knob(1)
    if u.id == "ExtendTx":  # the warning, the first time (SETUP-8): OK turns it on
        t.keys("MENU")
    t.home()
    t.eq(t.get("0 8 9"), "On" if on else "Off", f"Extend TX {'on' if on else 'off'}")


@test("TX-35.5", needs=("keyup",), where="sim", pages=("ExtendTx", "Group", "Home"))
def emergency_refusal_toast(t):
    """With Extend TX on (the emergency frequencies are outside the amateur bands, so the band plan refuses first
    without it) a remote key-up on 406.025 MHz is refused with a toast saying why ("No remote TX: emergency
    frequency") and never opens the second-PTT prompt. Simulator only: on the radio the PC keys on 145.495 and
    446.475 MHz alone, and Extend TX is the owner's."""
    try:
        extend_tx(t, True)
        t.cmd("slot A vfo 406.025 fm offset off")
        t.cmd("slot A active")
        t.home()
        t.keyup_refused()
        u = t.ui(0.1)
        t.check(u.toast and "emergency" in (u.toast["head"] + " " + u.toast["body"]).lower(), "the toast says why",
                u.toast)
        t.check(u.id not in ("Emergency", "Transmit"), "no prompt, no transmit", u.id)
        t.eq(u.state["keyed"], "-", "nothing keyed")
    finally:
        extend_tx(t, False)
