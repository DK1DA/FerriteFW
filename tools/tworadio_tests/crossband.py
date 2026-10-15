"""The crossband repeater (chapter 19: XB-1 to XB-20, SAFE-17.3) on two radios and the RTL-SDR.

The receiving radio (t.rx) is the repeater: its slot A on 145.495 (side 1), B on 446.475 (side 2), power Low, started
with `xb start` (XB-20.1) or # on its start page by remote keys (XB-20.2). The transmitting radio (t.tx) keys one
side's input; every such key-up goes through `t.relay`, which admits the repeater's pass (the key-up, its hang and
the pass's end) on the receiving radio's duty limit too. The repeater's state comes from its readback (`xb
running=1 state=N by=WHO ... passes=N listen=N ab=N ba=N` and the four cells, XB-17); the air from the SDR on the
output frequency (--sdr): the carrier's runs (bench.carrier_runs), the sub-tone on it (bench.analyse_subtone), the
end tone and the CW ID (sdrsuite.match_tones / sdrtones.steady_tones). A time on the air is always a length inside
one capture (rtl_sdr starts late by an unknown amount), never a PC-clock time.

The repeater's settings (hang, timeout, tone gate, ID) have no `set` path: they are changed on the start page by
remote keys (◀▶▲▼ to the cell, the knob to the value), read back from its rows, and put back after the test.
"""
import contextlib

import bench
import sdrtones

from .core import UHF, VHF, need_slot, rx_quiet, test, tx_on

# The start page's cells (screens/xb.rs): 0 side 1, 1 side 2, then the grid; NEXT[cell] = (left, right, up, down).
GRID = {"Power": 2, "Timeout": 3, "Hang": 4, "Tone gate": 5, "ID": 6, "Speaker": 7}
NEXT = [(0, 1, 0, 2), (0, 1, 1, 4), (2, 3, 0, 5), (2, 4, 0, 6), (3, 4, 1, 7), (5, 6, 2, 5), (5, 7, 3, 6), (6, 7, 4, 7)]
MOVES = ("LEFT", "RIGHT", "UP", "DOWN")
END = 0.1  # XB-7: a pass ends 100 ms after the drop (plus the hang); the 400 ms hold is the unkey tail's
CW_HZ = 700  # VOX-12: the CW ID's tone


def xb(u):
    x = u["xb"] if u is not None else None
    return x if isinstance(x, dict) else {}


def cells(u):
    return {r["name"]: r["value"] for r in u.rows}


def by_port(t):
    return "bt" if getattr(t.rx.radio, "ble", False) else "cable"


def gate_s(t):
    """The keyer's gates before KEY on (three SLOT reads): longer over Bluetooth."""
    return 3.5 if getattr(t.tx.radio, "ble", False) else 0.6


def cap_s(t, key, extra, more=2.0):
    """A capture's length around a key-up: the SDR's lead, the gates, the key-up, the repeater's extra, a margin."""
    return 1.0 + gate_s(t) + key + extra + more


def value_num(v):
    """A grid value as a number to step towards: "1.5 s" 1.5, "30 s" 30, "3 min" 180; else None (a toggle)."""
    w = v.split()
    try:
        n = float(w[0])
    except (ValueError, IndexError):
        return None
    return n * 60 if len(w) > 1 and w[1] == "min" else n


# ---- the start page ----

def page(t):
    """The start page from home (MENU 0 3 5), the repeater not running."""
    t.rx.home()
    u = t.rx.keys("MENU 0 3 5")
    t.check(u.id == "Crossband" and xb(u).get("running") == 0, "the crossband start page (MENU 0 3 5)",
            f"{u.id} {xb(u)}")
    return u


def goto(t, u, cell):
    """◀▶▲▼ from the focused cell to `cell` (a breadth-first walk of the page's moves)."""
    at = xb(u).get("cell", 0)
    path, seen, todo = {at: []}, {at}, [at]
    while todo:
        c = todo.pop(0)
        for k, n in zip(MOVES, NEXT[c]):
            if n not in seen:
                seen.add(n)
                path[n] = path[c] + [k]
                todo.append(n)
    if path[cell]:
        u = t.rx.keys(" ".join(path[cell]))
    t.check(xb(u).get("cell") == cell, f"the start page's cell {cell}", xb(u))
    return u


