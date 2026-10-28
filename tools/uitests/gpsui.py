"""The GPS position page and the terminal's `gps` (chapter 28: GPS-*; TERM-36), APRS compose's limit (TEXT-5).

The position page's readback: `gps "status" age=`, `pos "big" "small"`, the six tiles as rows, `near "name"
"1.2 km NE" deg=`. On the radio the page is compared with `gps` (the module's last GGA); the simulator's GGA is a
fixed sentence unrelated to its sample fix, so there the formats are only checked against each other.
"""
import re
import time

from .core import test


def gga(t):
    """(lat, lon, alt m, utc hhmm, age s) from `gps`, or None."""
    out = t.cmd("gps")
    m = re.search(r"GGA, (\d+) s ago: \$G.GGA,(\d\d)(\d\d)\d\d(?:\.\d+)?,(\d\d)(\d\d\.\d+),([NS]),"
                  r"(\d\d\d)(\d\d\.\d+),([EW]),\d,\d+,[\d.]*,(-?[\d.]+)", out)
    if not m:
        return None
    age, hh, mm, la, lam, ns, lo, lom, ew, alt = m.groups()
    lat = (int(la) + float(lam) / 60) * (1 if ns == "N" else -1)
    lon = (int(lo) + float(lom) / 60) * (1 if ew == "E" else -1)
    return lat, lon, float(alt), f"{hh}:{mm}", int(age)


def parse_coord(s):
    """A coordinate as the page writes it, in any 0 6 3 format: signed degrees."""
    s = s.strip()
    hemi = s[-1]
    body = s[:-1].strip()
    nums = [float(x) for x in re.findall(r"\d+(?:\.\d+)?", body)]
    deg = nums[0] + (nums[1] / 60 if len(nums) > 1 else 0) + (nums[2] / 3600 if len(nums) > 2 else 0)
    return deg if hemi in "NE" else -deg


def page(t):
    t.home()
    u = t.keys("MENU 0 6 2")
    t.eq(u.id, "Gps", "MENU 0 6 2")
    return u


def need_fix(t):
    t.need(t.get("0 6 1") != "Off", "GPS is off")
    u = t.home()
    t.need(u.bar.get("gps") == "fix", "no GPS fix (outdoors: --with fix)")


@test("GPS-15.2", needs=("fix",), pages=("Gps", "Group"))
def position_in_each_format(t):
    """With a fix, the position page shows latitude and longitude large in the 0 6 3 format (decimal degrees,
    degrees and minutes, degrees minutes seconds; Grid square: the square large, degrees and minutes small under
    it): every format the same place, and (radio) the module's GGA's."""
    need_fix(t)
    # 0 6 3 is set on the radio's menu (the knob, which goes round them); the terminal can't set it yet.
    u = t.keys("MENU 0 6 3")
    start = u.sel["value"]
    places = {}
    for _ in range(6):
        f = u.sel["value"]
        if f in places:
            break
        u = page(t)
        big, small = u["pos"]["args"]
        if "grid" in f.lower():
            t.check(re.fullmatch(r"[A-R]{2}\d\d[a-x]{2}", big), f"{f}: the square, large", big)
            parts = re.split(r"\s{2,}", small)
            places[f] = (parse_coord(parts[0]), parse_coord(parts[1]))
        else:
            places[f] = (parse_coord(big), parse_coord(small))
        t.home()
        t.keys("MENU 0 6 3")
        u = t.knob(1)
    t.eq(u.sel["value"], start, "round the formats, back to the first")
    t.check(len(places) >= 3, "the formats", list(places))
    ref = next(iter(places.values()))
    for f, (la, lo) in places.items():
        t.check(abs(la - ref[0]) < 2e-4 and abs(lo - ref[1]) < 2e-4, f"{f}: the same place", (f, la, lo, ref))
    if not t.sim:
        g = gga(t)
        t.check(g, "`gps` gives the GGA", t.cmd("gps"))
        t.check(abs(g[0] - ref[0]) < 1e-3 and abs(g[1] - ref[1]) < 1e-3, "the GGA's position", (g, ref))
    t.note(f"{places}")


