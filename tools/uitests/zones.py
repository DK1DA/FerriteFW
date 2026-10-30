"""The zones page, MENU 0 2 3 (chapter 10, CHAN-27 to CHAN-30, C11): the compact list with its chips, the preview of
the selected zone (name, place, frequency), the Nearby row only with a position and positioned channels.

Each test loads its own zone and five channels (991-995, refused if any is in use) and takes the radio somewhere no
real channel is: the GPS off (0 6 1, put back by the runner) and a fixed position (0 6 9) in the Pacific, so the
places are the same on the radio and the simulator. Everything is removed after: the channels with `ch N delete`,
the zone with \\* on the page (CHAN-12), 0 6 9 back to None.
"""
import math
import re

from .core import test

ZONE = "UITEST Z"
BASE = (0.5, -160.5)  # the fixed position: the Pacific, far from any real channel
NUMS = range(991, 996)


def toward(km, deg):
    """A position `km` from BASE toward `deg`, as the radio measures (flat earth: 111.195 km a degree)."""
    lat = BASE[0] + km * math.cos(math.radians(deg)) / 111.195
    lon = BASE[1] + km * math.sin(math.radians(deg)) / (111.195 * math.cos(math.radians((BASE[0] + lat) / 2)))
    return f"{lat:.7f},{lon:.7f}"


# number, name, MHz, position (None: none), the place the preview shows (metric, imperial)
CHANNELS = [
    (991, "UT North", "146.52", toward(2.3, 0), ("N 2.3km", "N 1.4mi")),
    (992, "UT East far", "446.5", f"{BASE[0]},{BASE[1] + 20}", ("E far", "E far")),
    (993, "UT no position", "147.0", None, ("", "")),
    (994, "UT South", "145.0", toward(123, 180), ("S 123km", "S 76.4mi")),
    (995, "UITEST LONG NAME", "439.9875", toward(23.4, 270), ("W 23.4km", "W 14.5mi")),
]


class Fixture:
    """The test zone and channels, the fixed position; `close` takes them away again."""

    def __init__(self, t):
        self.t = t
        self.zone = None
        self.made = []
        self.fixed_before = t.get("0 6 9")
        t.need(self.fixed_before == "None", f"a fixed position is set ({self.fixed_before}): not touched")
        for n in NUMS:
            ok, out = t.cmd_err(f"ch {n}")
            t.need(not ok and "no such channel" in out, f"channel {n} in use: {out.strip()}")
        ok, out = t.cmd_err(f'zone 254 "{ZONE}"')
        m = re.search(r"the next is zone (\d+)", out)
        t.need(m, f"no room for a test zone: {out.strip()}")
        self.zone = int(m.group(1))
        t.cmd(f'zone {self.zone} "{ZONE}"')
        try:
            for n, name, mhz, pos, _ in CHANNELS:
                t.cmd(f'ch {n} "{name}" {mhz} zone={self.zone}' + (f" pos={pos}" if pos else ""))
                self.made.append(n)
        except Exception:
            self.close()
            raise

    def located(self, units="Metric"):
        """No GPS fix, the fixed position standing in (GPS-3)."""
        self.t.set("0 6 1", "Off")
        self.t.set("0 6 4", units)
        self.t.cmd(f"set 0 6 9 {BASE[0]},{BASE[1]}")

    def nowhere(self):
        """No fix and no fixed position: no location anywhere."""
        self.t.set("0 6 1", "Off")
        self.t.cmd("set 0 6 9 none")

    def open(self):
        """The zones page, on the test zone (the last one)."""
        t = self.t
        t.home()
        u = t.keys("MENU 0 2 3")
        t.eq(u.id, "Zones", "MENU 0 2 3")
        for _ in range(260):
            if u.sel and u.sel.get("kind") == "zone" and u.sel["name"] == ZONE:
                return u
            u = t.keys("UP")
        t.fail("the test zone isn't in the list")

    def close(self):
        t = self.t
        notes = []
        for n in self.made:
            ok, out = t.cmd_err(f"ch {n} delete")
            if not ok:
                notes.append(f"ch {n} delete: {out}")
        self.made = []
        if self.zone is not None:
            # CHAN-12: \\* asks; OK deletes the zone (its channels are gone already)
            try:
                u = self.open()
                u = t.keys("STAR")
                if u.id == "Confirm":
                    u = t.keys("MENU")
                gone = not any(r["name"] == ZONE for r in t.ui().rows)
                if not gone:
                    notes.append("the test zone is still there")
            except Exception as e:  # leave a note, not a second failure
                notes.append(f"deleting the test zone: {e}")
            self.zone = None
        if self.fixed_before == "None":
            t.cmd("set 0 6 9 none")
        for n in notes:
            t.note(n)
        t.home()