def put(t, u, name, want):
    """A grid cell to `want` by the knob, read back from its row."""
    u = goto(t, u, GRID[name])
    for _ in range(14):
        v = cells(u).get(name)
        if v == want:
            return u
        n, w = value_num(v or ""), value_num(want)
        t.rx.radio.knob(1 if n is None or w is None or w > n else -1)
        t.sleep(0.1)
        u = t.rx.ui()
    t.fail(f"the start page's {name} to {want!r} (reads {cells(u).get(name)!r})")


def restore_grid(t, orig):
    t.rx.cmd_err("xb stop")
    u = page(t)
    for name, v in orig.items():
        if cells(u).get(name) != v:
            u = put(t, u, name, v)
    t.rx.home()


def setup(t, hang="1.0 s", gate="On", ident="Off", timeout=None, txtones=("off", "off"), rxtones=("off", "off"),
          endtone="Off", watch="Off"):
    """The receiving radio as the repeater: A on 145.495 (side 1), B on 446.475 (side 2), each slot's own tones, the
    end tone as asked (0 8 5 4: Off where a carrier's length is measured), the grid as asked, power Low and the
    pre-flight checks passing. The grid is put back after the test (deferred)."""
    rx_quiet(t, watch=watch)
    # Each side read back as set (the line `slot` answers): a side that didn't take stops the test here, before
    # anything keys (Oct 14, 2026: passes=0 all module long, the repeater's sides on the radio's own slots).
    for L, mhz, i in (("A", VHF, 0), ("B", UHF, 1)):
        line = t.rx.cmd(f"slot {L} vfo {mhz} fm low txtone {txtones[i]} rxtone {rxtones[i]} offset off")
        need_slot(t, t.rx, L, mhz, txtones[i], rxtones[i], line=line.strip(), what=f"the repeater's side {i + 1}")
    t.rx.set("0 8 5 4", endtone)
    u = page(t)
    g = cells(u)
    want = (f"A {float(VHF):.5f}", f"B {float(UHF):.5f}")
    t.check((g.get("Side 1"), g.get("Side 2")) == want, f"the repeater's sides are A {VHF} and B {UHF} (its start page)",
            (g.get("Side 1"), g.get("Side 2")))
    orig = {k: g[k] for k in GRID if k in g}
    t.defer(lambda: restore_grid(t, orig))
    for name, want in (("Hang", hang), ("Tone gate", gate), ("ID", ident), ("Timeout", timeout)):
        if want is not None:
            u = put(t, u, name, want)
    u = t.rx.ui()
    t.eq(cells(u).get("Power"), "Low", "the repeater's power (XB-3: Low, the only one these tests run)")
    fails = [x for x in u.lines if x.startswith("check fail")]
    t.check(not fails, "the start page's pre-flight checks pass (XB-2)", fails)
    return u


def start(t, keys=False):
    """Start it: `xb start` (XB-20.1), or # on the start page by remote keys (XB-20.2); stopped after the test."""
    t.defer(lambda: t.rx.cmd_err("xb stop"))
    if keys:
        t.rx.keys("HASH")
    else:
        out = t.rx.cmd("xb start").strip()
        t.check(out.startswith(f"Crossband started: {SIDES}"), "xb start starts it", out)
    u, _ = t.poll(t.rx, lambda u: xb(u).get("running") == 1, 2.0)
    t.check(u is not None, "the repeater runs (its readback: xb running=1)", xb(t.rx.raw()))
    sides_running(t)
    return u


SIDES = f"A {float(VHF):.5f} <-> B {float(UHF):.5f}"


def sides_running(t):
    """`xb` on the repeater: running on A 145.495 <-> B 446.475 (one short terminal line, light over Bluetooth)."""
    out = t.rx.cmd("xb").strip()
    t.check(out.startswith(f"Crossband: running, {SIDES}"), f"xb: running, {SIDES}", out)


def keyer_on(t, mhz, txtone="off"):
    """Right before a key-up into the repeater: the transmitting radio on `mhz` (read back) and the repeater still
    running on its sides (`xb`), so a set-up that didn't take fails the test before anything keys."""
    tx_on(t, mhz, txtone)
    sides_running(t)


def stop(t):
    ok, out = t.rx.cmd_err("xb stop")
    t.check(ok and out.strip() == "Crossband stopped", "xb stop stops it", out)


