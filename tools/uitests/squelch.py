"""Squelch and monitor on home (chapter 13, SQL-*): the level, SIDE1 + knob, SIDE1 tap and hold.

The level is the active slot's quick setting (MENU 1); `slot` and `dump` don't carry it, so each test puts it back
itself (the quick settings editor: MENU, OK, the digit, OK).
"""
from .core import test

QUIET = "146.55"  # nothing on it: the simulator's band noise stays under squelch 4


def level(t):
    t.home()
    v = t.keys("MENU").row("Squelch")["value"]
    t.home()
    return v


def set_level(t, v):
    t.home()
    t.keys("MENU")
    u = t.keys(f"MENU {v.split()[0]} MENU")
    t.eq(u.row("Squelch")["value"].split()[0], v.split()[0], f"squelch put back to {v}")
    t.home()


def quiet_slot(t):
    """Slot C active on a quiet 2 m VFO, every scan stopped (the simulator's fake radio puts carriers on A and B
    now and then, never on C). Returns what `put_back` needs."""
    stopped = t.stop_scans()
    t.cmd(f"slot C vfo {QUIET} fm low tone off offset off")
    t.cmd("slot C active")
    u = t.home()
    if u.active.get("state") == "MON":  # monitor latched by an earlier test: off
        t.keys("SIDE1")
    return stopped


@test("SQL-13", pages=("Home", "Quick", "Squelch", "Value"))
def level_zero_reads_open(t):
    """Level 0 is squelch off: the menus show it as "0 (open)"; SIDE1 + knob gets there with no OK (live)."""
    v0 = level(t)
    try:
        t.r.down("SIDE1")
        t.r.knob(-9)
        u = t.ui(0.3)
        t.eq(u.id, "Squelch", "SIDE1 + knob: the squelch screen")
        t.r.up("SIDE1")
        u = t.ui(0.5)
        t.eq(u.id, "Home", "SIDE1 let go: home")
        u = t.keys("MENU")
        t.eq(u.row("Squelch")["value"], "0 (open)", "quick settings show 0 (open)")
        u = t.keys("MENU")
        t.eq(u["value"]["args"], ["Squelch", "0 (open)"], "and so does its editor")
    finally:
        set_level(t, v0)


@test("SQL-7", "SQL-13", where="radio", pages=("Home", "Quick"))
def level_change_is_live(t):
    """SIDE1 + knob changes the level live: at 0 the squelch opens on a quiet channel, back at 4 it closes, with
    no OK anywhere (the simulator doesn't model the squelch level: its carriers come and go by time)."""
    stopped = quiet_slot(t)
    v0 = level(t)  # slot C's (the level is per slot)
    try:
        u = t.ui(0.5)
        t.need(u.active["rx"] == 0, f"{QUIET} MHz isn't quiet here: {u.active}")
        t.r.down("SIDE1")
        t.r.knob(-9)
        t.r.up("SIDE1")
        u = t.ui(0.6)
        t.eq(u.active["rx"], 1, "level 0: the squelch is open on a quiet channel")
        t.r.down("SIDE1")
        t.r.knob(+4)
        t.r.up("SIDE1")
        u = t.ui(0.6)
        t.eq(u.active["rx"], 0, "level 4: closed again")
    finally:
        set_level(t, v0)
        t.put_back(stopped)


@test("SQL-9.2", pages=("Home",))
def side1_tap_latches_monitor(t):
    """SIDE1 tapped: the squelch opens (MON on the card) and stays open until SIDE1 is tapped again."""
    stopped = quiet_slot(t)
    try:
        u = t.ui(0.5)
        t.need(u.active["rx"] == 0, f"{QUIET} MHz isn't quiet here: {u.active}")
        u = t.keys("SIDE1")
        t.check(u.toast and u.toast["head"] == "Monitor", "a Monitor toast", u.toast)
        u = t.ui(1.0)
        t.eq((u.active["state"], u.active["rx"]), ("MON", 1), "latched open")
        u = t.ui(1.0)
        t.eq(u.active["state"], "MON", "still open a second later")
        u = t.keys("SIDE1")
        u = t.ui(0.5)
        t.eq(u.active["rx"], 0, "tapped again: closed")
        t.check(u.active["state"] != "MON", "no MON", u.active)
    finally:
        t.put_back(stopped)


@test("SQL-9.1", xfail="SIDE1 held alone does nothing: only the tap (monitor latch) and SIDE1 + knob/arrows are "
                     "built (home.rs; sim, Oct 14, 2026)", pages=("Home",))
def side1_held_opens_while_held(t):
    """SIDE1 held: the squelch is open while it is held, and closes when it is let go."""
    stopped = quiet_slot(t)
    try:
        u = t.ui(0.5)
        t.need(u.active["rx"] == 0, f"{QUIET} MHz isn't quiet here: {u.active}")
        t.r.down("SIDE1")
        try:
            u = t.ui(1.2)
            t.eq(u.active.get("rx"), 1, "held 1.2 s: open")
        finally:
            t.r.up("SIDE1")
        u = t.ui(0.5)
        t.eq(u.active["rx"], 0, "let go: closed")
        t.check(u.active["state"] != "MON", "not latched", u.active)
    finally:
        t.put_back(stopped)
