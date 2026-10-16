"""The mode screens by their readback (chapters 19, 20, 20a, 21: XB-*, BCR-*, HF-*, SCOPE-*): HF mode's `hf` line,
the FM screen's `fm` line, a broadcast card's report, the band scope's `scope` line, crossband's start page, and
the `mode=`/`fm=` the state line gives (MODE-1: which exclusive mode runs).

HF mode keeps its own VFO (the settings record, not in `dump`): every HF test puts the frequency and mode back
(`HfVfo`). The FM screen's last station likewise (`fm_back`).
"""
import time

from .core import test

QUIET = "146.55"


# ---- HF mode ----

def hf_line(u):
    return u["hf"] or {}


def hf_open(t):
    """MENU 0 3 4, past the SSB patch's load (HF-3): the readback's `hf freq=...` line."""
    t.home()
    u = t.keys("MENU 0 3 4")
    t.eq(u.id, "Hf", "MENU 0 3 4")
    return t.wait(lambda u: "freq" in hf_line(u), timeout=20, what="HF mode loaded (the SSB patch streamed in)")


def hf_type(t, mhz):
    """Types a frequency in MHz in HF mode: digits, * for the point, OK enters (VFO-24)."""
    whole, _, frac = f"{float(mhz):.5f}".partition(".")
    frac = frac.rstrip("0") or "0"
    seq = " ".join(whole) + " STAR " + " ".join(frac) + " MENU"
    return t.keys(seq, settle=0.3)


def hf_mode(t, want, tries=6):
    """* until HF mode's mode is `want` (or it never comes: None)."""
    u = t.ui()
    for _ in range(tries):
        if hf_line(u).get("mode") == want:
            return u
        u = t.keys("STAR", settle=0.25)
    return u if hf_line(u).get("mode") == want else None


class HfVfo:
    """HF mode's VFO as a test found it, put back after (frequency, then the mode)."""

    def __init__(self, t):
        self.t = t
        h = hf_line(hf_open(t))
        self.freq, self.mode = h["freq"], h["mode"]

    def restore(self):
        t = self.t
        try:
            u = t.ui()
            if u.id != "Hf":
                u = hf_open(t)
            if hf_line(u).get("freq") != self.freq:
                hf_type(t, self.freq)
            hf_mode(t, self.mode)
            got = hf_line(t.ui())
            if (got.get("freq"), got.get("mode")) != (self.freq, self.mode):
                t.note(f"HF VFO left at {got.get('freq')} {got.get('mode')}, was {self.freq} {self.mode}")
        finally:
            t.home()


@test("HF-15", pages=("Hf",))
def hf_screen_shows_its_fields(t):
    """HF mode shows the frequency, band, mode, receiver, step, bandwidth and signal (SNR on the SI4732, an
    S-meter on chip 1), and the BFO offset in USB, LSB and CW only."""
    vfo = HfVfo(t)
    try:
        hf_type(t, 9.58)
        u = hf_mode(t, "AM")
        h = hf_line(u)
        t.eq((h["band"], h["rx"], h["mode"]), ("31 m", "SI4732", "AM"), "9.58 MHz AM")
        t.check(h["step"].endswith("kHz") and h["bw"].endswith("kHz"), "AM: a step and a bandwidth in kHz", h)
        t.check(h["signal"].startswith("SNR "), "the SI4732's signal is its SNR", h["signal"])
        t.eq(h["bfo"], "", "no BFO in AM")
        u = hf_mode(t, "USB")
        if u is not None:
            t.check(hf_line(u)["bfo"].startswith("BFO "), "USB: the BFO offset", hf_line(u))
        else:
            t.note("no USB here (no SSB patch): the BFO line not checked")
        hf_type(t, 28.4)
        u = hf_mode(t, "FM")
        t.check(u is not None, "28.4 MHz offers FM", hf_line(t.ui()))
        h = hf_line(u)
        t.eq(h["rx"], "CHIP 1", "FM on 28.4 MHz: chip 1")
        t.check(h["signal"].startswith("S"), "chip 1's signal is an S-meter", h["signal"])
        t.eq(h["band"], "10 m", "the band")
    finally:
        vfo.restore()