def until_ready(t, passes, timeout=5.0):
    """After a key-up: the pass over (state 0) with `passes` counted."""
    u, _ = t.poll(t.rx, lambda u: xb(u).get("state") == 0 and xb(u).get("passes") == passes, timeout, every=0.1,
                  light=True)
    t.check(u is not None, f"the pass ends, {passes} passes counted", xb(t.rx.state()))
    return t.rx.raw() if isinstance(u, dict) else u


# ---- the air ----

def runs(cap):
    return bench.carrier_runs(cap.raw)


def longest(cap):
    r = runs(cap)
    return max((b - a for a, b in r), default=0.0)


def tail_tones(cap, secs=8.0):
    """The steady tones in the output carrier's last `secs` s (Hz, length s): the end tone, the CW ID's elements."""
    k = bench._keyed_dev(cap.raw)
    if not k:
        return []
    dev, fs = k["dev"], k["fs"]
    a = max(0, len(dev) - int(secs * fs))
    seg = dev[a:] - dev[a:].mean()
    return [(f, t1 - t0) for t0, t1, f, _ in sdrtones.steady_tones(seg, fs)]


# ---- the tests ----

@test("XB-20.1", "XB-20.2", "XB-20.4", "XB-20.5", "XB-20.6", "XB-15", "XB-13", "XB-17",
      part=("XB-20.5", "XB-20.6", "XB-15", "XB-13", "XB-17"), idle=20)
def started_and_stopped_over_the_link(t):
    """No keying. `xb start` from home opens the repeater's screen and starts it (readback: Crossband, running, by
    cmd:PORT, the mode xb, watch and scan off in the bar, the hint "Any key: Stop", the cells at rest); `xb` says so.
    SIDE1 held listens (listen=1) and doesn't stop it. A remote digit stops it (the start page stays up, "Repeater
    stopped"). # on the start page by remote keys starts it again (by keys:PORT); `xb stop` stops it, a second one
    is refused. Home's watch is back after. The radio's own keys (XB-20.5) and the cable log (XB-20.6) are the host
    tests' and the bench's."""
    setup(t, watch="A+B")
    home = t.rx.home()
    t.check(home.bar.get("watch") == 2, "home: watch A+B in the bar (before)", home.bar)
    t.check(t.rx.cmd("xb").strip().startswith("Crossband: off"), "xb: off", t.rx.cmd("xb"))
    u = start(t)
    port = by_port(t)
    x = xb(u)
    t.eq(u.id, "Crossband", "the repeater's screen while it runs (XB-20.4)")
    t.eq(x.get("by"), f"cmd:{port}", "who started it (XB-20.6)")
    t.eq(u.state.get("mode"), "xb", "the exclusive mode is the repeater's (XB-13: watch, scan paused)")
    t.check(u.bar.get("watch") == 0 and u.bar.get("scan") == 0, "no watch or scan in the bar while it runs", u.bar)
    t.check(("Any key", "Stop") in u.hints, "the hint: any key stops it (XB-20.5)", u.hints)
    c = cells(u)
    t.check(c.get("Last pass") == "none yet" and c.get("Next ID") == "Off" and c.get("Passes", "").startswith("0 "),
            "the four cells at rest (XB-17)", c)
    out = t.rx.cmd("xb").strip()
    t.check(out.startswith("Crossband: running") and out.endswith(f"started by cmd:{port}"), "xb: running, and who",
            out)
    u = t.rx.keys("SIDE1:down")
    t.check(xb(u).get("running") == 1 and xb(u).get("listen") == 1, "SIDE1 held: listening, still running (XB-15)",
            xb(u))
    u = t.rx.keys("SIDE1:up")
    t.check(xb(u).get("running") == 1 and xb(u).get("listen") == 0, "SIDE1 let go: not listening", xb(u))
    u = t.rx.keys("5")
    t.check(xb(u).get("running") == 0, "a remote digit stops it (XB-20.5)", xb(u))
    t.eq(u.id, "Crossband", "the press did nothing else: the start page is up")
    t.check(u.toast and u.toast["head"] == "Repeater stopped", "the toast says so", u.toast)
    u = start(t, keys=True)
    t.eq(xb(u).get("by"), f"keys:{port}", "# on the start page by remote keys starts it (XB-20.2), who logged")
    stop(t)
    ok, out = t.rx.cmd_err("xb stop")
    t.check(not ok and "isn't running" in out, "xb stop again: refused, it isn't running", out)
    home = t.rx.home()
    t.check(home.bar.get("watch") == 2, "home: watch back after it stops (XB-13)", home.bar)