def zone_row(u, name):
    return next((r for r in u.rows if r.get("kind") == "zone" and r["name"] == name), None)


def nearby(u):
    return next((r for r in u.rows if r.get("kind") == "nearby"), None)


@test("CHAN-27", "CHAN-30", pages=("Confirm", "Zones"))
def zones_list_rows_and_chips(t):
    """One line per zone: name, chips, channel count; SCAN and HID chips from the zone's settings; the active slot's
    chip only on the zone its channel is in (none for a VFO); the hint bar says OK Use, SLOT Edit, # New (decision 178)."""
    f = Fixture(t)
    try:
        u = f.open()
        r = zone_row(u, ZONE)
        t.eq((r["value"], r["chips"]), ("5", ""), "the test zone: 5 channels, no chips")
        t.check(re.match(r"\d+ zones? · \d+ ch$", u["head"]["args"][0]), "the header counts zones and channels", u["head"])
        t.eq(u.hints, [("OK", "Use"), ("SLOT", "Edit"), ("#", "New")], "the hint bar")
        t.cmd(f'zone {f.zone} "{ZONE}" scan=on shown=off')
        u = t.ui()
        t.eq(zone_row(u, ZONE)["chips"], "SCAN HID", "in scan all and hidden from ◀▶")
        # the active slot's chip: only its zone, only when it is on a channel
        active = u.state.get("active", "A")
        line = t.slot(active)
        m = re.search(r"\bch (\d+)", line)
        with_slot = [r["name"] for r in u.rows if r.get("kind") == "zone" and re.search(r"\b[ABC]\b", r.get("chips", ""))]
        if m:
            zm = re.search(r"zone=(\d+)", t.cmd(f"ch {m.group(1)}"))
            t.need(zm, f"the active slot's channel {m.group(1)} is in no zone")
            zline = t.cmd(f"zone {zm.group(1)}")
            zname = re.search(r'"([^"]*)"', zline).group(1)
            t.check(with_slot in ([zname], []), f"only {zname} carries the slot chip (or it is off this page)", with_slot)
            if with_slot:
                t.check(active in zone_row(u, zname)["chips"].split(), f"its chip is {active}", zone_row(u, zname))
        else:
            t.eq(with_slot, [], "the active slot is a VFO: no slot chip")
    finally:
        f.close()


@test("CHAN-27", "CHAN-28", "CHAN-29", pages=("Confirm", "Zones"))
def zones_preview_with_places(t):
    """With a position: the Nearby row gives only the nearest channel's place; the preview names the zone in
    capitals with its bands, its first four channels as name, place (only those with a position) and frequency, no
    channel numbers or shift signs, then "+1 more · scan off"; in scan, the footer marks its priority channel with
    a star, a 16-character name whole (decision 157). Imperial units give miles the same way."""
    f = Fixture(t)
    try:
        f.located()
        u = f.open()
        n = nearby(u)
        t.check(n is not None, "a Nearby row with a position and positioned channels", u.lines)
        t.eq(n["value"], "N 2.3km", "Nearby: the nearest one's direction and distance only")
        t.eq(u["preview"]["args"], [ZONE.upper(), "2m · 70cm"], "the preview's header")
        want = [(name, place[0], f"{float(mhz):.3f}") for _, name, mhz, _, place in CHANNELS[:4]]
        t.eq([(r["name"], r["place"], r["freq"]) for r in u["pvrows"]], want, "the first four: name, place, frequency")
        t.eq(u["pvfoot"]["args"], ["+1 more · scan off"], "the footer")
        t.check(not any("+" in r["freq"] or "-" in r["freq"] for r in u["pvrows"]), "no shift signs", u["pvrows"])
        t.cmd(f'zone {f.zone} "{ZONE}" scan=on pri1=995')
        u = t.ui()
        t.eq(u["preview"]["args"], [ZONE.upper(), "scan on"], "in scan: the preview's header")
        t.eq(u["pvfoot"]["args"], ["+1 more · ★ UITEST LONG NAME"], "in scan: the priority channel, starred")
        t.cmd(f'zone {f.zone} "{ZONE}" scan=off pri1=none')
        f.located("Imperial")
        u = t.ui()
        t.eq(nearby(u)["value"], "N 1.4mi", "Nearby in miles")
        t.eq([r["place"] for r in u["pvrows"]], [c[4][1] for c in CHANNELS[:4]], "places in miles")
    finally:
        f.close()