@test("HF-9", "HF-10", pages=("Hf",))
def hf_receiver_follows_frequency_and_mode(t):
    """HF mode's receiver by range and mode: LW, MW and shortwave AM on the SI4732; USB, LSB, CW on it too (with
    the patch); FM 18-64 MHz on chip 1; AM 18-30 MHz on the SI4732."""
    vfo = HfVfo(t)
    try:
        for mhz, band in ((0.198, "LW"), (0.909, "MW"), (9.58, "31 m"), (15.4, "19 m")):
            hf_type(t, mhz)
            u = hf_mode(t, "AM")
            h = hf_line(u)
            t.eq((h["band"], h["rx"]), (band, "SI4732"), f"{mhz} MHz AM")
        hf_type(t, 7.074)
        u = hf_mode(t, "USB")
        if u is not None:
            t.eq(hf_line(u)["rx"], "SI4732", "USB on 40 m: the SI4732")
        for mhz in (27.185, 28.4, 50.125):
            hf_type(t, mhz)
            u = hf_mode(t, "FM")
            t.check(u is not None, f"{mhz} MHz offers FM", hf_line(t.ui()))
            t.eq(hf_line(u)["rx"], "CHIP 1", f"{mhz} MHz FM: chip 1")
        hf_type(t, 27.185)
        u = hf_mode(t, "AM")
        t.eq(hf_line(u)["rx"], "SI4732", "AM on CB (18-30 MHz): the SI4732")
    finally:
        vfo.restore()


@test("HF-10", pages=("Hf",), xfail="AM on 30-64 MHz stays on the SI4732 (hf.rs HfMem::rx: chip 1 for FM only); the "
      "SI4732 tunes to 30 MHz, and HF-9/HF-10 put AM 30-64 MHz on chip 1")
def hf_am_above_30_mhz_is_chip_1(t):
    """AM above 30 MHz (6 m) is chip 1's: the SI4732 stops at 30 MHz (HF-9: chip 1 takes 18-64 MHz, FM and AM;
    HF-10: AM uses the SI4732 on 18-30 MHz only)."""
    vfo = HfVfo(t)
    try:
        hf_type(t, 50.4)
        u = hf_mode(t, "AM")
        t.eq(hf_line(u)["rx"], "CHIP 1", "AM on 50.4 MHz")
    finally:
        vfo.restore()


@test("HF-12", pages=("Advanced", "Hf", "Home", "Recall"))
def hf_keys(t):
    """HF mode's keys: knob and ▲▼ tune by the step, SIDE2 + knob by 1 MHz, ◀▶ the next band, * the mode, digits
    a frequency (* point, OK enters), # the recent list, hold MENU the options (0 there the full menu); hold # stays
    key lock."""
    vfo = HfVfo(t)
    try:
        hf_type(t, 9.6)
        u = hf_mode(t, "AM")
        h = hf_line(u)
        step_khz = float(h["step"].split()[0]) if h["step"].endswith("kHz") else float(h["step"].split()[0]) / 1000
        f0 = float(h["freq"])
        u = t.keys("UP")
        t.check(abs(float(hf_line(u)["freq"]) - (f0 + step_khz / 1000)) < 1e-6, "▲: one step up", hf_line(u))
        u = t.knob(-2)
        t.check(abs(float(hf_line(u)["freq"]) - (f0 - step_khz / 1000)) < 1e-6, "knob -2: two steps down", hf_line(u))
        f1 = float(hf_line(u)["freq"])
        t.r.down("SIDE2")
        try:
            t.r.knob(1)
        finally:
            t.r.up("SIDE2")
        u = t.ui(0.3)
        t.check(abs(float(hf_line(u)["freq"]) - (f1 + 1.0)) < 1e-6, "SIDE2 + knob: 1 MHz", hf_line(u))
        b0 = hf_line(u)["band"]
        u = t.keys("RIGHT", settle=0.3)
        t.check(hf_line(u)["band"] not in ("", b0), "▶: the next band", (b0, hf_line(u)))
        m0 = hf_line(u)["mode"]
        u = t.keys("STAR", settle=0.3)
        t.check(hf_line(u)["mode"] != m0, "*: the next mode", (m0, hf_line(u)))
        u = t.keys("7 STAR 1")
        t.eq(hf_line(u)["entry"], "Go to 7.1 MHz", "digits and * type a frequency")
        u = t.keys("MENU", settle=0.3)
        t.eq((hf_line(u)["freq"], hf_line(u)["entry"]), ("7.10000", ""), "OK enters it")
        u = t.keys("HASH")
        t.eq(u.id, "Recall", "#: the recent list")
        u = t.keys("BACK")
        t.eq(u.id, "Hf", "BACK: HF mode again")
        u = t.press("MENU", long=True)
        t.eq([r["name"] for r in u.rows][:6], ["Step", "Bandwidth", "AGC", "LNA", "Fine tune", "Recent list"],
             "hold MENU: HF's options")
        u = t.keys("0")
        t.eq(u.id, "Advanced", "0 there: the full menu")
        t.home()
        u = t.press("HASH", long=True)
        t.eq(u.bar["lock"], 1, "hold #: key lock, as everywhere")
        t.press("HASH", long=True)
    finally:
        vfo.restore()


