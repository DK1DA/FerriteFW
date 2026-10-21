"""Settings' ranges, help lines and what the screens say about them (chapters 12, 15, 18, 31, 33, 35, 36): walked
with the knob on their menu rows and read back. The runner puts every setting back after each test (`dump`)."""
import time

from .core import test


def walk(t, path, steps, start_low=60):
    """The values the menu row at `path` (MENU 0 ...) takes, from its lowest (`start_low` knob steps down) up,
    `steps` steps; the readback's value each time."""
    u = t.keys("MENU " + path)
    name = u.sel["name"]
    u = t.knob(-start_low)
    seen = [u.sel["value"]]
    for _ in range(steps):
        u = t.knob(1)
        seen.append(u.sel["value"])
    t.check(u.sel["name"] == name, "still on the same row", u.sel)
    return seen


@test("TX-30.1", pages=("Group",))
def tx_timeout_range(t):
    """The TX timeout (0 8 1 1) is always on: 30 s to 600 s in 30 s steps, nothing under 30 s and no Off."""
    seen = walk(t, "0 8 1 1", 21)
    t.eq(seen[0], "30 s", "the lowest is 30 s")
    want = [f"{s} s" for s in range(30, 601, 30)]
    t.eq(seen[:20], want, "30 s steps up to 600 s")
    t.check(set(seen[20:]) == {"600 s"}, "600 s is the top", seen[20:])
    t.check(not any(v.lower() == "off" for v in seen), "no Off", seen)


@test("VOX-14.3", pages=("Group",))
def vox_level_help(t):
    """0 8 2 2's help line says a higher VOX level needs a louder voice."""
    u = t.keys("MENU 0 8 2 2")
    t.eq(u.sel["name"], "Level", "MENU 0 8 2 2")
    h = u["help"] or {}
    text = " ".join(h.get("args", []))
    t.check("louder" in text.lower() and "higher" in text.lower(), "the help: higher needs a louder voice", text)


@test("SQL-12.3", pages=("Group", "Home"))
def s_meter_styles(t):
    """The S-meter style (0 7 3 1) is Bars or dBm, nothing else; the cards' signal follows it."""
    ok, why = t.cmd_err(f"set 0 7 3 1 {t.get('0 7 3 1')}")
    t.need(ok, f"the S-meter style isn't settable yet: {why}")
    u = t.keys("MENU 0 7 3 1")
    t.eq(u.sel["name"], "S-meter", "MENU 0 7 3 1")
    seen = []
    for _ in range(4):
        seen.append(u.sel["value"])
        u = t.knob(1)
    t.eq(sorted(set(seen)), ["Bars", "dBm"], "only Bars and dBm")
    for style in ("dBm", "Bars"):
        t.set("0 7 3 1", style)
        u = t.home()
        s = u.active["s"]
        t.check(s.endswith("dBm") if style == "dBm" else s.startswith("S"), f"{style}: the card's signal", s)


@test("SQL-12.1", pages=("Home", "Squelch"))
def squelch_screen_floor(t):
    """SIDE1 + knob on a quiet channel: the squelch screen shows the level, the live signal and, once read, the
    noise floor beside it ("Noise floor -125 dBm"), smoothed: two readings a moment apart stay close."""
    stopped = t.stop_scans()
    t.cmd("slot A vfo 146.55")
    t.r.down("SIDE1")
    try:
        t.r.knob(1)
        t.r.knob(-1)
        u = t.wait(lambda u: (u["squelch"] or {}).get("floor"), timeout=5, what="the noise floor read")
        f1 = int(u["squelch"]["floor"].split()[2])
        time.sleep(1.0)
        u = t.ui()
        sq = u["squelch"]
        t.eq(u.id, "Squelch", "SIDE1 held: the squelch screen")
        f2 = int(sq["floor"].split()[2])
        t.check(abs(f2 - f1) <= 6, "the floor moves smoothly (an eighth of each reading)", (f1, f2))
        t.check(sq["s"], "the live signal beside it", sq)
        t.note(f"floor {f1} then {f2} dBm, signal {sq['s']}")
    finally:
        t.r.up("SIDE1")
        t.put_back(stopped)


@test("TX-12.1", pages=("Home", "Quick"))
def power_shows_level_names(t):
    """Power shows as its level name only: LOW, MID or HIGH on the cards and the quick menu's TX power row (no
    watts); on 1.25 m the capped power itself, "0.25 W" (TX-9). Slot A made active first: the cards read are the
    active slot's (a run that starts on B read B's LOW on 1.25 m)."""
    t.cmd("slot A vfo 146.52 active")
    for i in range(3):
        u = t.home()
        p = u.active["power"]
        t.check(p in ("LOW", "MID", "HIGH"), "the card: a level name", p)
        u = t.keys("MENU")
        t.check(u.row("TX power")["value"] in ("Low", "Mid", "High"), "the quick menu: a level name",
                u.row("TX power"))
        t.home()
        t.press("2", long=True)  # hold 2: the next power level (KEY-20)
    t.cmd("slot A vfo 223.5")
    u = t.home()
    # HOME-4.2: the card says "No TX" while 1.25 m is locked (TX-10, until the bench measures the cap).
    t.check(u.active["power"] in ("0.25 W", "No TX"), "1.25 m: the capped power, or No TX while locked", u.active["power"])
    u = t.keys("MENU")
    t.eq(u.row("TX power")["value"], "0.25 W", "and the quick menu says it")