def relay_one_way(t, key_mhz, out_mhz, state, txtones, item, what):
    """One pass: the other radio keys `key_mhz` 3 s; the readback during it (one direction: `state` throughout, the
    cells), after it (one pass more, no turnaround, the strip); the SDR on `out_mhz`: one carrier as long as the
    key-up and the pass's end, carrying the output slot's own tone `item`."""
    setup(t, txtones=txtones)
    u0 = start(t)
    p0 = xb(u0).get("passes", 0)
    keyer_on(t, key_mhz)
    extra = 1.0 + END
    with t.capture(out_mhz, cap_s(t, 3, extra), what, key=(3, extra)) as cap:
        with t.relay(3, what, extra=extra):
            u, at = t.poll(t.rx, lambda u: xb(u).get("state") in (1, 2), 2.0)
            t.check(u is not None, "a pass starts", xb(t.rx.raw()))
            t.eq(xb(u).get("state"), state, f"the pass's direction ({'side 1 to 2' if state == 1 else 'side 2 to 1'})")
            t.eq(cells(u).get("Last pass"), "now", "Last pass: now while passing (XB-17)")
            got = t.samples(t.rx, max(0.3, at + 1.8 - t.now()), light=True)
            states = sorted({xb(x).get("state") for _, x in got})
            t.check(states == [state], "one direction all through the key-up (XB-5)", states)
    u = until_ready(t, p0 + 1)
    t.sleep(1.0)
    u = t.rx.ui()
    t.eq(xb(u).get("passes"), p0 + 1, "no turnaround after the pass (XB-5, XB-11): one pass more")
    ab, ba = xb(u).get("ab", 0), xb(u).get("ba", 0)
    mine, other = (ab, ba) if state == 1 else (ba, ab)
    t.check(mine >= 1 and other == 0, "the strip: this direction's bar only (XB-16)", (ab, ba))
    c = cells(u)
    t.check(c.get("Passes", "").startswith(f"{p0 + 1} "), "Passes counts it (XB-17)", c.get("Passes"))
    t.check(c.get("Duty, 10 min", "").endswith("% of 50%"), "Duty, 10 min: its share of the limit (XB-17)",
            c.get("Duty, 10 min"))
    t.check(c.get("Last pass") not in ("now", "none yet"), "Last pass: how long ago (XB-17)", c.get("Last pass"))
    r = runs(cap)
    t.check(len(r) == 1, f"one carrier on {out_mhz} MHz (the pass)", r)
    n = r[0][1] - r[0][0]
    t.note(f"output carrier {n:.2f} s for a 3 s key-up, hang 1.0 s")
    t.check(2.6 <= n <= 3 + extra + 1.5, "it follows the key-up: about its length plus the hang and the pass's end "
            "(XB-6, XB-7)", f"{n:.2f} s")
    st = bench.analyse_subtone(cap.raw, item)
    t.check(st["ok"], "the output carries the transmitting slot's own tone (XB-6)", st.get("text"))


@test("XB-5", "XB-6", "XB-16", "XB-17", part=("XB-5", "XB-6", "XB-16"), keyups=(3,), relays=(1.1,), idle=12, sdr=True)
def relays_2m_to_70cm(t):
    """The other radio keys 145.495 (side 1) 3 s: the repeater passes side 1 to 2 throughout (state 1, never 2),
    Last pass "now"; after: one pass more, no turnaround, the strip's A to B bar, Passes, Duty above 0. On the SDR at
    446.475: one carrier about the key-up's length plus the hang, carrying side 2's own T 100.0."""
    relay_one_way(t, VHF, UHF, 1, ("off", "100.0"), {"hz": 100.0}, "2 m into the repeater, out on 70 cm")


@test("XB-5", "XB-6", "XB-16", part=("XB-5", "XB-6", "XB-16"), keyups=(3,), relays=(1.1,), idle=12, sdr=True)
def relays_70cm_to_2m(t):
    """The other way: the other radio keys 446.475 (side 2): state 2 throughout, the strip's B to A bar; on the SDR
    at 145.495 the pass carries side 1's own D023N (decoded with its polarity, never the other)."""
    relay_one_way(t, UHF, VHF, 2, ("D023N", "off"), {"code": 0o23, "inverted": False},
                  "70 cm into the repeater, out on 2 m")