@test("HF-12", pages=("Hf", "Squelch"), xfail="SIDE1 does nothing in HF mode (hf.rs has no SIDE1 key): no monitor or "
      "squelch on chip 1")
def hf_side1_on_chip_1(t):
    """SIDE1 in HF mode on chip 1: monitor, and with the knob the squelch (HF-12)."""
    vfo = HfVfo(t)
    try:
        hf_type(t, 28.4)
        hf_mode(t, "FM")
        f0 = hf_line(t.ui())["freq"]
        t.r.down("SIDE1")
        try:
            t.r.knob(1)
            u = t.ui(0.3)
        finally:
            t.r.up("SIDE1")
        t.check(u.id == "Squelch" or hf_line(u).get("freq") == f0, "SIDE1 + knob: the squelch, not the frequency",
                (u.id, hf_line(u)))
        t.eq(u.id, "Squelch", "SIDE1 + knob: chip 1's squelch")
    finally:
        vfo.restore()


@test("HF-31", pages=("Hf",))
def hf_seek(t):
    """Hold 3 seeks up, hold 1 down, in AM below 30 MHz (the SI4732's seek): it stops on a station and keeps the
    frequency, or gives up within 15 s; in USB or on chip 1 a toast says AM below 30 MHz only."""
    vfo = HfVfo(t)
    try:
        hf_type(t, 9.4)
        hf_mode(t, "AM")
        f0 = hf_line(t.ui())["freq"]
        u = t.press("3", long=True, settle=0.05)
        t.check(hf_line(u)["seek"] == 1 or hf_line(u)["freq"] != f0, "hold 3: seeking (or landed already)", hf_line(u))
        u = t.wait(lambda u: hf_line(u).get("seek") == 0, timeout=19, what="the seek ends (a station or 15 s)")
        f1 = hf_line(u)["freq"]
        t.note(f"seek up from {f0}: stopped at {f1}")
        t.check(float(f1) >= float(f0), "up: the frequency it stopped on is above (or it gave up there)", (f0, f1))
        time.sleep(0.5)
        t.eq(hf_line(t.ui())["freq"], f1, "HF mode keeps the frequency it stopped on")
        u = t.press("1", long=True)
        u = t.wait(lambda u: hf_line(u).get("seek") == 0, timeout=19, what="the seek down ends")
        t.check(float(hf_line(u)["freq"]) <= float(f1), "down: below", hf_line(u))
        if hf_mode(t, "USB") is not None:
            u = t.press("3", long=True)
            t.eq(u.toast, {"head": "Seek", "body": "AM below 30 MHz only"}, "USB: no seek, the toast says so")
            t.eq(hf_line(u)["seek"], 0, "and nothing seeks")
    finally:
        vfo.restore()