@test("DISP-16.3", pages=("Home",))
def quick_menu_knob_is_brightness(t):
    """Hold * on home: the quick menu; two knob steps up raise Brightness (0 7 4 2) by two."""
    t.set("0 7 4 2", "5")
    u = t.press("STAR", long=True)
    t.check(u.qmenu, "hold *: the quick menu", u.lines)
    t.knob(1)
    u = t.knob(1)
    t.eq(t.get("0 7 4 2"), "7", "two steps up from 5")
    knob = [q for q in u.qmenu if q[:2] == ["Knob", "Brightness"]]
    t.eq(knob and knob[0][2], "7", "the quick menu shows it")


@test("DISP-15.1", slow=True, pages=("Home",))
def backlight_timeout_and_wake(t):
    """The backlight goes off after the timeout (10 s here): `dark=1`; a remote key wakes it, and a remote knob step
    wakes it and tunes too (decision 162: the PC can't see the screen is dark, so a key from the link acts);
    "Never" (always on) never goes dark. A key on the radio that only wakes it can't be driven from here: a hand
    check (the host tests have it)."""
    t.set("0 7 4 1", "10 s")
    t.set("0 7 4 4", "Keys")  # nothing heard wakes it meanwhile (the simulator's slots hear a lot)
    t.home()
    u = t.wait(lambda u: u.state.get("dark") == 1, timeout=16, what="dark after 10 s", every=1.0)
    u = t.keys("BACK")
    t.eq((u.state["dark"], u.id), (0, "Home"), "a remote key wakes it")
    u = t.wait(lambda u: u.state.get("dark") == 1, timeout=16, what="dark again", every=1.0)
    f0 = u.active["freq"]
    u = t.knob(1)
    t.eq(u.state["dark"], 0, "a remote knob step wakes it")
    t.check(u.active["freq"] != f0, "and tunes, as from the link (decision 162)", (f0, u.active["freq"]))
    t.knob(-1)
    t.set("0 7 4 1", "Never")
    t.home()
    time.sleep(13)
    t.eq(t.ui().state["dark"], 0, "Never: never dark")


@test("PWR-14", "CAL-18", pages=("Group", "Info"))
def trims_are_clamped(t):
    """The battery reading trim (0 9 7 3) stops at ±0.30 V and About's volts follow it; the crystal trim (0 9 7 2)
    at ±20 ppm. (Skipped while the trims "can't be set yet".)"""
    ok, why = t.cmd_err("set 0 9 7 3 +0.00 V")
    t.need(ok, f"the trims aren't settable yet: {why}")
    u = t.keys("MENU 0 9 9")
    v0 = float(u.row("Battery")["value"].split()[0])
    hi = walk(t, "0 9 7 3", 40)
    t.eq((hi[0], hi[-1]), ("-0.30 V", "+0.30 V"), "battery trim: -0.30 V to +0.30 V")
    t.set("0 9 7 3", "+0.10 V")
    t.home()
    u = t.keys("MENU 0 9 9")
    v1 = float(u.row("Battery")["value"].split()[0])
    t.check(abs(v1 - v0 - 0.10) <= 0.03, "About's volts follow the trim", (v0, v1))
    t.set("0 9 7 3", "+0.00 V")
    t.home()
    ppm = walk(t, "0 9 7 2", 60)
    t.eq((ppm[0], ppm[-1]), ("-20 ppm", "+20 ppm"), "crystal trim: ±20 ppm")
    t.set("0 9 7 2", "0 ppm")


@test("CAL-17", pages=("Group",))
def reset_trims_clears_only_trims(t):
    """"Reset trims" clears the trims only: every other setting (`dump`) as it was. (Skipped while the trims
    "can't be set yet".)"""
    ok, why = t.cmd_err("set 0 9 7 3 +0.10 V")
    t.need(ok, f"the trims aren't settable yet: {why}")
    before = [x for x in t.cmd("dump").splitlines() if not x.startswith("#") and "0 9 7" not in x]
    u = t.keys("MENU 0 9 7")
    names = u.names()
    t.need("Reset trims" in names, f"no Reset trims row in 0 9 7 ({names})")
    i = names.index("Reset trims")
    t.keys(" ".join(["DOWN"] * i) + " MENU")
    u = t.ui()
    if u.id == "Confirm":
        t.keys("MENU")
    t.eq(t.get("0 9 7 3"), "+0.00 V", "the battery trim cleared")
    after = [x for x in t.cmd("dump").splitlines() if not x.startswith("#") and "0 9 7" not in x]
    t.eq(after, before, "nothing else changed")


@test("SELF-6", pages=("Group", "Info"))
def about_without_the_patch(t):
    """Without an SSB patch About says "Not installed", and the SSB patch page (0 9 6) says how to install it:
    the flasher's --ssb-from or the web app."""
    u = t.keys("MENU 0 9 9")
    t.need(u.row("SSB patch")["value"] != "Installed", "the SSB patch is installed (needs a radio without it)")
    t.eq(u.row("SSB patch")["value"], "Not installed", "About")
    t.home()
    u = t.keys("MENU 0 9 6 MENU")
    h = " ".join((u["help"] or {}).get("args", []))
    t.check("--ssb-from" in h and "web app" in h, "the page names the way to install it", h)


@test("SELF-7", pages=("Group", "Info"))
def about_explains_the_update_route(t):
    """There is no firmware-update screen (System has no update row); About explains the stock bootloader's route
    (hold PF1 + PF2 while switching on, then the flasher)."""
    u = t.keys("MENU 0 9")
    t.check(not any("update" in n.lower() for n in u.names()), "System: no update row", u.names())
    t.home()
    u = t.keys("MENU 0 9 9")
    h = " ".join((u["help"] or {}).get("args", []))
    t.check("PF1 + PF2" in h and "switching on" in h, "About: the bootloader route", h)
