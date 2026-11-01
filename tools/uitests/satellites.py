"""Satellite mode (chapter 28a: SAT-*): the pass list filling in as it predicts, the alarm picker, the pass page's
keys and the pointing page, by the readback's `sat`, `row` and `pass` lines. Needs satellites: the simulator's
sample (data.ferritefw.dev's fixture in its store, a fixed day when its clock isn't set); on the radio the
satellites Console or `:sat add` loaded (SAT-2).
"""
import time

from .core import test

MANY = ("--sats", "32")


def sat_line(u):
    return u["sat"] or {}


def open_list(t):
    t.home()
    u = t.keys("MENU 0 3 7")
    t.eq(u.id, "SatList", "MENU 0 3 7 opens the pass list (SAT-1)")
    t.need(sat_line(u).get("state") not in ("no-sats", "no-clock", "no-position"), f"satellite mode can't predict here: {sat_line(u).get('state')}")
    return u


def ready(t):
    return t.wait(lambda u: sat_line(u).get("state") == "ready" and int(sat_line(u).get("passes", 0)) > 0, timeout=30,
                  what="the passes predicted")


@test("SAT-1", "SAT-14", pages=("SatList", "Group"))
def satellite_mode_takes_and_gives_back(t):
    """MENU 0 3 7 opens the pass list with satellite mode running (both chips: watch and scans pause); BACK leaves
    it and normal receive comes back."""
    u = open_list(t)
    t.eq(u.title, "Passes", "the status bar's name")
    t.eq(u.state.get("mode"), "sat", "satellite mode runs while the screen is open")
    u = t.keys("BACK")
    t.check(u.id != "SatList", "BACK leaves", u.id)
    u = t.home()
    t.check(u.state.get("mode") != "sat", "satellite mode stopped at home", u.state.get("mode"))


@test("SAT-6", pages=("SatList", "SatPass"), sim_args=MANY)
def pass_list_fills_in_and_stays_usable(t):
    """While the passes are predicted the list fills in (`Predicting 7 of 19`), each satellite's passes joining in
    rise order; the page answers keys meanwhile (a pass opens, BACK comes back) and the selection stays on its pass
    as rows arrive."""
    u = open_list(t)
    seen = []
    busy = False
    deadline = time.time() + 40
    moved = opened = False
    while time.time() < deadline:
        s = sat_line(u)
        if s.get("state") == "busy":
            busy = True
            seen.append(int(s.get("passes", 0)))
            sel = next((r for r in u.rows if r.get("sel")), None)
            if not moved and int(s.get("passes", 0)) >= 3:
                u = t.keys("DOWN")
                moved = True
                sel2 = next((r for r in u.rows if r.get("sel")), None)
                t.check(sel2 is not None and sel2 != sel, "DOWN moves the selection mid-prediction", (sel, sel2))
                continue
            if moved and not opened:
                u = t.keys("MENU")
                t.eq(u.id, "SatPass", "OK opens the pass mid-prediction")
                u = t.keys("BACK")
                t.eq(u.id, "SatList", "BACK to the list")
                opened = True
                continue
        elif s.get("state") == "ready":
            break
        u = t.ui()
    t.check(busy, "the prediction was seen running", seen[:5])
    t.check(len(set(seen)) >= 3 and seen == sorted(seen), "rows arrived as it ran, never fewer", seen[:20])
    u = ready(t)
    t.check(int(sat_line(u)["passes"]) >= 30, "32 satellites' passes", sat_line(u))
    t.check(moved and opened, "keys answered while predicting", (moved, opened))


@test("SAT-12", "SAT-20", pages=("SatList", "SatAlarm"))
def alarm_picker_default_and_last_choice(t):
    """# on a pass opens the alarm picker on 5 minutes; OK sets it (a bell and its lead on the row, the status
    bar's mark); the last choice is the next pass's default; Off clears it."""
    open_list(t)
    u = ready(t)
    u = t.keys("DOWN")
    u = t.keys("HASH")
    t.eq(u.id, "SatAlarm", "# opens the picker")
    t.eq(sat_line(u).get("picker"), 3, "5 minutes to start (the third choice)")
    u = t.keys("UP")
    t.eq(sat_line(u).get("picker"), 2, "UP: 2 minutes")
    u = t.keys("MENU")
    t.eq(u.id, "SatList", "OK sets it")
    t.eq(sat_line(u).get("alarm"), 2, "a 2-minute alarm")
    sel = next((r for r in u.rows if r.get("sel")), {})
    t.check("bell=2" in sel.get("value", ""), "the row's bell and lead", sel)
    t.check("sat" in u.bar, "the status bar's satellite mark (SAT-20)", u.bar)
    u = t.keys("DOWN")
    u = t.keys("HASH")
    t.eq(sat_line(u).get("picker"), 2, "the last choice is the default")
    u = t.keys("BACK")
    u = t.keys("UP")
    u = t.keys("HASH")
    t.eq(sat_line(u).get("picker"), 2, "the pass with the alarm: its own lead")
    u = t.keys("DOWN DOWN DOWN")
    t.eq(sat_line(u).get("picker"), 5, "Off is the last choice")
    u = t.keys("MENU")
    t.eq(sat_line(u).get("alarm"), 0, "Off clears the alarm")
    t.check("sat" not in u.bar, "the mark goes with it", u.bar)