@test("HF-3", pages=("Group", "Hf"))
def hf_patch_load_back_cancels(t):
    """Opening HF mode with the SSB patch streams it in behind a progress bar (`hf loading=N`); BACK during the
    load cancels and goes back one level (to Tools)."""
    u = t.keys("MENU 0 9")
    patch = u.row("SSB patch")["value"]
    t.need(patch == "Installed", f"no SSB patch to load ({patch!r}): nothing streams in")
    t.home()
    t.keys("MENU 0 3")
    u = t.keys("4", settle=0.05)
    t.check("loading" in hf_line(u), "the patch loading, with its progress", u.lines)
    u = t.keys("BACK")
    t.eq((u.id, u.title), ("Group", "Tools"), "BACK during the load: back one level")
    t.eq(u.state["mode"], "off", "HF mode stopped")


@test("INST-14", pages=("Group", "Hf"))
def no_patch_no_ssb_modes(t):
    """Without the SSB patch HF mode offers no USB, LSB or CW: * cycles AM (and FM on chip 1's range) only; the
    rest works (tuning, bands)."""
    u = t.keys("MENU 0 9")
    patch = u.row("SSB patch")["value"]
    t.need(patch != "Installed", "the SSB patch is installed (the check needs a radio without it)")
    vfo = HfVfo(t)
    try:
        for mhz in (7.074, 28.4):
            hf_type(t, mhz)
            seen = set()
            for _ in range(6):
                seen.add(hf_line(t.keys("STAR", settle=0.25))["mode"])
            t.check(not seen & {"USB", "LSB", "CW"}, f"{mhz} MHz: no USB, LSB or CW", seen)
        u = t.keys("RIGHT", settle=0.3)
        t.check(hf_line(u)["band"], "◀▶ still steps the bands", hf_line(u))
    finally:
        vfo.restore()


# ---- FM broadcast ----

def fm_line(u):
    return u["fm"] or {}


def fm_find_station(t, want=lambda f: f.get("rds"), seeks=12):
    """On the FM screen: seek up until a station has what `want` asks (RDS text by default); its `fm` line, or
    None."""
    u = t.ui()
    for _ in range(seeks):
        if want(fm_line(u)):
            return u
        f0 = fm_line(u).get("freq")
        t.keys("RIGHT")
        u = t.wait(lambda u: fm_line(u).get("freq") != f0, timeout=8, what="the seek lands")
        time.sleep(2.0)  # RDS needs a few seconds (BCR-30: radio text shown once complete)
        u = t.ui()
    return u if want(fm_line(u)) else None


def fm_back(t, freq):
    """The FM screen's station back to `freq` (MHz text) by the knob at the FM step."""
    u = t.ui()
    if u.id != "Fm":
        u = t.keys("MENU 0 3 1")
    step = t.get("0 3 1 2")
    khz = float(step.split()[0])
    now = float(fm_line(u).get("freq", freq))
    n = round((float(freq) - now) * 1000 / khz)
    while n:
        k = max(-20, min(20, n))
        t.knob(k)
        n -= k
    got = fm_line(t.ui()).get("freq")
    if got != freq:
        t.note(f"FM screen left on {got}, was {freq}")


@test("BCR-2.2", "BCR-2.3", needs=("fm-station",), pages=("Fm",))
def fm_screen_rds_and_stereo(t):
    """The FM screen on a station with RDS shows its text (radio text, else the station name), and STEREO on a
    stereo station (the simulator's 98.1 MHz sends both)."""
    u = t.keys("MENU 0 3 1")
    f0 = fm_line(u)["freq"]
    try:
        u = fm_find_station(t, lambda f: f.get("rds") and f.get("stereo") == 1)
        t.check(u is not None, "a station with RDS and the stereo pilot within 12 seeks", fm_line(t.ui()))
        f = fm_line(u)
        t.check(f["rds"].strip(), "its RDS text shows", f)
        t.eq(f["stereo"], 1, "STEREO")
        t.check(f["dbuv"] > 0, "a signal in dBuV", f)
        t.note(f"{f['freq']} MHz: {f['rds']!r}, {f['dbuv']} dBuV, SNR {f['snr']} dB")
    finally:
        fm_back(t, f0)


def bcast_station(t):
    """A local station's frequency (MHz text): the simulator's 98.1, else one the FM screen's seek finds with RDS."""
    if t.sim:
        return "98.1"
    u = t.keys("MENU 0 3 1")
    f0 = fm_line(u)["freq"]
    try:
        u = fm_find_station(t)
        t.need(u is not None, "no station with RDS found by seeking")
        return fm_line(u)["freq"]
    finally:
        fm_back(t, f0)
        t.home()


