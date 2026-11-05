"""Scanning and the band scope's trigger (chapters 14, 21: SCAN-14, SCAN-21, SCOPE-10), by keys and readback."""
import time

from .core import test


def presets(t):
    return [int(x.split()[1]) for x in t.cmd("dump").splitlines() if x.startswith("preset ")]


def delete_preset(t, n):
    """A range preset deleted by keys (the terminal has no delete): * on it in MENU 0 1 4, then OK."""
    have = presets(t)
    t.home()
    t.keys("MENU 0 1 4")
    u = t.keys(" ".join(["DOWN"] * have.index(n) + ["STAR"]))
    t.eq(u.id, "Confirm", f"* on preset {n}: delete it?")
    t.keys("MENU")
    t.check(n not in presets(t), f"preset {n} deleted", presets(t))


@test("SCAN-14", pages=("Confirm", "ScanLists"))
def preset_editor_refuses_a_span_across_64(t):
    """The range preset editor refuses an end that would make a preset span 64 MHz: the toast "Can't span 64 MHz,
    one side only", the end stays where it was, the editor stays open. (A preset made for the test on 60-64 MHz,
    deleted after.)"""
    have = presets(t)
    t.need(len(have) < 16, "16 presets already")
    n = len(have) + 1
    t.cmd(f'preset {n} "UT 6 m" 60.0 64.0')
    try:
        t.home()
        t.keys("MENU 0 1 4")
        u = t.keys(" ".join(["DOWN"] * (n - 1) + ["MENU"]))
        t.eq(u.id, "ScanLists", "OK on the preset: its editor")
        u = t.keys("DOWN DOWN")  # Name, Start, End
        u = t.knob(1)  # 64.0 + 0.1 MHz: across 64
        t.eq(u.toast, {"head": "Can't span 64 MHz", "body": "one side only"}, "refused with the toast")
        t.eq(u.id, "ScanLists", "the editor stays open")
        line = next(x for x in t.cmd("dump").splitlines() if x.startswith(f"preset {n} "))
        t.check(" 64.00000" in line, "the end where it was", line)
    finally:
        delete_preset(t, n)


@test("SCAN-21", where="radio", pages=("Home", "Scan"))
def range_scan_fast_pass_rate(t):
    """A band scan over a quiet stretch of 70 cm (nothing to stop on) steps at least 30 times a second (each step
    reads the RSSI for 15 ms), measured from the active card's frequency over 4 s, watch off (the runner puts it
    back): a watched slot on the other band takes the chip for its turns and the rate falls to about 27 (the host
    model, scan.rs `fast_pass_on_the_ten_ms_tick`; the radio's 27.2 and 27.5, Oct 15, 2026). Radio only: the
    simulator's bands are full of stations."""
    stopped = t.stop_scans()
    try:
        t.set("0 1 1", "Off")
        t.cmd("slot A vfo 433.0")
        t.cmd("slot A active")
        u = t.home()
        step = t.keys("MENU").row("Step")["value"]
        t.home()
        khz = float(step.split()[0]) * (1000 if step.endswith("MHz") else 1)
        t.keys("SIDE2")
        t.home()
        last, moved, t0 = None, 0.0, time.time()
        stops = 0
        while time.time() - t0 < 4:
            f = float(t.ui(0).cards["A"]["freq"])
            if last is not None:
                d = f - last
                if 0 < d < 1.0:
                    moved += d
                elif d == 0:
                    stops += 1
            last = f
        rate = moved * 1000 / khz / (time.time() - t0)
        t.note(f"{rate:.1f} steps a second at {step} ({stops} reads without a move)")
        t.check(rate >= 30, "at least 30 steps a second (nicFW's)", f"{rate:.1f}")
        del u
    finally:
        t.home()
        if "scanning" in t.slot("A"):
            t.keys("SIDE2")
        t.put_back(stopped)


@test("SCOPE-10", needs=("fm-station",), pages=("Group", "Home", "Scope"))
def scope_trigger_monitor_and_log(t):
    """The trigger over the broadcast band's strong carriers: Monitor (0 3 1 4) listens to the strongest bar once it
    has been over the line 3 sweeps running (`listen=3`), and goes back to sweeping when its squelch closes; Log
    marks it without stopping (until the event log's store is built, its toast is the record: "Over the line")."""
    stopped = t.stop_scans()
    try:
        # the radio: a broadcast carrier; the simulator: its 2 m sample station (its scope has no broadcast band)
        t.cmd("slot A vfo 146.52" if t.sim else "slot A vfo 100.1")
        t.cmd("slot A active")
        for trig in ("Monitor", "Log"):
            t.home()
            u = t.keys("MENU 0 3 1 4")
            for _ in range(3):
                if u.sel["value"] == trig:
                    break
                u = t.knob(1)
            t.eq(u.sel["value"], trig, f"trigger {trig}")
            t.home()
            t.press("1", long=True)
            t.press("1", long=True)  # the trigger line low: 1 of 9
            if trig == "Monitor":
                u = t.wait(lambda u: (u["scope"] or {}).get("listen") == 3, timeout=15, what="the trigger listens")
                t.note(f"listening at {u['scope']['marker']}")
            else:
                u = t.wait(lambda u: u.toast and u.toast["head"] == "Over the line", timeout=15,
                           what="the trigger's log toast")
                t.eq((u["scope"] or {}).get("listen"), 0, "Log doesn't stop the sweep")
            t.press("0", long=True)  # the line cleared
    finally:
        t.home()
        t.put_back(stopped)
