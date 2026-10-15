"""First-run setup by remote keys (chapter 4: SETUP-*) and the factory reset (BOOT-14, SAFE-9, SAFE-14), read
back with setup's `setup` line, its rows and the summary's note.

"Run setup again" (MENU 0 9 8 1) goes through every page with OK, keeping what the radio has. The reset wipes
the settings, the PIN, channels, zones, presets, the home channel, messages, the event log and waypoints: on the
radio it runs only with --with reset, and the test writes the `dump` back after (settings, zones, presets,
channels, the home channel); messages, the log, waypoints and FM presets aren't in a dump and stay gone.
"""
import time

from .core import test

REGIONS = 8


def setup_line(u):
    return u["setup"] or {}


def ok_through(t, u, region=None):
    """OK on each page from here to the summary (on the region page, OK on `region` if given, else the focused
    row); returns the summary's readback."""
    for _ in range(10):
        s = setup_line(u)
        if s.get("page") == s.get("of"):
            return u
        if s.get("head") == "Region" and region is not None:
            names = [r["name"] for r in u.rows][:REGIONS]
            t.check(region in names, f"region {region!r} on the page", names)
            cur = next((i for i, r in enumerate(u.rows) if r["sel"]), 0)
            want = names.index(region)
            if want != cur:
                u = t.keys(" ".join(["DOWN" if want > cur else "UP"] * abs(want - cur)))
        u = t.keys("MENU")
    t.fail("setup didn't reach its summary")


@test("SETUP-3", "SETUP-10", pages=("Home", "Setup"))
def setup_again_ok_through(t):
    """Run setup again (MENU 0 9 8 1): eight pages, OK on each keeps a working radio; the region page lists all
    eight regions on one screen (and Extend TX after them); the summary shows the transmit state (region,
    Extend TX, callsign), the channels and how to add them; OK there goes home with every setting as it was."""
    before = [x for x in t.cmd("dump").splitlines() if not x.startswith("#")]
    region = t.get("0 9 4")
    u = t.keys("MENU 0 9 8 1")
    t.eq(u.id, "Setup", "MENU 0 9 8 1")
    t.eq((setup_line(u)["page"], setup_line(u)["of"]), (1, 8), "page 1 of 8")
    pages = []
    for _ in range(8):
        s = setup_line(u)
        pages.append(s["head"])
        if s["head"] == "Region":
            names = [r["name"] for r in u.rows]
            t.eq(len(names), REGIONS + 1, "eight regions and Extend TX, on one page")
            t.eq(names[-1], "Extend TX", "Extend TX after the regions")
            chosen = [r["name"] for r in u.rows if r["value"] == "chosen"]
            t.eq(chosen, [region], "the radio's region is the one chosen")
        if s["page"] == s["of"]:
            break
        u = t.keys("MENU")
    t.eq(pages, ["Welcome to Ferrite", "Your callsign", "Region", "Owner card", "Time", "Look", "Battery Save",
                 "All set"], "the eight pages")
    rows = {r["name"]: r["value"] for r in u.rows}
    t.eq(list(rows), ["Callsign", "Region", "Extend TX", "Time zone", "Home", "Battery Save", "Channels"],
         "the summary's rows")
    t.eq(rows["Region"], region, "the region")
    note = " ".join((u["help"] or {}).get("args", [])[1:])
    t.check("Add channels" in note and "MENU 0 9 8 1" in note, "how to add channels, and how to run it again", note)
    if rows["Callsign"] == "none":
        t.check("No callsign" in note, "no callsign: automatic transmissions stay off", note)
    u = t.keys("MENU")
    t.eq(u.id, "Home", "OK on the summary: home")
    after = [x for x in t.cmd("dump").splitlines() if not x.startswith("#")]
    t.eq(after, before, "every setting as it was")


def about_rows(t):
    t.home()
    u = t.keys("MENU 0 9 9")
    a = {r["name"]: r["value"] for r in u.rows}
    t.home()
    u = t.keys("MENU 0 9 7 1 MENU")
    a.update({"cal " + r["name"]: r["value"] for r in u.rows})
    t.home()
    u = t.keys("MENU 0 9 6 MENU")
    a.update({"ssb " + r["name"]: r["value"] for r in u.rows})
    t.home()
    return a