@test("BCR-26.1", "BCR-26.2", "BCR-26.3", needs=("fm-station",), pages=("Fm", "Home"))
def broadcast_card_report(t):
    """A slot tuned to a local station: its card has the frequency, the signal (dBuV) and SNR, STEREO when the
    pilot is heard, and the RDS text (the radio text whole, else the station name, else none: "No RDS"), the same
    the FM screen reads."""
    f = bcast_station(t)
    t.cmd(f"slot A vfo {f}")
    t.home()
    t.wait(lambda u: u.bcast.get("A", {}).get("dbuv", 0) > 0, timeout=8, what="the broadcast card's report")
    time.sleep(3.0)  # the radio text whole (BCR-30)
    u = t.ui()
    a, b = u.cards["A"], u.bcast["A"]
    t.eq(a["kind"], "broadcast", f"A on {f}")
    t.check(a["freq"].startswith(f), "the card's frequency", a["freq"])
    t.check(b["dbuv"] > 0 and b["snr"] >= 0, "the signal and SNR", b)
    t.check(b["rds"], "the RDS text (radio text, else the station name)", b)
    u = t.keys("MENU 0 3 1")
    fm = fm_line(u)
    t.eq(b["rds"], fm["rds"], "the card's text is the FM screen's for the same station")
    t.eq(b["stereo"], fm["stereo"], "STEREO as the FM screen has it")


@test("BCR-26.6", "BCR-26.7", needs=("fm-station",), pages=("Home",))
def broadcast_text_on_each_layout(t):
    """The RDS text and the signal row on each home layout with the broadcast slot active (single + scope: the
    signal in dBuV with the SNR and STEREO, the text under them)."""
    f = bcast_station(t)
    t.cmd(f"slot A vfo {f}")
    time.sleep(3)
    for layout in ("Watch", "Watch + activity", "Single + scope"):
        t.set("0 7 2", layout)
        t.home()
        u = t.ui(1.0)
        t.eq(u.cards["A"]["kind"], "broadcast", f"{layout}: A is the broadcast card")
        b = u.bcast.get("A", {})
        t.check(b.get("rds") and b.get("dbuv", 0) > 0, f"{layout}: the RDS text and the signal", b)


@test("BCR-29.1", pages=("Counter", "Hf", "Home", "Scope", "ToneSearch"))
def mode_screens_stop_the_broadcast(t):
    """A broadcast slot playing (mode=fm): the band scope, tone search, the counter and HF mode each stop it while
    up (their own mode runs, fm=0); home again, it plays."""
    stopped = t.stop_scans()
    try:
        t.cmd(f"slot A vfo {'98.1' if t.sim else '100.1'}")
        u = t.wait(lambda u: u.state.get("mode") == "fm" and u.state.get("fm") == 1, timeout=5,
                   what="the broadcast slot plays")
        for keys, want in (("1:long", "scope"), ("MENU 0 3 6", "tone"), ("MENU 0 3 3", "counter"),
                           ("MENU 0 3 4", "hf")):
            t.home()
            u = t.keys(keys, settle=0.5)
            u = t.wait(lambda u: u.state.get("mode") == want, timeout=5, what=f"{keys}: {want} runs")
            t.eq(u.state["fm"], 0, f"{keys}: the broadcast stopped")
            u = t.home()
            u = t.wait(lambda u: u.state.get("mode") == "fm" and u.state.get("fm") == 1, timeout=5,
                       what=f"after {keys}: the broadcast again")
    finally:
        t.put_back(stopped)


