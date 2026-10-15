"""Restarts over the link (LINK-24: REBOOT, the stock bootloader's 200 ms listen, then Ferrite again) and what the
radio is after one (chapters 5, 36, 37: BOOT-*, SELF-*, TERM-*). On the radio the boot report is read from the
cable before the first byte goes out (`T.restart(capture=...)`); the simulator has none, so those parts are radio
only. PIN checks set a PIN by keys, as a person does (the cable is never asked for it: TERM-3), and turn the lock
off after; the PIN itself stays set (Change PIN has no "none"), so on the radio they need --with pin.
"""
import re
import time

from .core import test

TEST_PIN = "1357"


def about(t):
    t.home()
    u = t.keys("MENU 0 9 9")
    return {r["name"]: r["value"] for r in u.rows}


@test("BOOT-15", "SELF-1", pages=("Home", "Info"))
def restart_lands_home(t):
    """A restart over the link: home after the self-test; the boot report gives the MCU's DEBUG_IDCODE
    (0x70050344 expected) and the SPI flash's JEDEC ID with the size from it, and About shows the same."""
    t.need(t.get("0 9 3 1") == "Off", "a PIN lock is on: the restart asks for it")
    u = t.restart(capture=8)
    t.eq((u.id, bool(u.qmenu)), ("Home", False), "home after the restart")
    a = about(t)
    t.check(a["MCU"].startswith("AT32F403A"), "About: the MCU", a["MCU"])
    if t.sim:
        t.note("boot report: the simulator has none (radio only)")
        return
    text = t.boot_text
    m = re.search(r"MCU idcode 0x([0-9A-F]{8}) \(expected 0x([0-9A-F]{8})\)", text)
    t.check(m, "the boot report's idcode line", text[-600:])
    t.eq(m.group(1), m.group(2), "DEBUG_IDCODE as expected")
    t.eq(m.group(2), "70050344", "the AT32F403AVGT7's")
    m = re.search(r"SPI flash JEDEC 0x([0-9A-F]+), (\d+) KB", text)
    t.check(m, "the JEDEC line", text[-600:])
    kb = int(m.group(2))
    t.eq(kb, 1 << (int(m.group(1), 16) & 0xFF) >> 10, "the size from the JEDEC ID's capacity byte")
    t.check(a["SPI flash"].startswith(f"{kb // 1024} MB"), "About's SPI flash row", a["SPI flash"])
    t.check("MCU: ok" in text and "SPI flash: ok" in text, "both checks pass", text[-600:])


@test("BOOT-7", pages=("Home", "Info"))
def restart_checks_calibration(t):
    """At boot the calibration is checked (programmed, PA bytes, order, crystal trim, tone gains, TX windows, the CRC
    recorded at install): the boot report says "Calibration: ok" and "calibration crc: same as recorded", the
    self-test page's Calibration check is OK and the calibration page (0 9 7 1) shows the parsed values."""
    t.need(t.get("0 9 3 1") == "Off", "a PIN lock is on: the restart asks for it")
    t.restart(capture=8)
    if not t.sim:
        text = t.boot_text
        t.check("Calibration: ok" in text, "the boot report: Calibration: ok", text[-600:])
        t.check("calibration crc: same as recorded" in text, "the CRC as recorded at install", text[-600:])
    t.home()
    u = t.keys("MENU 0 9 9")
    u = t.keys("LEFT")
    checks = {r["name"]: r["value"] for r in u.rows}
    t.eq(checks.get("Calibration"), "OK", "the self-test page: calibration OK")
    t.home()
    u = t.keys("MENU 0 9 7 1 MENU")
    page = {r["name"]: r["value"] for r in u.rows}
    t.eq(page.get("Status"), "Valid", "the calibration page: valid")
    t.eq(page.get("CRC at install"), "Matches", "the CRC recorded at install matches")
    t.note(f"calibration page: {page} (the parsed values themselves aren't on a page)")


def pin_screen_type(t, pin):
    return t.keys(" ".join(pin) + " MENU")


def power_on_pin(t, pin):
    """The PIN at power on: BACK first (the boot's first key can go to waking the screen; BACK on an empty entry
    deletes nothing), then the digits and OK."""
    t.keys("BACK")
    return pin_screen_type(t, pin)


