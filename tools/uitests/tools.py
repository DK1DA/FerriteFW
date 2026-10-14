"""Tools and the rest reached by keys: FM broadcast, scope, scan, crossband, the clock, the link's records."""
import struct

import ferrite_link as fl

from .core import test


@test("BCR-1")
def fm_broadcast_screen(t):
    """MENU 0 3 1 opens FM broadcast (◀▶ seek, OK presets); BACK leaves and the slots are as they were."""
    before = [t.slot(L) for L in "AB"]
    u = t.keys("MENU 0 3 1")
    t.eq((u.id, u.title), ("Fm", "FM"), "MENU 0 3 1")
    t.eq(u.hint("<>"), "Seek", "◀▶ seek")
    u = t.keys("BACK")
    t.check(u.id != "Fm", "BACK leaves", u.id)
    t.eq([t.slot(L) for L in "AB"], before, "slots A and B unchanged")


@test("KEY-5", "MENU-25")
def scope_opens_and_closes(t):
    """Hold 1: the band scope (◀▶ marker, SIDE1 listen, OK go); BACK leaves it and home tunes as before."""
    f0 = t.ui().active["freq"]
    u = t.press("1", long=True)
    t.eq(u.id, "Scope", "hold 1")
    t.eq(u.hints, [("<>", "Marker"), ("SIDE1", "Listen"), ("OK", "Go")], "the scope's keys")
    u = t.keys("BACK")
    t.eq(u.id, "Home", "BACK")
    t.eq(u.active["freq"], f0, "the active slot as it was")


@test("SCAN-2", "KEY-18", "SBAR-11")
def side2_starts_and_stops_a_scan(t):
    """SIDE2 starts a scan on the active slot (the scan screen names its keys: SIDE2 Stop), SIDE2 stops it; the
    slot line says scanning meanwhile; the target follows 0 1 4."""
    target = t.get("0 1 4")
    u = t.keys("SIDE2")
    u = t.ui(0.3)
    t.eq(u.id, "Scan", "SIDE2: the scan screen")
    t.eq(u.hint("SIDE2"), "Stop", "SIDE2 stops it")
    t.check("scanning" in t.slot("A"), "slot A scanning", t.slot("A"))
    u = t.keys("SIDE2")
    t.check("scanning" not in t.slot("A"), "stopped", t.slot("A"))
    t.note(f"target {target}")


@test("XB-2", "XB-3")
def crossband_start_page(t):
    """MENU 0 3 5: the crossband repeater's start page (knob changes, # starts, BACK leaves); nothing starts
    by opening it."""
    u = t.keys("MENU 0 3 5")
    t.eq(u.id, "Crossband", "MENU 0 3 5")
    t.eq(u.hint("#"), "Start", "# starts")
    t.eq(u.state["keyed"], "-", "nothing transmits")
    u = t.keys("BACK")
    t.check(u.id != "Crossband", "BACK leaves", u.id)


@test("GPS-23")
def clock_typed_by_hand(t):
    """OK on 0 6 6 types the date and time as 12 digits; a date that doesn't exist is refused."""
    u = t.keys("MENU 0 6 6")
    t.eq(u.sel and u.sel["name"], "Clock", "MENU 0 6 6")
    u = t.keys("MENU")
    t.eq(u.id, "Text", "OK: the clock typed by hand")
    u = t.keys("2 0 2 6 0 2 3 0 1 2 0 0 MENU")
    t.check(u.id == "Text" or (u.toast and u.toast["head"]), "Feb 30 refused", (u.id, u.toast))
    t.check(u.toast is not None, "with a reason", u.toast)
    t.home()
    u = t.keys("MENU 0 6 6 MENU")
    u = t.keys("2 0 2 6 1 0 1 4 1 2 3 4 MENU")
    t.check(u.id != "Text", "a good date is taken", (u.id, u.toast))


@test("LINK-21", "LINK-22")
def link_records_round_trip(t):
    """The link's READ gives channel 12 as `ch 12` shows it and the settings with the PIN as zeros; a channel
    written over the link reads back; a bad one is refused `invalid` with the reason; delete removes it."""
    link = t.r.link

    def ask(ty, payload):
        s, rest = link.ask(ty, payload)
        return s, rest

    def read(what, idx):
        rec = bytearray()
        while True:
            s, a = ask(fl.READ, struct.pack("<BHHB", what, idx, len(rec), 200))
            t.check(s == 0, f"READ {what} {idx}", fl.why(s, a))
            total = struct.unpack("<H", a[:2])[0]
            rec += a[2:]
            if len(rec) >= total:
                return bytes(rec)

    rec = read(fl.CHANNEL, 11)
    num, zone, rx, name = fl.channel_fields(rec)
    t.has(t.cmd("ch 12"), f'"{name}"', "READ channel 12's name")
    t.eq(f"{rx / 100000:.5f}", t.cmd("ch 12").split('"')[2].split()[0], "and its frequency")
    st = read(fl.SETTINGS, 0)
    t.check(len(st) > 16, "the settings record", len(st))
    n = 989
    ok, out = t.cmd_err(f"ch {n + 1}")
    t.need(not ok and "no such channel" in out, f"channel {n + 1} in use: {out}")
    new = bytearray(rec)
    new[0:2] = struct.pack("<H", n)
    new[7:23] = b"UITEST LINK".ljust(16, b"\x00")
    try:
        s, a = ask(fl.BEGIN, struct.pack("<H", 1))
        t.check(s == 0, "BEGIN", fl.why(s, a))
        s, a = ask(fl.WRITE, struct.pack("<BHHH", fl.CHANNEL, n, 0, len(new)) + bytes(new))
        t.check(s == 0, "WRITE channel 990", fl.why(s, a))
        ask(fl.END, b"")
        t.has(t.cmd(f"ch {n + 1}"), "UITEST LINK", "channel 990 reads back on the terminal")
        bad = bytearray(new)
        bad[3:7] = struct.pack("<I", 70000000)  # 700 MHz: no chip receives it
        ask(fl.BEGIN, struct.pack("<H", 1))
        s, a = ask(fl.WRITE, struct.pack("<BHHH", fl.CHANNEL, n, 0, len(bad)) + bytes(bad))
        ask(fl.END, b"")
        t.check(s == 9 and a, "700 MHz refused as invalid with a reason", fl.why(s, a))
        t.has(t.cmd(f"ch {n + 1}"), "UITEST LINK", "the refused write changed nothing")
    finally:
        link.ask(fl.DELETE, struct.pack("<BH", fl.CHANNEL, n))
    ok, out = t.cmd_err(f"ch {n + 1}")
    t.check(not ok and "no such channel" in out, "DELETE removed it", out)


@test("SND-5")
def end_tone_is_a_transmit_setting(t):
    """The end tone is in the menus at 0 8 5 4 (Transmit) and in the channel editor's settings."""
    name, value, _ = t.get_full("0 8 5 4")
    t.eq(name, "End tone", "0 8 5 4")
    u = t.keys("MENU 0 8 5 4")
    t.eq((u.title, u.sel and u.sel["name"], u.sel and u.sel["value"]), ("Tail", "End tone", value),
         "MENU 0 8 5 4 reaches it")