@test("BCR-29.2", pages=("Home",))
def strip_beside_the_broadcast(t):
    """Single + scope: with the broadcast slot active the broadcast takes the strip's place (mode=fm); with a
    normal slot active the strip sweeps (mode=strip) beside the broadcast, which plays on (fm=1)."""
    stopped = t.stop_scans()
    try:
        t.set("0 7 2", "Single + scope")
        t.cmd("slot A vfo 98.1" if t.sim else "slot A vfo 100.1")
        t.cmd(f"slot B vfo {QUIET}")
        t.cmd("slot A active")
        u = t.home()
        u = t.wait(lambda u: u.state.get("fm") == 1, timeout=5, what="A plays broadcast")
        time.sleep(1.5)
        u = t.ui()
        t.eq(u.state["mode"], "fm", "the broadcast slot active: no strip")
        t.cmd("slot B active")
        u = t.wait(lambda u: u.state.get("mode") == "strip", timeout=6, what="B active: the strip sweeps")
        t.eq(u.state["fm"], 1, "the broadcast plays on beside it")
    finally:
        t.put_back(stopped)


@test("BCR-29.4", slow=True, pages=("Fm", "Home"))
def sleep_timer_ends_the_fm_screen_only(t):
    """The sleep timer at its shortest ends the FM screen's broadcast (home, a toast); a broadcast slot plays on
    past it until tuned out."""
    u = t.keys("MENU 0 3 1 5")
    t.eq(u.sel["name"], "Sleep timer", "MENU 0 3 1 5")
    t.knob(1)
    shortest = t.get("0 3 1 5")
    t.note(f"sleep timer {shortest}")
    mins = float(shortest.split()[0])
    t.home()
    u = t.keys("MENU 0 3 1")
    t.eq(u.id, "Fm", "the FM screen")
    u = t.wait(lambda u: u.id == "Home", timeout=mins * 60 + 10, what="the sleep timer ends the FM screen",
               every=2.0)
    t.check(u.state["fm"] == 0, "the broadcast ended", u.state)
    t.cmd("slot A vfo 98.1" if t.sim else "slot A vfo 100.1")
    t0 = time.time()
    while time.time() - t0 < mins * 60 + 10:
        u = t.ui(2.0)
        t.check(u.state.get("fm") == 1 or u.cards["A"].get("state") == "muted", "the broadcast slot plays on",
                (u.state, u.cards["A"]))
    t.cmd(f"slot A vfo {QUIET}")
    t.eq(t.ui(0.5).state["fm"], 0, "tuned out: it ends")


# ---- The band scope ----

def scope_line(u):
    return u["scope"] or {}


@test("SCOPE-5", pages=("Home", "Scope"))
def scope_leaves_other_path_points_blank(t):
    """A sweep across 64 MHz (or 300 MHz) leaves the points needing the other relay state (or chip) blank: from
    63.9 MHz at 25 kHz, the 26 bars past 64 MHz; the relay (PC4, bench builds) doesn't move."""
    t.need(t.get("0 3 2 1") == "60" and t.get("0 3 2 2") == "25 kHz", "the scope's default 60 bars of 25 kHz")
    stopped = t.stop_scans()
    try:
        for f, need_blank in (("63.9", 26), ("299.9", 26)):
            t.cmd(f"slot A vfo {f}")
            t.cmd("slot A active")
            t.home()
            pc4 = t.pin("PC4") if t.r.bench and not t.sim else None
            t.press("1", long=True)
            u = t.wait(lambda u: scope_line(u).get("sweeps", 0) >= 2, timeout=10, what=f"{f}: two sweeps")
            s = scope_line(u)
            t.eq(s["blank"], need_blank, f"{f} MHz: the bars on the far side blank")
            if pc4 is not None:
                t.eq(t.pin("PC4"), pc4, f"{f} MHz: PC4 as before the sweep")
            t.home()
    finally:
        t.put_back(stopped)


@test("SCOPE-12.2", needs=("keyup",), pages=("Home", "Scope"))
def scope_ignores_ptt_while_sweeping(t):
    """The scope sweeping, a remote key-up: refused, the toast "Listen first: SIDE1 at the marker", keyed=- and
    never the Transmit screen (tx-dry or the simulator, on 145.495 MHz)."""
    stopped = t.stop_scans()
    try:
        t.cmd("slot A vfo 145.495 offset off")
        t.cmd("slot A active")
        t.home()
        t.press("1", long=True)
        t.wait(lambda u: scope_line(u).get("sweeps", 0) >= 1, timeout=10, what="the scope sweeping")
        refused, why = t.keyup_refused()
        u = t.ui(0.1)
        t.check(u.id == "Scope", "still the scope, not the Transmit screen", u.id)
        t.eq(u.state["keyed"], "-", "nothing keyed")
        t.check(u.toast and "Listen first" in u.toast["head"] + " " + u.toast["body"], "the toast", u.toast)
        t.note(f"refused: {why}")
    finally:
        t.put_back(stopped)