@test("XB-4", part=("XB-4",), keyups=(2, 2, 2), relays=(1.1,), idle=25)
def tone_gate_opens_only_on_its_tone(t):
    """Side 1 (A) on R 88.5, the tone gate on: the other radio's carrier with no tone never starts a pass (state 0
    all through, passes unchanged); with 88.5 it does (state 1). The tone gate off: a carrier with no tone passes.
    The 30 ms of open squelch before a pass is the host tests' (a_pass_with_hang_and_id)."""
    setup(t, rxtones=("88.5", "off"))
    p = xb(start(t)).get("passes", 0)
    keyer_on(t, VHF)
    with t.relay(2, "no tone into a side gated on 88.5", extra=1.0 + END):
        t.sleep(0.3)
        got = t.samples(t.rx, 1.2, light=True)
    states = sorted({xb(u).get("state") for _, u in got})
    t.check(states == [0], "no tone: no pass (state 0 all through)", states)
    t.sleep(1.5)
    t.eq(xb(t.rx.ui()).get("passes"), p, "no tone: no pass counted")
    keyer_on(t, VHF, "88.5")
    with t.relay(2, "88.5 into a side gated on 88.5", extra=1.0 + END):
        u, _ = t.poll(t.rx, lambda u: xb(u).get("state") == 1, 1.5, light=True)
        t.check(u is not None, "88.5: a pass starts (state 1)", xb(t.rx.state()))
    p += 1
    until_ready(t, p)
    stop(t)
    u = put(t, page(t), "Tone gate", "Off (carrier)")
    p = xb(start(t)).get("passes", 0)
    keyer_on(t, VHF)
    with t.relay(2, "no tone, the tone gate off", extra=1.0 + END):
        u, _ = t.poll(t.rx, lambda u: xb(u).get("state") == 1, 1.5, light=True)
        t.check(u is not None, "the tone gate off: a carrier with no tone passes", xb(t.rx.state()))
    until_ready(t, p + 1)


@test("XB-7", "XB-8", part=("XB-7", "XB-8"), keyups=(1.8, 1.8), relays=(2.1,), idle=20, sdr=True)
def hang_and_carrier_hold_on_air(t):
    """Two 1.8 s key-ups into side 1, the SDR on side 2 (446.475): with hang 0 the output carrier is the key-up's
    length plus 0.5 to 1.4 s (the 100 ms drop, the 400 ms hold and the unkey wait: XB-8's carrier hold, the input's
    own hold); with hang 2.0 s it is 2.0 s (1.7 to 2.3) longer, the readback in hang (state 3) after the key-up."""
    lengths = {}
    for hang in ("0.0 s", "2.0 s"):
        if lengths:
            t.rx.cmd_err("xb stop")
            put(t, page(t), "Hang", hang)
        else:
            setup(t, hang=hang)
        start(t)
        keyer_on(t, VHF)
        h = value_num(hang)
        extra = h + END
        with t.capture(UHF, cap_s(t, 1.8, extra), f"hang {hang}", key=(1.8, extra)) as cap:
            with t.relay(1.8, f"a pass with hang {hang}", extra=extra):
                t.poll(t.rx, lambda u: xb(u).get("state") == 1, 1.5, light=True)
            if h:
                u, _ = t.poll(t.rx, lambda u: xb(u).get("state") == 3, 1.5, light=True)
                t.check(u is not None, "after the key-up: the hang (state 3)", xb(t.rx.state()))
        lengths[hang] = longest(cap)
        t.note(f"hang {hang}: output carrier {lengths[hang]:.2f} s")
    d = lengths["0.0 s"] - 1.8
    t.check(0.5 <= d <= 1.4, "hang 0: the carrier holds after the pass's end (XB-8), about 0.9 s past the key-up",
            f"{d:.2f} s")
    d = lengths["2.0 s"] - lengths["0.0 s"]
    t.check(1.7 <= d <= 2.3, "hang 2.0 s: the pass 2.0 s longer (XB-7)", f"{d:.2f} s")


