"""The link and the terminal over it (chapters 29 and 30: LINK-*, TERM-*): what the build carries and says."""
import struct

import ferrite_link as fl

from .core import test

# The bench commands (decision 151). `txtest` and `rx` are left out: on a bench build they would act.
BENCH = ["mem 0x08003000 4", "mem32 0x40021000", "gpio", "bk 0 30", "si", "bt", "mic", "gps query"]


@test("TERM-38")
def bench_commands_refused_in_a_user_build(t):
    """On the images a user installs (receive, tx) the bench commands aren't there: each is refused, saying a
    bench build has it; `help` lists them as not in this build."""
    t.need(not t.r.bench, "a bench build: its bench commands work (nothing to check here)")
    for line in BENCH:
        ok, out = t.cmd_err(line)
        t.check(not ok, f"{line}: refused", out)
        t.check("bench" in out.lower() or "not in this build" in out.lower(), f"{line}: names the bench build", out)
    h = t.cmd("help")
    t.has(h, "not in this build", "help")


@test("LINK-35", "LINK-34", pages=("Group", "Info"))
def info_and_terminal_over_the_link(t):
    """INFO gives the version (About's), the build flavour (HELLO's) and the region and callsign; CMD runs a
    terminal line on the link session and gives its output (`get` as the menu shows it)."""
    r = t.r.link.ok(fl.INFO)
    ver, rest = fl.text(r)
    chans, zones, presets, slen, region = struct.unpack("<HBBHB", rest[:7])
    call, tail = fl.text(rest[7:])
    t.eq(fl.build_name(tail), t.r.build, "INFO's build is HELLO's")
    u = t.keys("MENU 0 9 9")
    t.check(u.row("Version")["value"].startswith("v" + ver.lstrip("v")), "INFO's version is About's",
            (ver, u.row("Version")["value"]))
    t.check(slen > 16, "the settings record's size", slen)
    t.eq(call, t.get("0 8 7 1") if call else "", "the callsign")
    t.home()
    u = t.keys("MENU 0 7 1")
    t.eq(t.get("0 7 1"), u.sel["value"], "CMD get 0 7 1 is the menu's Theme")
    ok, out = t.cmd_err("nosuchcommand")
    t.check(not ok and out, "an unknown command is refused with a reason", out)
    t.note(f"INFO: {ver}, {chans} channels, {zones} zones, {presets} presets, region {region}, callsign {call!r}")


@test("LINK-26")
def settings_write_keeps_protected_fields(t):
    """A settings record written over the link sets the callsign in capitals (typed in lower case) and keeps the
    radio's region and Extend TX whatever the record says (LINK-5); the original record written back restores it."""
    link = t.r.link
    rec = link.read(fl.SETTINGS, 0)
    call0 = t.get("0 8 7 1")
    region0, ext0 = t.get("0 9 4"), t.get("0 8 9")
    # The record's first fields (persist.rs): callsign (10 bytes), owner (3 x 20), boot card, region, Extend TX.
    t.eq(rec[:10].rstrip(b"\x00").decode("ascii", "replace"), "" if call0 in ("None", "Empty") else call0,
         "the record starts with the callsign")
    new = bytearray(rec)
    new[0:10] = b"ut9zz".ljust(10, b"\x00")
    new[71] = (rec[71] + 1) % 8
    new[72] = 0 if rec[72] else 1
    try:
        link.write(fl.SETTINGS, 0, bytes(new))
        t.eq(t.get("0 8 7 1"), "UT9ZZ", "the callsign written, in capitals")
        t.eq(t.get("0 9 4"), region0, "the region kept")
        t.eq(t.get("0 8 9"), ext0, "Extend TX kept")
    finally:
        link.write(fl.SETTINGS, 0, rec)
    t.eq(t.get("0 8 7 1"), call0, "the original record written back: the callsign as it was")