@test("SCOPE-13", pages=("Home", "Scope"))
def scope_pauses_watch_and_scan(t):
    """While the scope is open, watch and the scans pause (the bar's watch and scan icons go, mode=scope);
    leaving resumes them."""
    t.set("0 1 1", "A+B")
    u = t.home()
    w0 = u.bar["watch"]
    t.check(w0, "watch A+B shows in the bar", u.bar)
    u = t.press("1", long=True)
    u = t.wait(lambda u: u.state.get("mode") == "scope", timeout=5, what="the scope runs")
    t.eq((u.bar["watch"], u.bar["scan"]), (0, 0), "the scope open: watch and scan paused")
    u = t.home()
    u = t.wait(lambda u: u.bar["watch"] == w0, timeout=5, what="home: watch resumes")


# ---- The crossband repeater's start page ----

def xb_checks(u):
    return [ln.split(" ", 2)[1:] for ln in u.lines if ln.startswith("check ")]


@test("XB-18", "SAFE-12.3", needs=("tx", "tx-dry"), pages=("Crossband",))
def crossband_start_checks(t):
    """The crossband start page checks transmit on each side's receive frequency: a side with a shift warns
    "Simplex: shifts are ignored" and the check is on the receive frequency; a side outside the band plan
    (160.000 MHz) fails, naming the side, and # can't start it (a remote # meets the same checks: XB-20.2). On
    1.25 m the band plan allows it; the 0.25 W cap is the Radio task's (not on the start page)."""
    t.cmd("slot A vfo 145.495 offset -0.6")
    t.cmd("slot B vfo 446.475 offset off")
    u = t.keys("MENU 0 3 5")
    t.eq(u.id, "Crossband", "MENU 0 3 5")
    rows = {r["name"]: r["value"] for r in u.rows}
    t.need(rows.get("Side 1", "").startswith("A ") and rows.get("Side 2", "").startswith("B "),
           f"crossband's sides are A and B here ({rows.get('Side 1')}, {rows.get('Side 2')})")
    t.eq(rows["Side 1"], "A 145.49500", "side 1 shows A's receive frequency")
    checks = xb_checks(u)
    t.check(["warn", '"Simplex: shifts are ignored"'] in checks, "A's shift: the simplex warning", checks)
    t.check(not any(k == "fail" for k, _ in checks), "145.495 with a -600 kHz shift: the receive frequency is "
            "checked (144.895 would be checked too, and both are in the band plan)", checks)
    t.home()
    t.cmd("slot A vfo 160.000 offset off")
    u = t.keys("MENU 0 3 5")
    checks = xb_checks(u)
    t.check(["fail", '"Side 1 can\'t transmit there"'] in checks, "side 1 on 160.000 MHz: refused, naming it", checks)
    u = t.keys("HASH")
    t.eq(u.state["mode"], "off", "# starts nothing")
    t.home()
    t.cmd("slot A vfo 222.000 offset off")
    u = t.keys("MENU 0 3 5")
    t.note(f"222.000 MHz: {xb_checks(u)}")


def xb_line(u):
    return u["xb"] or {}


def xb_sides(t):
    """The repeater's sides on the two approved test frequencies (A 145.495, B 446.475, no tones, low power), so a
    start from the PC can only ever relay there; the runner puts the slots back."""
    t.cmd("slot A vfo 145.495 fm low tone off offset off")
    t.cmd("slot B vfo 446.475 fm low tone off offset off")