def the_pin(t):
    """The PIN these tests use: the run's --pin when the radio has one set, else TEST_PIN (set by them)."""
    return getattr(t.opts, "pin", None) or TEST_PIN


def set_pin_lock(t):
    """PIN lock "At power on" by keys (MENU 0 9 3 1, OK): with no PIN yet, the PIN typed twice first."""
    t.home()
    u = t.keys("MENU 0 9 3 1")
    for _ in range(3):
        if t.get("0 9 3 1") == "At power on":
            return
        u = t.keys("MENU")
        if u.id == "Pin":
            pin_screen_type(t, the_pin(t))
            u = pin_screen_type(t, the_pin(t))
    t.eq(t.get("0 9 3 1"), "At power on", "PIN lock at power on")


def clear_pin_lock(t):
    """PIN lock off by keys: OK steps the lock's modes on to Off, which asks the PIN."""
    for _ in range(3):
        u = t.ui()
        if u.id != "Pin" or not (u["pin"] or {}).get("args", [""])[0] == "Enter PIN":
            break
        power_on_pin(t, the_pin(t))  # the radio restarted: its PIN first
        time.sleep(0.3)
    t.home()
    if t.get("0 9 3 1") == "Off":
        return
    u = t.keys("MENU 0 9 3 1")
    for _ in range(4):
        if t.get("0 9 3 1") == "Off":
            break
        u = t.keys("MENU")
        if u.id == "Pin":
            pin_screen_type(t, the_pin(t))
            t.home()
            t.keys("MENU 0 9 3 1")
    t.home()
    if t.get("0 9 3 1") != "Off":
        t.note(f"PIN lock left {t.get('0 9 3 1')!r} (PIN {the_pin(t)})")


class pin_lock:
    """`with pin_lock(t):` around a PIN test: the PIN lock off before (else the test is skipped) and turned off
    after, whatever happens."""

    def __init__(self, t):
        self.t = t
        t.need(t.get("0 9 3 1") == "Off", "a PIN lock is on")
        t.need(t.get("0 9 3 3") != "Set" or getattr(t.opts, "pin", None) or t.sim,
               "a PIN is set: give it with --pin")

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        clear_pin_lock(self.t)
        return False


@test("TERM-3", needs=("pin",), pages=("Group", "Pin"))
def cable_never_asks_for_the_pin(t):
    """With a PIN and PIN lock set, the cable's link is never asked: a new session's HELLO says it may write, and
    terminal commands run without the PIN."""
    with pin_lock(t):
        import ferrite_link as fl
        set_pin_lock(t)
        hello = bytes([fl.PROTOCOL]) + bytes(8) + b"uitest.py"
        s, rest = t.r.link.ask(fl.HELLO, hello)
        t.eq(s, 0, "HELLO answered")
        t.eq(rest[2], 1, "the cable may write without AUTH")
        t.check(t.cmd("get 0 7 1"), "a terminal command runs", "")


@test("BOOT-13", needs=("pin",), slow=True, pages=("Group", "Home", "Pin"))
def pin_at_power_on_and_the_wait(t):
    """PIN lock at power on: after a restart the PIN is asked; three wrong ones start a 30 s wait (the entry
    refuses meanwhile); the right one after it opens home."""
    with pin_lock(t):
        set_pin_lock(t)
        u = t.restart()
        t.eq(u.id, "Pin", "after the restart: the PIN asked")
        t.eq((u["pin"] or {}).get("args", [""])[0], "Enter PIN", "the PIN entry")
        for i in range(3):
            u = power_on_pin(t, "2468")
        note = (u["pin"] or {}).get("note", "")
        t.check(note.startswith("Wait "), "the third wrong one: the wait", note)
        t.check(25 <= int(note.split()[1]) <= 30, "30 s", note)
        u = power_on_pin(t, the_pin(t))
        t.check(u.id == "Pin", "the right PIN during the wait doesn't open it", u.id)
        t.wait(lambda u: not (u["pin"] or {}).get("note", "").startswith("Wait"), timeout=35, what="the wait ends",
               every=2.0)
        u = power_on_pin(t, the_pin(t))
        u = t.wait(lambda u: u.id == "Home", timeout=3, what="the right PIN after the wait: home")