@test("BOOT-14.4", "BOOT-14.5", "SAFE-9.1", "SAFE-14.5", "SETUP-3", needs=("reset",), pages=(
    "Confirm", "Group", "Home", "Info", "Setup"))
def reset_then_setup(t):
    """The reset (MENU 0 9 8 2: OK, hold # 3 s) by remote keys: setup's first page follows, with no region until
    setup picks one (none chosen on the region page); VOX, KISS and the digipeater read off; calibration (Valid,
    the CRC at install matching) and the SSB patch (its state) as before. Setup is then run with the radio's
    region and the dump written back; the dump after matches the one before."""
    t.need(t.get("0 9 3 1") == "Off", "a PIN lock is on")
    d0 = t.cmd("dump")
    region = t.get("0 9 4")
    a0 = about_rows(t)
    t.keys("MENU 0 9 8 2")
    u = t.ui()
    t.eq(u.id, "Confirm", "MENU 0 9 8 2: the reset's question")
    t.keys("MENU")
    t.r.down("HASH")
    try:
        time.sleep(3.4)
    finally:
        t.r.up("HASH")
    u = t.wait(lambda u: u.id == "Setup", timeout=15, what="setup after the reset")
    t.eq(setup_line(u)["page"], 1, "setup's first page (BOOT-14.5)")
    t.check(t.get("0 9 4") in ("None", "Not set", "-", ""), "no region until setup picks one", t.get("0 9 4"))
    for path, name in (("0 8 2 1", "VOX"), ("0 5 9 1", "KISS"), ("0 5 8 1", "digipeater")):
        t.eq(t.get(path), "Off", f"{name} off after the reset (SAFE-14.5)")
    u = t.keys("MENU MENU")
    t.eq(setup_line(u)["head"], "Region", "the region page")
    t.eq([r["name"] for r in u.rows if r["value"] == "chosen"], [], "none chosen until OK on one (SETUP-3)")
    u = ok_through(t, u, region=region)
    t.eq({r["name"]: r["value"] for r in u.rows}["Region"], region, "the region picked")
    t.keys("MENU")
    t.home()
    a1 = about_rows(t)
    for k in ("Calibration", "cal Status", "cal CRC at install", "SSB patch", "ssb State"):
        if k in a0:
            t.eq(a1.get(k), a0[k], f"{k} as before the reset (BOOT-14.4, SAFE-9.1)")
    bad = []
    for line in d0.splitlines():
        if not line.strip() or line.startswith("#") or line.strip() == "end":
            continue
        ok, why = t.cmd_err(line.split("#")[0].strip() if line.startswith("set ") else line)
        if not ok:
            bad.append(f"{line}: {why}")
    t.eq(bad, [], "the dump written back, every line taken")
    # What the reset seeded that the dump before didn't have (load merges: it deletes nothing).
    keep = {tuple(x.split()[:2]) for x in d0.splitlines() if x.split()[:1] in (["ch"], ["preset"], ["zone"])}
    extra = [tuple(x.split()[:2]) for x in t.cmd("dump").splitlines()
             if x.split()[:1] in (["ch"], ["preset"], ["zone"]) and tuple(x.split()[:2]) not in keep]
    for kind, n in extra:
        if kind == "preset":
            continue
        ok, why = t.cmd_err(f"{kind} {n} delete")
        t.check(ok, f"{kind} {n} (seeded by the reset) deleted", why)
    # Range presets have no terminal delete: by keys on the list (MENU 0 1 7), * then OK, the last first.
    presets = [int(x.split()[1]) for x in t.cmd("dump").splitlines() if x.startswith("preset ")]
    for kind, n in sorted((e for e in extra if e[0] == "preset"), key=lambda e: -int(e[1])):
        t.home()
        t.keys("MENU 0 1 7")
        i = presets.index(int(n))
        u = t.keys(" ".join(["DOWN"] * i + ["STAR"]))
        t.eq(u.id, "Confirm", f"* on preset {n}: delete it?")
        t.keys("MENU")
        presets.remove(int(n))
        t.check(f"preset {n} " not in t.cmd("dump"), f"preset {n} (seeded by the reset) deleted", n)
    t.put_back([])
    d1 = t.cmd("dump")
    t.eq([x for x in d1.splitlines() if not x.startswith("#")], [x for x in d0.splitlines() if not x.startswith("#")],
         "the dump after is the one before")