@test("CHAN-29", "CHAN-15", pages=("Browse", "Confirm", "Zones"))
def zones_nearby_row_only_with_a_position(t):
    """The Nearby row: ▲ from the first zone selects it (OK opens the browser's Nearby view, nearest first) and its
    preview lists the nearest channels; with no fix and no fixed position it isn't there, ▲ wraps to the last zone,
    and the preview shows no places."""
    f = Fixture(t)
    try:
        f.located()
        t.home()
        u = t.keys("MENU 0 2 3")
        t.check(nearby(u) is not None, "the Nearby row", u.lines)
        t.check(u.sel and u.sel.get("kind") == "zone", "the page opens on a zone, not on Nearby", u.sel)
        first = u.sel["name"]
        u = t.keys("UP")
        t.check(u.sel and u.sel.get("kind") == "nearby", "▲ from the first zone: Nearby", u.sel)
        t.eq(u["preview"]["args"], ["NEARBY", "nearest first"], "its preview")
        t.eq(u["pvrows"][0]["name"], "UT North", "nearest first")
        t.eq(u.hints[0], ("OK", "Open"), "OK opens it")
        u = t.keys("MENU")
        t.eq(u.id, "Browse", "OK on Nearby: the channel browser's Nearby view")
        t.home()
        f.nowhere()
        u = t.keys("MENU 0 2 3")
        t.eq(nearby(u), None, "no fix, no fixed position: no Nearby row")
        t.eq(u.sel["name"], first, "on the first zone")
        u = t.keys("UP")
        t.check(u.sel and u.sel.get("kind") == "local" and u.sel["name"] == "Local", "▲ wraps to the last row: Local (CHAN-34)",
                u.sel)
        u = t.keys("UP")
        t.check(u.sel and u.sel.get("kind") == "zone" and u.sel["name"] == ZONE, "then the last zone (the test zone)",
                u.sel)
        u = f.open()
        t.check(all(r["place"] == "" for r in u["pvrows"]), "no places without a position", u["pvrows"])
    finally:
        f.close()


@test("CHAN-12", "CHAN-27", pages=("Zones", "Confirm"))
def zones_editor_and_delete_keep_their_keys(t):
    """SLOT on a zone opens its editor (decision 178; SLOT again or BACK returns to the list on it); \\* asks before
    deleting it (BACK keeps it, OK deletes it and its channels stay, in no zone)."""
    f = Fixture(t)
    try:
        u = f.open()
        u = t.keys("SLOT")
        t.eq(u.name, "Zone editor", "SLOT on a zone")
        t.check(any(r["name"] == "Scan all" for r in u.rows), "its rows read back", u.rows)
        u = t.keys("SLOT")
        t.eq((u.name, u.sel and u.sel["name"]), ("Zones", ZONE), "SLOT again: the list, on the zone")
        u = t.keys("SLOT BACK")
        t.eq((u.name, u.sel and u.sel["name"]), ("Zones", ZONE), "BACK: the list, on the zone")
        u = t.keys("STAR")
        t.eq(u.id, "Confirm", "* asks first")
        u = t.keys("BACK")
        t.check(zone_row(u, ZONE) is not None, "BACK keeps it", u.rows)
        t.keys("STAR")
        u = t.keys("MENU")
        t.eq(zone_row(u, ZONE), None, "OK deletes it")
        t.has(t.cmd("ch 991"), '"UT North"', "its channels stay")
        t.check("zone=" not in t.cmd("ch 991"), "in no zone", t.cmd("ch 991"))
        f.zone = None  # gone already
    finally:
        f.close()