@test("XB-8", part=("XB-8",), keyups=(2,), relays=(0.8,), idle=10, sdr=True)
def end_tone_on_the_output(t):
    """The repeater's end tone (0 8 5 4) Beep, hang 0: at the pass's end the output carrier (446.475) carries the
    1000 Hz beep before it drops, as the transmitting slot's end tone at an unkey (XB-8)."""
    setup(t, hang="0.0 s", endtone="Beep")
    start(t)
    keyer_on(t, VHF)
    extra = 0.8
    with t.capture(UHF, cap_s(t, 2, extra), "end tone Beep", key=(2, extra)) as cap:
        with t.relay(2, "a pass with the end tone on", extra=extra):
            t.poll(t.rx, lambda u: xb(u).get("state") == 1, 1.5)
    tones = tail_tones(cap, 1.5)
    t.check(any(abs(f - 1000) < 40 and n >= 0.08 for f, n in tones), "a 1000 Hz beep at the pass's end",
            [(round(f), round(n, 3)) for f, n in tones])


def cut(t, how, what):
    """A pass cut by the repeater's own stop while the other radio still keys: the output carrier ends long before
    an uncut pass would (3 s + hang 1.0 + its end: about 4.5 s), the readback stopped."""
    keyer_on(t, VHF)
    extra = 1.0 + END
    with t.capture(UHF, cap_s(t, 3, extra), what, key=(3, extra)) as cap:
        with t.relay(3, what, extra=extra) as k:
            u, _ = t.poll(t.rx, lambda u: xb(u).get("state") == 1, 2.0, light=True)
            t.check(u is not None, "a pass starts", xb(t.rx.state()))
            t.sleep(0.5)
            if how == "key":
                u = t.rx.keys("5")
            else:
                stop(t)
                u = t.rx.raw()
            still = k.t_off is None
            t.check(xb(u).get("running") == 0, "stopped mid-pass", xb(u))
            t.check(still, "while the other radio still keys")
    n = longest(cap)
    t.note(f"{what}: output carrier {n:.2f} s (uncut about 4.5 s)")
    t.check(0.2 <= n <= 3.5, "the output ends with the stop, not with the key-up", f"{n:.2f} s")


@test("XB-20.5", "XB-20.1", part=("XB-20.5", "XB-20.1"), keyups=(3, 3), relays=(1.1,), idle=20, sdr=True)
def a_key_or_xb_stop_cuts_a_pass(t):
    """Mid-pass (the other radio keying side 1 for 3 s), a remote digit on the repeater stops it: its output carrier
    (446.475) ends with the stop, the readback stopped; then `xb stop` mid-pass does the same."""
    setup(t)
    start(t)
    cut(t, "key", "a remote key mid-pass")
    start(t)
    cut(t, "xb stop", "xb stop mid-pass")


@test("XB-1", "SAFE-17.3", keyups=(2,), relays=(1.1,), idle=40)
def never_on_after_a_restart(t):
    """The repeater running, the radio restarted over the link (REBOOT): it comes back home with the repeater off
    (`xb`: off, the mode not xb), and the other radio keying side 1 gets no pass: the readback stays home and, with
    the SDR, nothing on 446.475."""
    t.need(t.rx.get("0 9 3 1") == "Off", "a PIN lock is on: the restart asks for the PIN")
    setup(t)
    start(t)
    t.rx.radio.restart()
    u, _ = t.poll(t.rx, lambda u: u.whole and u.id not in ("", "Boot"), 20.0, every=0.5)
    t.check(u is not None, "the radio is up again", t.rx.raw().id)
    t.eq(u.id, "Home", "home after the restart, not the repeater")
    t.check(u.state.get("mode") != "xb", "the repeater's mode isn't running", u.state)
    out = t.rx.cmd("xb").strip()
    t.check(out.startswith("Crossband: off"), "xb: off after the restart (XB-1)", out)
    tx_on(t, VHF)
    extra = 1.0 + END
    cm = t.capture(UHF, cap_s(t, 2, extra), "after a restart", key=(2, extra)) if t.run.sdr else \
        contextlib.nullcontext()
    with cm as cap:
        with t.relay(2, "into side 1 after the restart", extra=extra):
            got = t.samples(t.rx, 1.2)
    t.check(all(x.id == "Home" and x.state.get("mode") != "xb" for _, x in got), "no repeater: home all through",
            sorted({x.id for _, x in got}))
    if cap is not None:
        t.check(not runs(cap), "nothing on 446.475", runs(cap))


