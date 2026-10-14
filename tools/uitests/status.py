"""The status bar and the Status page (chapter 8, SBAR-*), About and its help pages (HELP-*)."""
from .core import test

BAR_KEYS = {"lock", "scan", "watch", "inbox", "alert", "gps", "bt", "battery", "low", "cutoff", "clock", "utc"}


@test("SBAR-14", "PWR-8", "SBAR-10")
def status_page_lists_states(t):
    """The Status page lists the states in words and follows them (Battery Save, GPS, VOX, Key lock); Battery
    Save shows there and never in the status bar."""
    u = t.keys("MENU 0 0")
    for name in ("Transmit", "Region", "Watch", "Scan", "Battery Save", "GPS", "Bluetooth", "Battery", "Key lock",
                 "VOX", "Weather alerts"):
        t.check(u.row(name) is not None, f"a {name} row", u.names())
    t.eq(u.row("Battery Save")["value"], t.get("0 7 7"), "Battery Save follows the setting")
    t.check(u.row("Transmit")["value"], "the transmit state in words", u.row("Transmit"))
    t.check(u.row("Battery")["value"].endswith("%"), "the battery as a percentage", u.row("Battery"))
    t.set("0 7 7", "Off")
    u = t.ui()
    t.eq(u.row("Battery Save")["value"], "Off", "Battery Save off: the row follows")
    t.set("0 7 7", "Level 2")
    u = t.ui()
    t.eq(u.row("Battery Save")["value"], "Level 2", "Level 2: the row follows")
    t.eq(set(u.bar) - {"args"}, BAR_KEYS, "the status bar carries nothing for it")
    if t.sim:
        t.note("GPS off: the simulator's GPS ignores 0 6 1 (radio only)")
        return
    t.set("0 6 1", "Off")
    u = t.ui(0.5)
    t.check(u.row("GPS")["value"].startswith("Off"), "GPS off: the row", u.row("GPS"))
    t.eq(u.bar["gps"], "off", "GPS off: no GPS icon")


@test("SBAR-15", "SBAR-13")
def status_rows_open_their_setting(t):
    """OK on a Status row opens that row's setting: Battery Save, VOX, GPS; BACK returns to the row."""
    for name, want in (("Battery Save", "Battery Save"), ("GPS", "GPS"), ("VOX", "VOX")):
        t.home()
        u = t.keys("MENU 0 0")
        i = u.names().index(name)
        u = t.keys(" ".join(["DOWN"] * i) + " MENU" if i else "MENU")
        t.check(u.id in ("Group", "Quick", "Value", "Gps"), f"{name}: a setting opens", u.id)
        if u.id != "Gps":  # the GPS row opens the GPS page (on/off, fix, position)
            t.eq(u.sel["name"] if u.sel else (u["value"] or {}).get("args", [None])[0], want, f"{name}: on its setting")
        u = t.keys("BACK")
        t.eq((u.id, u.sel and u.sel["name"]), ("Status", name), f"{name}: BACK to the row")


@test("SBAR-11", "SBAR-4")
def bar_shows_lock_and_scan(t):
    """States that change what a key does are in the bar: the padlock while locked, SCN while the active slot
    scans; each appears only when it applies."""
    u = t.ui()
    t.eq((u.bar["lock"], u.bar["scan"]), (0, 0), "nothing at rest")
    t.keys("SIDE2")
    u = t.ui(0.3)
    t.eq(u.bar["scan"], 1, "SIDE2 scan: SCN")
    t.keys("SIDE2")
    t.home()
    t.eq(t.ui(0.3).bar["scan"], 0, "scan stopped: no SCN")
    u = t.press("HASH", long=True)
    t.eq(u.bar["lock"], 1, "locked: the padlock")
    u = t.press("HASH", long=True)
    t.eq(u.bar["lock"], 0, "unlocked")


@test("SBAR-19", "SBAR-9")
def battery_icon_status_about(t):
    """The battery: an icon in the bar (always), the percentage on Status, the volts on About; no "Battery as"
    anywhere in Display."""
    u = t.ui()
    t.check(isinstance(u.bar.get("battery"), int), "the bar always has the battery", u.bar)
    u = t.keys("MENU 0 0")
    t.check(u.row("Battery")["value"].endswith("%"), "Status: %", u.row("Battery"))
    t.home()
    u = t.keys("MENU 0 9 9")
    t.check(u.row("Battery")["value"].endswith("V"), "About: volts", u.row("Battery"))
    for path in ("MENU 0 7", "MENU 0 7 3"):
        t.home()
        u = t.keys(path)
        t.check(not any("Battery as" in n for n in u.names()), f"{path}: no Battery as", u.names())


@test("HELP-1", "SBAR-5")
def about_pages_go_round(t):
    """◀▶ on About go round its pages: About, Manual, Keys, Side+PTT, Hold 0-9, Menu nos, Self-test; the hint
    bar names the page ▶ goes to; every name fits 8 characters; BACK leaves."""
    u = t.keys("MENU 0 9 9")
    pages = [u]
    for _ in range(7):
        pages.append(t.keys("RIGHT"))
    titles = [p.title for p in pages]
    t.eq(titles, ["About", "Manual", "Keys", "Side+PTT", "Hold 0-9", "Menu nos", "Checks", "About"], "the pages")
    t.check(all(len(x) <= 8 for x in titles), "names fit 8 characters", titles)
    odd = []
    for cur, nxt in zip(pages, pages[1:]):
        label = cur.hint("<>")
        if label not in (nxt.name, nxt.title):
            odd.append(f"{cur.title}: hint {label!r}, next {nxt.name!r}")
    # About's hint reads "Help" for the Manual page (the help pages' first): the only one not named after it.
    t.check(odd in ([], ["About: hint 'Help', next 'Manual'"]), "each hint names the next page", odd)
    if odd:
        t.note(odd[0])
    u = t.keys("LEFT")
    t.eq(u.title, "Checks", "◀ goes back")
    u = t.keys("BACK")
    t.eq((u.id, u.sel and u.sel["name"]), ("Group", "About"), "BACK leaves to System, on About")


@test("BOOT-11")
def about_rows(t):
    """About: version, MCU, SPI flash, calibration, the transmit state, self-test, battery, MCU temp, uptime,
    SSB patch (installed or the path to install it)."""
    u = t.keys("MENU 0 9 9")
    want = ["Version", "MCU", "SPI flash", "Calibration", "Transmit", "Self-test", "Battery", "MCU temp", "Uptime",
            "SSB patch"]
    t.eq(u.names(), want, "the About rows")
    t.check(u.row("Version")["value"].startswith("v"), "the version", u.row("Version"))
    t.check(u.row("Transmit")["value"], "the transmit state", u.row("Transmit"))
    t.check(u.row("SSB patch")["value"], "the SSB patch state", u.row("SSB patch"))
    t.note(f"About: {[(r['name'], r['value']) for r in u.rows]}")