@test("SAT-8", "SAT-11", "SAT-17", pages=("SatList", "SatPass", "SatPoint"))
def pass_page_trim_duplex_and_pointing(t):
    """On a pass: ◀▶ trims the downlink 100 Hz a step (kept with the satellite), * switches full and half duplex,
    0 the pointing page and back, BACK the list."""
    open_list(t)
    ready(t)
    u = t.keys("MENU")
    t.eq(u.id, "SatPass", "OK opens the pass")
    p = u["pass"] or {}
    trim, hd = int(p.get("trim", 0)), p.get("hd")
    u = t.keys("RIGHT")
    t.eq(int((u["pass"] or {}).get("trim", 0)), trim + 100, "▶ trims 100 Hz up")
    u = t.keys("LEFT")
    t.eq(int((u["pass"] or {}).get("trim", 0)), trim, "◀ back")
    u = t.keys("STAR")
    t.check((u["pass"] or {}).get("hd") != hd, "* switches full and half duplex", (hd, u["pass"]))
    u = t.keys("STAR")
    t.eq((u["pass"] or {}).get("hd"), hd, "* again: as it was")
    u = t.keys("0")
    t.eq(u.id, "SatPoint", "0: the pointing page")
    u = t.keys("0")
    t.eq(u.id, "SatPass", "0 again: the plot")
    u = t.keys("BACK")
    t.eq(u.id, "SatList", "BACK: the list")


@test("SAT-22", pages=("SatList", "SatPass", "SatOptions"))
def options_page_per_satellite(t):
    """Hold MENU on a pass: the satellite's options (Doppler, uplink power Low to start, duplex, transponder,
    trim); OK on a row changes it, kept with the satellite (the pass line says so); BACK returns to the page."""
    open_list(t)
    ready(t)
    u = t.keys("MENU")
    t.eq(u.id, "SatPass", "OK opens the pass")
    u = t.press("MENU", long=True)
    t.eq(u.id, "SatOptions", "hold MENU: the satellite's options")
    t.eq(u.names()[:5], ["Doppler", "Uplink power", "Duplex", "Transponder", "Trim"], "the rows")
    t.eq(u.num(2)["value"], "Low", "the uplink at Low to start")
    u = t.keys("2 MENU")
    t.eq(u.num(2)["value"], "Mid", "OK: Mid")
    u = t.keys("1 MENU")
    t.eq(u.num(1)["value"], "70 cm only", "Doppler on the 70 cm side only")
    u = t.keys("BACK")
    t.eq(u.id, "SatPass", "BACK: the pass page")
    p = u["pass"] or {}
    t.eq((p.get("power"), p.get("uhf_only")), (1, 1), "kept with the satellite")
    u = t.press("MENU", long=True)
    u = t.keys("1 MENU 2 MENU MENU BACK")
    p = u["pass"] or {}
    t.eq((p.get("power"), p.get("uhf_only")), (0, 0), "back as they were")


@test("SAT-2", "SAT-3", pages=("SatList",))
def sat_command_lists_and_deletes(t):
    """`sat` lists the satellites held (place, name, catalogue number); `sat add` is the terminal's (pasting
    needs the terminal itself, not the link); `sat del N` deletes one and the pass list follows."""
    out = t.cmd("sat")
    t.check("ISS #25544" in out and "SO-50 #27607" in out, "the sample's satellites", out)
    ok, why = t.cmd_err("sat add")
    t.check(not ok and "not over the link" in why, "sat add is refused over the link", why)
    n = sum(1 for ln in out.splitlines() if "#" in ln)
    out = t.cmd(f"sat del {n}")
    t.check("deleted" in out, "sat del", out)
    out = t.cmd("sat")
    t.eq(sum(1 for ln in out.splitlines() if "#" in ln), n - 1, "one fewer")
    u = open_list(t)
    u = ready(t)
    t.eq(sat_line(u).get("all"), n - 1, "the pass list predicts the satellites held")