@test("XB-9", part=("XB-9",), keyups=(34,), relays=(0.0,), idle=12, long=True)
def a_pass_times_out(t):
    """Timeout 30 s, hang 0: the other radio keys side 1 for 34 s (the duty limit's timeout exception, both radios):
    the pass ends 29 to 32 s in (state 4, timed out) and doesn't pass again while that carrier lasts (passes
    unchanged, state 4); once it drops the repeater is ready. With the SDR the output carrier is 29.5 to 31.5 s."""
    setup(t, hang="0.0 s", timeout="30 s")
    p = xb(start(t)).get("passes", 0)
    keyer_on(t, VHF)
    cm = t.capture(UHF, cap_s(t, 34, 0.5), "a 30 s timeout", key=(34, 0.0, "timeout")) if t.run.sdr else \
        contextlib.nullcontext()
    with cm as cap:
        with t.relay(34, "a pass to its timeout", extra=0.0, kind="timeout", relay_kind="timeout"):
            u, t0 = t.poll(t.rx, lambda u: xb(u).get("state") == 1, 2.0)
            t.check(u is not None, "a pass starts", xb(t.rx.raw()))
            u, t1 = t.poll(t.rx, lambda u: xb(u).get("state") == 4, 33.0, every=0.25)
            t.check(u is not None, "it times out (state 4)", xb(t.rx.raw()))
            t.note(f"timed out {t1 - t0:.1f} s into the pass")
            t.check(29.0 <= t1 - t0 <= 32.0, "after the 30 s timeout", f"{t1 - t0:.1f} s")
            got = t.samples(t.rx, 1.5)
            t.check(all(xb(x).get("state") == 4 and xb(x).get("passes") == p + 1 for _, x in got),
                    "no new pass while the carrier lasts", sorted({(xb(x).get("state"), xb(x).get("passes"))
                                                                   for _, x in got}))
    until_ready(t, p + 1)
    if cap is not None:
        n = longest(cap)
        t.check(29.5 <= n <= 31.5, "the output carrier ends at the timeout", f"{n:.2f} s")


@test("XB-12", part=("XB-12",), keyups=(1, 1), relays=(7.0, 0.6), idle=20, sdr=True, long=True)
def id_after_the_first_pass(t):
    """The repeater's ID on (its callsign set on the radio, 0 8 7 1): the first pass ends with the callsign in CW
    (700 Hz elements on the output after the carrier's pass), Next ID then "CW in 10 min"; a second pass straight
    after has none. The ID after 10 minutes stays by hand. The pass with the ID is one long transmission (the duty
    limit's timeout exception)."""
    call = t.rx.get("0 8 7 1")
    t.need(call and call != "Empty", "the repeater's radio has no callsign (0 8 7 1, set on the radio)")
    setup(t, hang="0.0 s", ident="CW 10 m")
    u = start(t)
    p = xb(u).get("passes", 0)
    t.check(cells(u).get("Next ID", "").startswith("CW in"), "Next ID counts down (XB-17)", cells(u).get("Next ID"))
    keyer_on(t, VHF)
    id_s = 1.5 + 0.9 * len(call)
    with t.capture(UHF, cap_s(t, 1, id_s + END), "the first pass with the ID", key=(1, id_s + END, "timeout")) as cap:
        with t.relay(1, "the first pass (ID due)", extra=id_s + END, relay_kind="timeout"):
            t.poll(t.rx, lambda u: xb(u).get("state") == 1, 1.5)
        t.sleep(id_s)
    cw = [n for f, n in tail_tones(cap, id_s + 2) if abs(f - CW_HZ) < 60]
    t.check(len(cw) >= 3, f"the CW ID at the pass's end: {CW_HZ} Hz elements", len(cw))
    u = until_ready(t, p + 1, timeout=id_s + 3)
    t.check(cells(u).get("Next ID", "").startswith("CW in 1"), "Next ID: about 10 min after the ID",
            cells(u).get("Next ID"))
    with t.capture(UHF, cap_s(t, 1, 0.6), "a second pass", key=(1, 0.6)) as cap:
        with t.relay(1, "a second pass (no ID due)", extra=0.6):
            t.poll(t.rx, lambda u: xb(u).get("state") == 1, 1.5)
    cw = [n for f, n in tail_tones(cap, 3) if abs(f - CW_HZ) < 60]
    t.check(not cw, "no ID on a pass inside the 10 minutes", len(cw))
    t.check(longest(cap) < 2.6, "the second pass ends without the ID's length", f"{longest(cap):.2f} s")