@test("XB-20.1", "XB-20.2", "XB-20.4", "XB-20.5", needs=("tx-dry", "keyup"), pages=("Crossband", "Home"))
def crossband_from_the_link(t):
    """`xb start` from home opens the repeater's screen and starts it (`xb running=1`, by=cmd:...); `xb` says so;
    SIDE1 held listens without stopping it (listen=1); a digit stops it and does nothing else (the start page stays,
    a toast); remote keys' # on the start page starts it again (by=keys:...), and `xb stop` stops it. On the radio
    only on tx-dry (PA held at 0) and on the two approved test frequencies."""
    xb_sides(t)
    t.home()
    ok, out = t.cmd_err("xb start")
    t.check(ok, "xb start", out)
    u = t.ui()
    t.eq(u.id, "Crossband", "its screen is up (XB-20.4)")
    x = xb_line(u)
    t.eq(x.get("running"), 1, "running")
    t.check(str(x.get("by", "")).startswith("cmd:"), "started by the command (XB-20.6's by=)", x)
    t.eq(u.hint("Any key"), "Stop", "any key stops")
    t.check(t.cmd("xb").startswith("Crossband: running"), "xb tells", t.cmd("xb"))
    u = t.keys("SIDE1:down")
    t.eq(xb_line(u).get("listen"), 1, "SIDE1 held: listening (XB-15)")
    t.eq(xb_line(u).get("running"), 1, "SIDE1 doesn't stop it")
    u = t.keys("SIDE1:up")
    t.eq(xb_line(u).get("listen"), 0, "let go: not listening")
    u = t.keys("5")
    t.eq(xb_line(u).get("running"), 0, "a digit stops it (XB-20.5)")
    t.eq(u.id, "Crossband", "and does nothing else: the start page")
    t.check(u.toast and u.toast["head"] == "Repeater stopped", "a toast says so", u.toast)
    u = t.keys("HASH")
    x = xb_line(u)
    t.eq(x.get("running"), 1, "remote keys' # starts it (XB-20.2)")
    t.check(str(x.get("by", "")).startswith("keys:"), "started by remote keys", x)
    t.check(t.cmd("xb stop").startswith("Crossband stopped"), "xb stop", None)
    t.eq(xb_line(t.ui()).get("running"), 0, "stopped")
    ok, out = t.cmd_err("xb stop")
    t.check(not ok and "isn't running" in out, "xb stop again: refused", out)
    t.home()


@test("XB-20.6", needs=("tx-dry", "cablelog"), where="radio", pages=("Crossband", "Home"))
def crossband_starts_and_stops_logged(t):
    """Over Bluetooth with the cable log on: `xb start` logs "xb: started by cmd:bt", a remote digit "xb: stopped by
    keys:bt: a key", a remote # "xb: started by keys:bt" and `xb stop` "xb: stopped by cmd:bt: xb stop"."""
    import re
    xb_sides(t)
    t.home()
    log = t.cable_log()
    steps = (("cmd", "xb start", r"xb: started by cmd:bt"), ("keys", "5", r"xb: stopped by keys:bt: a key"),
             ("keys", "HASH", r"xb: started by keys:bt"), ("cmd", "xb stop", r"xb: stopped by cmd:bt: xb stop"))
    for how, what, want in steps:
        t0 = time.time()
        if how == "cmd":
            t.cmd(what)
        else:
            t.keys(what)
        t.check(log.wait(re.escape(want), t0, 5), f"{what}: {want!r} in the cable log",
                [ln for _, ln in log.since(t0)][-6:])
    t.home()


@test("HF-19", pages=("Group", "Hf"))
def hf_patch_streams_on_each_entry(t):
    """With the SSB patch installed every entry into HF mode streams it in (the load's progress, `hf loading=N`,
    shows each time); changing the mode inside HF mode doesn't load it again."""
    u = t.keys("MENU 0 9")
    t.need(u.row("SSB patch")["value"] == "Installed", "no SSB patch")
    vfo = HfVfo(t)
    try:
        for i in range(2):
            t.home()
            t.keys("MENU 0 3")
            u = t.keys("4", settle=0.05)
            t.check("loading" in hf_line(u), f"entry {i + 1}: the patch streams in", u.lines)
            t.wait(lambda u: "freq" in hf_line(u), timeout=20, what="loaded")
        seen = []
        for _ in range(5):
            t.r.press("STAR")
            for _ in range(3):
                seen.append(hf_line(t.ui(0.05)))
        t.check(not any("loading" in h for h in seen), "mode changes: no load again", seen[:4])
    finally:
        vfo.restore()