def put_back(t, letter, before):
    """The slot as it was before a test moved it: its VFO (V/M), or its channel (OK on its zone, then ▲▼ to it)."""
    if re.search(r"\bch \d+", t.slot(letter)) and " vfo " in f" {before} ":
        t.home()
        t.keys("VM")
        return
    m = re.search(r"\bch (\d+)", before)
    if not m:
        return
    zm = re.search(r"zone=(\d+)", t.cmd(f"ch {m.group(1)}"))
    name = zm and re.search(r'"([^"]*)"', t.cmd(f"zone {zm.group(1)}"))
    if not name:
        t.note(f"slot {letter}: not put back on ch {m.group(1)} (no zone)")
        return
    t.home()
    u = t.keys("MENU 0 2 3")
    for _ in range(260):
        if u.sel and u.sel.get("kind") == "zone" and u.sel["name"] == name.group(1):
            break
        u = t.keys("DOWN")
    t.keys("MENU")
    for _ in range(260):
        if re.search(rf"\bch {m.group(1)}\b", t.slot(letter)):
            return
        t.keys("DOWN")
    t.note(f"slot {letter}: not put back on ch {m.group(1)}")


@test("CHAN-27", "NAV-1", "HOME-4.3", pages=("Zones", "Home"))
def zones_ok_puts_the_slot_on_the_zone(t):
    """OK on a zone (decision 178): the active slot goes to channel mode on the zone's first channel in number order
    (991) and the page goes home, the card's header naming the zone ("991 · UITEST Z", HOME-4.3). The slot is put
    back after."""
    f = Fixture(t)
    letter, before = None, ""
    try:
        u = f.open()
        letter = u.state.get("active", "A")
        before = t.slot(letter)
        t.need("aprs" not in before, "the active slot is the APRS slot (APRS-19)")
        u = t.keys("MENU")
        t.eq(u.id, "Home", "OK on a zone: home")
        t.check(re.search(r"\bch 991\b", t.slot(letter)), "the slot on the zone's first channel", t.slot(letter))
        t.eq(u.active.get("head"), "991 · " + ZONE, "the header names the zone")
    finally:
        if letter:
            put_back(t, letter, before)
        f.close()


@test("CHAN-34", "CHAN-34.1", "CHAN-34.4", pages=("Zones", "Home"))
def zones_local_zone(t):
    """Local (MENU 0 2 5, 50 km by default) is the zones page's last row: the channels with a position within the
    range of the fixed position, nearest first ("within 50 km"); a shorter range takes the far one out; OK puts the
    slot on the nearest; with no position it is still there, dimmed, saying "no position"."""
    f = Fixture(t)
    try:
        t.eq(t.get("0 2 5"), "50 km", "on by default, 50 km")
        f.located()
        t.home()
        u = t.keys("MENU 0 2 3 UP UP")  # Nearby, then round to the last row
        t.check(u.sel and u.sel.get("kind") == "local" and u.sel["name"] == "Local", "▲▲: Local, the last row", u.sel)
        t.eq(u.sel["value"], "2", "UT North (2.3 km) and the long name (23.4 km); 123 km and far are out")
        t.eq(u["preview"]["args"], ["LOCAL", "within 50 km"], "its preview's header")
        t.eq([r["name"] for r in u["pvrows"]], ["UT North", "UITEST LONG NAME"], "nearest first")
        t.set("0 2 5", "10 km")
        t.home()
        u = t.keys("MENU 0 2 3 UP UP")
        t.eq((u.sel["name"], u.sel["value"]), ("Local", "1"), "10 km: UT North only")
        u = t.keys("MENU")
        t.eq(u.id, "Home", "OK: the slot on Local, home")
        f.nowhere()
        t.home()
        u = t.keys("MENU 0 2 3 UP")
        t.check(u.sel and u.sel.get("kind") == "local" and u.sel.get("grey"), "no position: Local still there, dimmed", u.sel)
        t.eq(u["preview"]["args"], ["LOCAL", "no position"], "and says why")
    finally:
        t.set("0 2 5", "50 km")
        f.close()