@test("GPS-15.3", needs=("fix",), pages=("Gps",))
def six_tiles(t):
    """With a fix, six tiles: grid square, altitude, speed, course, local time (the status bar's clock) and UTC;
    (radio) the altitude and UTC as the GGA has them."""
    need_fix(t)
    u = page(t)
    tiles = {r["name"]: r["value"] for r in u.rows}
    t.eq(list(tiles), ["Grid", "Altitude", "Speed", "Course", "Local time", "UTC"], "the six tiles")
    t.check(re.fullmatch(r"[A-R]{2}\d\d[a-x]{2}", tiles["Grid"]), "a grid square", tiles["Grid"])
    t.check(re.search(r"-?\d+ (m|ft)$", tiles["Altitude"]), "an altitude", tiles["Altitude"])
    t.check(re.search(r"\d+ (km/h|mph|kn)$", tiles["Speed"]), "a speed", tiles["Speed"])
    clock = u.bar["clock"]
    if clock != "-":
        h, m = clock.split(":")
        lt = tiles["Local time"]
        t.check(re.match(rf"0?{int(h) % 12 if 'M' in lt else int(h)}:{m}", lt) or lt.replace(" ", "").endswith(m),
                "local time as the status bar's clock", (lt, clock))
    t.check(tiles["UTC"].endswith(" Z"), "UTC, marked Z", tiles["UTC"])
    if not t.sim:
        g = gga(t)
        t.check(g, "`gps` gives the GGA", t.cmd("gps"))
        alt = float(tiles["Altitude"].split()[0]) * (0.3048 if tiles["Altitude"].endswith("ft") else 1)
        t.check(abs(alt - g[2]) <= 2, "the altitude as the GGA's (field 9)", (tiles["Altitude"], g[2]))
        t.check(tiles["UTC"].startswith(g[3]) or abs(int(tiles["UTC"][3:5]) - int(g[3][3:])) <= 1,
                "UTC as the GGA's", (tiles["UTC"], g[3]))


@test("GPS-15.4", needs=("fix",), pages=("Gps",))
def nearest_waypoint(t):
    """With a fix and a waypoint stored, the foot shows the nearest one: its name, distance and cardinal, with a
    bearing (the arrow's) that matches the cardinal."""
    need_fix(t)
    u = page(t)
    near = u["near"]["args"]
    t.need(near[0], "no waypoint stored (# on the position page marks one)")
    name, dist = near[0], near[1]
    t.check(re.fullmatch(r"[\d.]+ (m|km|ft|mi) (N|NE|E|SE|S|SW|W|NW)", dist), "distance and cardinal", dist)
    deg = u["near"]["deg"]
    card = dist.split()[-1]
    want = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"][int(((deg + 22.5) % 360) // 45)]
    t.eq(card, want, f"the cardinal for {deg}°")
    t.note(f"nearest: {name} {dist} ({deg}°)")


@test("GPS-34.6", pages=())
def gga_every_second(t):
    """After Ferrite's NMEA set, GGA still comes every second: `gps` read over a few seconds never shows one
    older than 1 s."""
    t.need(t.get("0 6 1") != "Off", "GPS is off")
    if t.sim:
        t.note("the simulator's GGA is a fixed sentence, 1 s old")
    worst = 0
    for _ in range(6):
        g = gga(t)
        t.need(g is not None or worst, "no GGA yet (no module heard)")
        worst = max(worst, g[4])
        time.sleep(0.7)
    t.check(worst <= 1, "no GGA older than 1 s", worst)


@test("TEXT-5", pages=("Compose", "Msgs"))
def compose_stops_at_67(t):
    """Compose counts against the 67 characters APRS allows ("34/67"); the key that would pass it is refused with
    "67 characters: the APRS limit" and the count stays 67/67. (The draft is put back after.)"""
    t.home()
    t.keys("MENU 0 5 2")
    u = t.keys("HASH")
    t.eq(u.id, "Compose", "# on the messages: compose")
    to0 = (u["compose"] or {}).get("to", "")
    if (u["compose"] or {}).get("field") == 0:
        # an empty recipient shows the address book (APRS-38): OK takes its first callsign; else OK is Next
        u = t.keys("OK")
    c = u["compose"]
    text0 = c["text"]
    n0 = int(c["count"].split("/")[0])
    t.eq(c["count"].split("/")[1], "67", "counted against 67")
    try:
        n = n0
        while n < 65:
            u = t.keys("2 HASH", settle=0.05)
            n = int(u["compose"]["count"].split("/")[0])
        # Different keys from here: in multi-tap (the radio may be left there, TEXT-10) the same key again within
        # a second changes the last letter (KEY-12) and adds nothing, so a second "2" was never refused.
        for k in ("3", "4"):
            if n >= 67:
                break
            u = t.keys(k, settle=0.05)
            n = int(t.ui()["compose"]["count"].split("/")[0])
        u = t.ui()
        t.eq(u["compose"]["count"], "67/67", "full")
        t.keys("5", settle=0)
        # the radio's readback can come before the toast (a pass of the UI task): wait for it
        u = t.wait(lambda u: u.toast is not None, timeout=1.0, what="the refusal's toast")
        t.eq(u.toast, {"head": "67 characters", "body": "the APRS limit"}, "the next key: refused, saying why")
        t.eq(u["compose"]["count"], "67/67", "and the count stays")
    finally:
        for _ in range(80):
            u = t.ui(0.02)
            c = u["compose"] or {}
            if c.get("text", text0) == text0 or not c:
                break
            t.keys("BACK", settle=0.02)
        u = t.ui()
        if (u["compose"] or {}).get("text") != text0:
            t.note(f"the draft left as {(u['compose'] or {}).get('text')!r}, was {text0!r}")
        # the recipient the book gave, cleared again
        if not to0 and (u["compose"] or {}).get("to"):
            t.keys("UP")
            for _ in range(len(u["compose"]["to"])):
                t.keys("BACK", settle=0.02)
