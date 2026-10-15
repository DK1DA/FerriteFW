#!/usr/bin/env python3
"""The transmit bench in one command: the on-air checks against the black radio (a `tx-dry` build) and an RTL-SDR,
each run's numbers stored, compared with the previous run of the same configuration, and one summary page.

    tools/bench.py --dry-plan                          # what it would do; touches nothing
    tools/bench.py --i-know-its-tx-dry                 # the default matrix: VHF wide, VHF NFM, UHF wide
    tools/bench.py --flash target/ferrite-txdry.btf    # flash first (flash.py --backup), wait for the boot, then run
    tools/bench.py --runs vhf-wide --refusals --keepalive --timeout --i-know-its-tx-dry
    tools/bench.py --load DIR [--build LABEL]          # re-analyse saved captures (DIR searched for <run>.u8)
    tools/bench.py --selftest                          # a fake radio and synthetic IQ: the whole flow and the gates
    tools/bench.py --selftest --sim tcp:127.0.0.1:7300 # the same against the simulator's link (fake SDR)
    tools/bench.py --auto [--dry-plan]                 # what changed since the last on-radio runs decides: the UI
                                                       # tests (tools/uitest.py), the SDR suite, both + a reboot check;
                                                       # only screen files (screens/<page>.rs): uitest.py --page <page>
    tools/bench.py --auto --all | --ui | --rf          # everything, or one side, forced
    tools/bench.py --auto --dry-plan --diff-files crates/ferrite-app/src/screens/home.rs   # a made-up change

Per run (TERM-34, LINK-20, LINK-33, LINK-34; everything goes over the link, nothing through the keypad):
  1. save slot A as it is (`slot A`; its VFO's if it is on a channel), the active slot, the end tone (0 8 5 4) and
     the TX timeout (0 8 1 1);
  2. `slot A vfo FREQ MODE low tone off`, `slot A active`, `set 0 8 5 4 Off`, the TX timeout raised to 120 s if it
     is under 90 s, `txtest suite`;
  3. start rtl_sdr, key over the link (LINK-20 keepalives every 0.5 s) for the suite's 60 s and the 1 kHz hold, unkey;
  4. after all the runs: everything put back (in `finally`, whatever happened), then each capture measured with
     sdrsuite.py's analysis, the numbers stored and compared.

Safety gates: nothing is keyed unless all of these hold, checked right before every KEY:
  - the build is `tx-dry` (PA drive held at 0): HELLO's build byte says tx-dry with the bench bit (LINK-35: 0x82,
    decision 151; a radio that says receive or tx is refused outright, --i-know-its-tx-dry or not), or its version
    string says so ("dry"/"TX DRY"), or --flash flashed an image with the "TX DRY" boot line in it in this same
    invocation. Firmware from before the build byte doesn't say it, so without one of these the bench refuses,
    unless --i-know-its-tx-dry is given (a warning is printed and recorded);
  - the build transmits at all (`txtest` isn't refused as a receive-only build);
  - the slot is on its VFO at one of the approved test frequencies (config `test_freqs`), with no offset and no
    split (TX on the same frequency), power low, not scanning, not APRS, and it is the active slot;
  - on the cable, the KEY on is answered at once: a `tx` build holds every answer while keyed (TX-43) and a `tx-dry`
    build doesn't; no answer within 0.6 s means a real transmitter, so it unkeys at once and stops.
  The refusal checks (--refusals) key on 160.000, 222.000 and 243.000 on purpose: those must be refused by the radio
  (each with its reason); if one is not, the bench unkeys at once and fails the check. The other gates still hold.
  KEY off is always sent in `finally` (and at exit), refused or not; Ctrl-C and SIGTERM end there too.

Extra checks: --refusals (remote PTT refused out of band, with the expected reason; no carrier on the SDR),
--keepalive (keepalives stopped while keyed: the carrier must drop within 2.5 s of the last), --timeout (TX timeout
30 s, keyed 45 s with keepalives: the carrier ~30 s; the timeout put back).

Results: under --results (default ../bench-results, beside the ferrite checkout) one directory per session,
<date-time>_<build>, holding session.json, one <run>.json per run (key numbers and curves), each run's sdrsuite
page, txt and charts in <run>/, the capture (<run>.u8, unless --no-keep-captures) and summary.html, a single
self-contained page. The build is the radio's version plus --build LABEL, or the flashed image's hash, or the host
checkout's git hash (marked "host"). Each run is compared with the newest earlier session holding the same
configuration (run name, frequency, mode); changes beyond the tolerances (config `tolerances`: [glob, abs|rel|exact,
value], first match wins) are flagged; an `info` tolerance shows the change but never flags it. --config FILE.json overrides any default (see DEFAULTS).

Rules: bench/results.json (uitest's shape) holds each check tagged with the manual rules whose stated numbers it
measures (rule_tests(): "all" or "part" of a rule); --record puts them (and a --auto run's UI results) into the
test plan with tools/record_results.py; --rule-results SESSION writes the file for an older session.

Needs numpy and matplotlib (the project's .venv), pyserial for the cable, rtl_sdr for a capture.
"""
import argparse
import atexit
import contextlib
import datetime
import fnmatch
import hashlib
import html
import json
import math
import os
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
FERRITE = os.path.dirname(HERE)
ROOT = os.path.dirname(FERRITE)
sys.path.insert(0, HERE)
import ferrite_link as fl  # noqa: E402
import sdrsuite  # noqa: E402
import sdrtx  # noqa: E402

TX_TIMEOUT = "0 8 1 1"
END_TONE = "0 8 5 4"
KEY_ANSWER_S = 0.6  # TX-43: a tx-dry build answers KEY on at once; a tx build holds it while keyed

DEFAULTS = {
    "port": "/dev/cu.usbserial-114220",
    "slot": "A",
    "test_freqs": [145.495, 446.475],  # MHz; the only frequencies the bench keys on (user approved, Oct 13 2026)
    "runs": [
        {"name": "vhf-wide", "mhz": 145.495, "mode": "fm"},
        {"name": "vhf-nfm", "mhz": 145.495, "mode": "nfm"},
        {"name": "uhf-wide", "mhz": 446.475, "mode": "fm"},
    ],
    "aliases": {"wide": "vhf-wide", "nfm": "vhf-nfm", "uhf": "uhf-wide"},  # --load: older capture names
    "refusals": [
        {"mhz": 160.000, "reason": "outside the band plan"},
        {"mhz": 222.000, "reason": "1.25 m"},
        {"mhz": 243.000, "reason": "emergency"},
    ],
    "timeout_min_s": 90,
    "timeout_raise": "120 s",
    "suite_key_s": 63.5,  # the suite's 59.95 s and ~3.5 s of the 1 kHz hold
    "sdr_lead_s": 2.0,
    "sdr_tail_s": 3.0,
    "sdr_gain": 0.9,  # the R820T's lowest fixed gain: rtl_sdr -g 0 means AUTOMATIC, which saturated a UHF run (Oct 14, 2026)
    "sdr_ppm": 0,
    "keepalive": {"alive_s": 3.0, "wait_s": 5.0, "max_drop_s": 2.5},
    "timeout_test": {"set": "30 s", "expect_s": 30.0, "key_s": 45.0, "tol_s": 1.5},
    "refusal_capture_s": 4.0,
    "results": os.path.join(ROOT, "bench-results"),
    "backup": os.path.join(ROOT, "backups", "blackradio", "flash.bin"),
    "tolerances": [
        ["markers_found", "exact", 0],
        ["hold", "exact", 0],
        ["dcs.code", "exact", 0],
        # The tone generator's swell and timing per REG_40 burst vary run to run on an unchanged build (start_frac
        # swung -2.2 to +0.7, jitter 14 to 41 us): shown, never flagged. Their deviation and twist stay strict.
        ["reg40.*.start_frac", "info", 0],
        ["reg40.*.tau_ms", "info", 0],
        ["reg40.*.jitter_rms_us", "info", 0],
        # Exact at 0 errors; a run already full of errors (6855 against 6878) isn't a change.
        ["fsk4.errors", "rel", 0.05],
        ["fsk4.symbols", "rel", 0.2],
        # Absolute, so a UHF carrier offset of ~70 Hz (the SDR's ppm) moves all four together; fsk4.ratio (strict)
        # catches a real change of scale, a flip moves them by thousands.
        ["fsk4.levels.*", "abs", 100],
        ["fsk4.ratio", "abs", 0.1],
        ["sweep.*.low_hz", "rel", 0.05],
        ["sweep.*.high_hz", "abs", 30],
        ["*.twist_db", "abs", 0.5],
        ["snr_db", "abs", 6],
        ["*_db", "abs", 0.5],
        ["*.jitter_rms_us", "abs", 5],
        ["*.tau_ms", "abs", 10],
        ["*.t90_ms", "abs", 20],
        ["*.start_frac", "abs", 0.08],
        ["endtone.*.hz", "abs", 1.0],
        ["endtone.*.ms", "abs", 10],
        ["ctcss.*.hz", "abs", 0.1],
        ["dcs.baud", "abs", 0.5],
        ["dcs.windows_frac", "abs", 0.1],
        ["reg40.x3000", "abs", 8],
        ["reg40.lin3000", "abs", 8],
        ["carrier.offset_hz", "abs", 150],
        ["timing_ppm", "abs", 5],
        ["keyed_s", "abs", 1.0],
        ["*", "rel", 0.05],  # deviations and the rest
    ],
}


class BenchError(Exception):
    pass


class SafetyError(BenchError):
    """A gate refused: nothing is keyed (or it unkeyed at once)."""


class TermError(BenchError):
    pass


def say(*a):
    print(*a, flush=True)


# ---- Clocks (the selftest's is virtual, so a 60 s key-up takes no time) ----


class Clock:
    @staticmethod
    def time():
        return time.time()

    @staticmethod
    def sleep(s):
        time.sleep(max(s, 0))


class FakeClock:
    def __init__(self):
        self.t = 1_000_000.0

    def time(self):
        return self.t

    def sleep(self, s):
        self.t += max(s, 0)


# ---- The radio over the link ----


def mhz10(mhz):
    return int(round(mhz * 100000))


def parse_slot_line(line):
    """`slot A`'s line: "A* vfo 146.52000 mode=FM power=low tone=off offset=-0.60000" or
    "B  ch 12 "Club" 443.47500 mode=NFM ... txtone=100.0 rxtone=off"."""
    line = line.strip()
    m = re.match(r'([ABC])(\*?)\s+(?:(vfo|aprs)|ch (\d+)(?: "[^"]*")?)\s+(\d+\.\d+)\s*(.*)$', line)
    if not m:
        raise BenchError(f"can't read the slot line {line!r}")
    s = dict(slot=m.group(1), active=m.group(2) == "*", vfo=m.group(3) == "vfo", aprs=m.group(3) == "aprs",
             channel=int(m.group(4)) if m.group(4) else None, freq=m.group(5), line=line)
    for k, v in re.findall(r"(\w+)=(\S+)", m.group(6)):
        s[k] = v
    if "tone" in s:
        s["txtone"] = s["rxtone"] = s["tone"]
    s["scanning"] = " scanning" in line
    return s


class Radio:
    """The radio's link (ferrite_link.py's framing): HELLO, INFO, SLOT, CMD/MORE (terminal lines), KEY."""

    def __init__(self, spec, clock=None):
        self.spec = spec
        self.cable = not spec.startswith("tcp:")
        self.port = fl.Port(spec)
        self.link = fl.Link(self.port)
        self.last = time.time()
        self.hello_ans = self.hello()

    def close(self):
        with contextlib.suppress(Exception):
            self.port.close()

    def _ask(self, ty, payload=b"", timeout=2.0):
        if time.time() - self.last > 8:  # the link lets the cable go 10 s after the last frame (TERM-2)
            self.port.write(b"\x00")
            time.sleep(0.05)
        try:
            s, rest = self.link.ask(ty, payload, timeout=timeout)
        finally:
            self.last = time.time()
        return s, rest

    def hello(self):
        s, r = self._ask(fl.HELLO, bytes([fl.PROTOCOL]) + bytes(8) + b"bench.py")
        if s != 0:
            raise BenchError(f"HELLO: {fl.why(s, r)}")
        if not r[2]:
            raise BenchError("the radio's PIN lock is on (LINK-30); the bench uses the cable")
        ver, rest = fl.text(r[3:])
        return dict(protocol=r[0], version=ver, build=fl.build_name(rest), bench=fl.build_bench(rest))

    def info(self):
        s, r = self._ask(fl.INFO)
        if s != 0:
            raise BenchError(f"INFO: {fl.why(s, r)}")
        ver, r = fl.text(r)
        chans, zones, presets, slen, region = struct.unpack("<HBBHB", r[:7])
        call, _ = fl.text(r[7:])
        return dict(version=ver, channels=chans, zones=zones, presets=presets, region=region, callsign=call)

    def term(self, line):
        """One terminal command line (LINK-34); its output. A refusal raises TermError with the radio's reason."""
        s, rest = self._ask(fl.CMD, line.encode("ascii"))
        out = []
        for _ in range(400):
            if s != 0:
                raise TermError(f"{line}: {fl.why(s, rest)}")
            out.append(rest[1:].decode("ascii", "replace"))
            if rest[0] == 0:
                return "".join(out)
            if len(rest) == 1:
                time.sleep(0.05)
            s, rest = self._ask(fl.MORE)
        raise TermError(f"{line}: no end to its output")

    def slot(self, n):
        """LINK-33: the slot's working fields, binary."""
        s, r = self._ask(fl.SLOT, bytes([n]))
        if s != 0:
            raise BenchError(f"SLOT: {fl.why(s, r)}")
        sl, act, ch, f, mode, pw, rxt, txt, off, flags = struct.unpack("<BBHIBBHHiB", r[:19])
        return dict(active=bool(act), channel=None if ch == 0xFFFF else ch, freq10=f, mode=mode, power=pw, rx_tone=rxt,
                    tx_tone=txt, offset=off, split=bool(flags & 1), aprs=bool(flags & 2), scanning=bool(flags & 4))

    # KEY (LINK-20), a step at a time so the bench's loop owns the timing.
    def key_on(self, slot_code):
        """(status, reason) of KEY on, or None when no answer came within KEY_ANSWER_S."""
        self.last = time.time()
        seq = self.link.send(fl.KEY, bytes([fl.PTT, fl.PTT_ON, slot_code]))
        end = time.time() + KEY_ANSWER_S
        while time.time() < end:
            for t, s, status, rest in self.link.answers():
                if t == fl.KEY | 0x80 and s == seq:
                    return status, ("" if status == 0 else fl.why(status, rest))
        return None

    def key_alive(self):
        self.last = time.time()
        self.link.send(fl.KEY, bytes([fl.PTT, fl.PTT_ALIVE]))

    def poll_unkey(self):
        """A KEY answer with a failure: the radio unkeyed (its reason), else None."""
        for t, s, status, rest in self.link.answers():
            if t == fl.KEY | 0x80 and status != 0:
                return fl.why(status, rest)
        return None

    def key_off(self):
        """KEY off; True when answered within 3 s (held answers come out first, TX-43)."""
        seq = self.link.send(fl.KEY, bytes([fl.PTT, fl.PTT_OFF]))
        end = time.time() + 3
        while time.time() < end:
            for t, s, status, rest in self.link.answers():
                if t == fl.KEY | 0x80 and s == seq:
                    self.last = time.time()
                    return True
        return False


# ---- The SDR ----


class Sdr:
    """rtl_sdr writing a capture file in the background while the bench keys."""

    def __init__(self, cfg, clock):
        self.cfg, self.clock, self.p = cfg, clock, None

    def start(self, mhz, seconds, path):
        if not shutil.which("rtl_sdr"):
            raise BenchError("rtl_sdr not found (brew install librtlsdr)")
        n = int(seconds * sdrtx.FS)
        cmd = ["rtl_sdr", "-f", str(int(mhz * 1e6 + sdrtx.OFF)), "-s", str(sdrtx.FS), "-g", str(self.cfg["sdr_gain"]),
               "-p", str(self.cfg["sdr_ppm"]), "-n", str(n), path]
        self.path, self.seconds, self.t0 = path, seconds, self.clock.time()
        self.p = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)

    def finish(self):
        left = self.t0 + self.seconds - self.clock.time()
        try:
            self.p.wait(timeout=max(left, 0) + 20)
        except subprocess.TimeoutExpired:
            self.p.kill()
            raise BenchError("rtl_sdr didn't finish")
        if not os.path.exists(self.path) or os.path.getsize(self.path) < 1000:
            raise BenchError(f"rtl_sdr wrote nothing: {self.p.stderr.read().decode(errors='replace')[-300:]}")
        self.p = None
        return np.fromfile(self.path, np.uint8)

    def stop(self):
        if self.p and self.p.poll() is None:
            self.p.kill()


# ---- Measurements ----


def carrier_runs(raw, min_s=0.03, gap_s=0.05):
    """The keyed spans (s from the capture's start): power 20 dB over the floor in 10 ms blocks, gaps under `gap_s`
    joined. The capture must be mostly unkeyed (the floor is its 5th percentile)."""
    x = sdrtx.baseband(raw)
    fs = sdrtx.FS / sdrtx.DEC
    blk = int(fs * 0.01)
    p = np.abs(x) ** 2
    pb = p[: len(p) // blk * blk].reshape(-1, blk).mean(1)
    floor = np.percentile(pb, 5)
    if floor > 1e3:
        floor = 1.0
    on = pb > floor * 100
    d = np.diff(np.concatenate([[0], on.astype(int), [0]]))
    s, e = np.nonzero(d == 1)[0], np.nonzero(d == -1)[0]
    runs = []
    for a, b in zip(s, e):
        if runs and (a - runs[-1][1]) * 0.01 < gap_s:
            runs[-1][1] = b
        else:
            runs.append([a, b])
    return [(a * 0.01, b * 0.01) for a, b in runs if (b - a) * 0.01 >= min_s]


@contextlib.contextmanager
def quiet(path):
    """sdrsuite's report goes to the terminal through sys.__stdout__: into `path` instead (None: leave it)."""
    if path is None:
        yield
        return
    with open(path, "a") as f:
        old, old2 = sys.__stdout__, sys.stdout
        sys.__stdout__ = sys.stdout = f
        try:
            yield
        finally:
            sys.__stdout__, sys.stdout = old, old2


def jsonable(x):
    if isinstance(x, dict):
        return {str(k): jsonable(v) for k, v in x.items()}
    if isinstance(x, (list, tuple)):
        return [jsonable(v) for v in x]
    if isinstance(x, np.ndarray):
        return jsonable(x.tolist())
    if isinstance(x, (np.bool_, bool)):
        return bool(x)
    if isinstance(x, (np.integer,)):
        return int(x)
    if isinstance(x, (np.floating, float)):
        return None if not math.isfinite(float(x)) else float(x)
    return x


def slug(s):
    return re.sub(r"[^a-z0-9.]+", "-", str(s).lower()).strip("-")


def metrics_of(R):
    """sdrsuite's results to flat key numbers (name -> number or string) and curves (for the charts)."""
    m, curves = {}, {}
    if not R.get("fs"):
        m["markers_found"] = 0
        return m, curves
    m["keyed_s"] = R.get("keyed_s")
    m["snr_db"] = R.get("snr")
    m["carrier.offset_hz"] = R.get("offset")
    m["timing_ppm"] = R.get("timing_ppm")
    m["markers_found"] = len(R.get("markers", {}))
    S = R.get("sections", {})
    for n, name in ((1, "data"), (2, "voice")):
        if n in S:
            m[f"sweep.{name}.low_hz"] = S[n]["low"]
            m[f"sweep.{name}.high_hz"] = S[n]["high"]
            m[f"sweep.{name}.ref_dev"] = S[n]["ref"]
            curves.setdefault("sweep", {})[name] = dict(freqs=list(S[n]["freqs"]), db=list(S[n]["db"]))
    if 3 in S:
        for lvl, a in S[3].items():
            m[f"afsk.dac{lvl}.mark_dev"] = a.get(1200)
            m[f"afsk.dac{lvl}.space_dev"] = a.get(2200)
            m[f"afsk.dac{lvl}.twist_db"] = a.get("twist")
            m[f"afsk.dac{lvl}.jitter_rms_us"] = a.get("jitter_rms_us")
    if 4 in S:
        pts = []
        for b in S[4]["bursts"]:
            k = f"reg40.0x{b['reg40']:03X}" + ("" if b["filters"] == "voice" else f"-{b['filters']}")
            m[f"{k}.dev"] = b["dev"]
            m[f"{k}.twist_db"] = b.get("twist")
            m[f"{k}.jitter_rms_us"] = b.get("jitter_rms_us")
            m[f"{k}.tau_ms"] = b["swell"]["tau_ms"]
            m[f"{k}.start_frac"] = b["swell"]["start"]
            if b["filters"] == "voice":
                pts.append([b["reg40"], b["dev"]])
        curves["reg40"] = pts
        fit = S[4].get("fit")
        if fit:
            m["reg40.x3000"] = fit.get("x3000")
            m["reg40.lin3000"] = fit.get("lin3000")
            m["reg40.fit"] = fit.get("kind")
        bf = S[4].get("by_filters", {})
        for k in ("data", "flat"):
            if k in bf and bf.get("voice"):
                m[f"reg40.0x43C.{k}_vs_voice_db"] = 20 * math.log10(max(bf[k], 1e-3) / bf["voice"])
    if 5 in S:
        a = S[5]["attack"]
        m["tone1k.steady_dev"] = S[5]["steady"]
        m["tone1k.start_frac"] = a["start"]
        m["tone1k.tau_ms"] = a["tau_ms"]
        m["tone1k.t90_ms"] = a["t90_ms"]
        for t in S[5]["tones"]:
            k = f"endtone.{slug(t['name'])}.{t['want_hz']}"
            m[f"{k}.found"] = "yes" if t["found"] else "no"
            if t["found"]:
                m[f"{k}.hz"] = t["hz"]
                m[f"{k}.ms"] = t["ms"]
                m[f"{k}.start_dev"] = t["start"]
                m[f"{k}.dev"] = t["dev"]
                m[f"{k}.end_dev"] = t["end"]
    if 6 in S:
        for x in S[6]:
            if x["kind"] == "CTCSS":
                m[f"ctcss.{x['want']:.1f}.hz"] = x["hz"]
                m[f"ctcss.{x['want']:.1f}.dev"] = x["dev"]
            else:
                m["dcs.code"] = (f"{x['code']:03o} {'I' if x['inverted'] else 'N'}" if x["code"] is not None else "none")
                m["dcs.baud"] = x["baud"]
                m["dcs.dev"] = x["dev"]
                good, tot = x["windows"]
                m["dcs.windows_frac"] = good / tot if tot else None
    if S.get(7):
        r = S[7]
        m["fsk4.errors"] = r["errors"]
        m["fsk4.symbols"] = r["symbols"]
        for i, lv in enumerate(np.asarray(r["levels"]).tolist()):
            m[f"fsk4.levels.{i}"] = lv
        m["fsk4.ratio"] = r.get("ratio")
    m["hold"] = "yes" if R.get("hold") else "no"
    return jsonable(m), jsonable(curves)


HEADLINE = [
    ("Sweep data −3 dB", ["sweep.data.low_hz", "sweep.data.high_hz"], "{:.0f}–{:.0f} Hz"),
    ("Sweep voice −3 dB", ["sweep.voice.low_hz", "sweep.voice.high_hz"], "{:.0f}–{:.0f} Hz"),
    ("AFSK DAC1 100 %", ["afsk.dac100.mark_dev", "afsk.dac100.space_dev", "afsk.dac100.twist_db"], "{:.0f} / {:.0f} Hz, {:+.2f} dB"),
    ("AFSK jitter", ["afsk.dac100.jitter_rms_us"], "{:.1f} µs rms"),
    ("REG_40 for 3.0 kHz", ["reg40.x3000"], "{:.0f}"),
    ("1 kHz attack", ["tone1k.start_frac", "tone1k.tau_ms"], "starts {:.0%}, τ {:.0f} ms"),
    ("CTCSS 100.0", ["ctcss.100.0.hz", "ctcss.100.0.dev"], "{:.2f} Hz, {:.0f} Hz dev"),
    ("DCS", ["dcs.code"], "{}"),
    ("4FSK errors", ["fsk4.errors", "fsk4.symbols"], "{} of {}"),
    ("Carrier offset", ["carrier.offset_hz"], "{:+.0f} Hz"),
]


def headline(m):
    out = []
    for name, keys, fmt in HEADLINE:
        v = [m.get(k) for k in keys]
        if any(x is None for x in v):
            continue
        with contextlib.suppress(Exception):
            out.append((name, fmt.format(*v)))
    return out


# ---- Comparison ----


def tolerance(cfg, key):
    for pat, kind, val in cfg["tolerances"]:
        if fnmatch.fnmatchcase(key, pat):
            return kind, val
    return "rel", 0.05


def compare(cfg, now, prev):
    """Rows (key, now, prev, delta, tol text, status), status ok / CHANGED / missing / new; flagged first."""
    rows = []
    for k in sorted(set(now) | set(prev)):
        a, b = now.get(k), prev.get(k)
        kind, val = tolerance(cfg, k)
        tol = {"abs": f"±{val:g}", "rel": f"±{val * 100:g} %", "exact": "same", "info": "info only"}[kind]
        if a is None and b is None:
            continue
        if b is None:
            rows.append((k, a, None, None, tol, "new"))
            continue
        if a is None:
            rows.append((k, None, b, None, tol, "missing"))
            continue
        if kind == "info":
            rows.append((k, a, b, None if isinstance(a, str) or isinstance(b, str) else a - b, tol, "ok" if a == b else "info"))
            continue
        if isinstance(a, str) or isinstance(b, str) or kind == "exact":
            rows.append((k, a, b, None, tol, "ok" if a == b else "CHANGED"))
            continue
        d = a - b
        lim = val if kind == "abs" else val * max(abs(b), 1e-9)
        rows.append((k, a, b, d, tol, "ok" if abs(d) <= lim + 1e-12 else "CHANGED"))
    order = {"CHANGED": 0, "missing": 1, "new": 2, "info": 3, "ok": 4}
    rows.sort(key=lambda r: (order[r[5]], r[0]))
    return rows


def config_key(run):
    return f"{run['name']}@{run['mhz']:.4f}-{run['mode']}"


def previous_run(cfg, run, before_session):
    root = cfg["results"]
    if not os.path.isdir(root):
        return None
    for d in sorted(os.listdir(root), reverse=True):
        if d >= before_session:
            continue
        p = os.path.join(root, d, run["name"] + ".json")
        if os.path.exists(p):
            with contextlib.suppress(Exception):
                j = json.load(open(p))
                if j.get("config_key") == config_key(run) and j.get("metrics", {}).get("markers_found"):
                    j["session_dir"] = os.path.join(root, d)
                    return j
    return None


# ---- The build ----


def image_is_tx_dry(path):
    """Whether an image (.btf or raw .bin) carries the tx-dry build's boot line."""
    data = open(path, "rb").read()
    if path.lower().endswith(".btf"):
        import btf
        data = btf.unpack(data)[0]
    return b"TX DRY" in data, hashlib.sha256(data).hexdigest()[:10]


def host_build():
    def git(*a):
        r = subprocess.run(["git", "-C", FERRITE, *a], capture_output=True, text=True)
        return r.stdout.strip() if r.returncode == 0 else ""
    h = git("rev-parse", "--short=7", "HEAD") or "unknown"
    dirty = bool(git("status", "--porcelain", "--untracked-files=no"))
    return h + ("+" if dirty else "")


def says_tx_dry(version):
    return bool(re.search(r"tx[ _-]?dry|\bdry\b", version or "", re.I))


# ---- The bench ----


class Bench:
    def __init__(self, cfg, args, radio=None, sdr=None, clock=None, log=None):
        self.cfg, self.a = cfg, args
        self.radio, self.sdr = radio, sdr
        self.clock = clock or Clock()
        self.slot_letter = cfg["slot"].upper()
        self.slot_n = "ABC".index(self.slot_letter)
        self.slot_code = fl.SLOTS[self.slot_letter.lower()]
        self.keyed = False
        self.gatelog = []
        self.warnings = []
        self.tx_dry = None  # how it is known, or None
        self.restore_notes = []
        self.saved = None
        self.out = log or say

    # -- gates --
    def gate(self, what, ok, detail=""):
        self.gatelog.append(dict(gate=what, ok=bool(ok), detail=detail))
        if not ok:
            raise SafetyError(f"refused: {what}{': ' + detail if detail else ''}")

    def check_build(self, hello, info, flashed_dry=None):
        ver = info.get("version") or hello.get("version")
        said = hello.get("build", "not given")
        if said not in ("not given", "tx-dry"):
            # LINK-35 (decision 151): the radio names its build; a receive or full-power tx build is never keyed.
            self.gate("tx-dry build", False, f"the radio says it runs a {said} build (LINK-35): flash a tx-dry image")
        if said == "tx-dry":
            self.gate("a bench build (txtest is in it)", hello.get("bench"),
                      "the build byte says tx-dry without the bench bit (0x80): txtest isn't in it")
        if said == "tx-dry" and not flashed_dry:
            self.tx_dry = "the radio says so (LINK-35 build byte: tx-dry, bench)"
        elif flashed_dry:
            self.tx_dry = f"flashed {flashed_dry} (its image has the TX DRY boot line)"
        elif says_tx_dry(ver) or says_tx_dry(hello.get("version")):
            self.tx_dry = f"the version says so ({ver})"
        elif self.a.i_know_its_tx_dry:
            self.tx_dry = "--i-know-its-tx-dry (not verifiable over the link)"
            w = (f"WARNING: the radio's version ({ver!r}) doesn't say tx-dry and nothing was flashed here; going on "
                 "because of --i-know-its-tx-dry. A `tx` build would transmit at full drive. On the cable the KEY "
                 "answer check (TX-43) still stops a `tx` build at its first key-up.")
            self.warnings.append(w)
            self.out(w)
        self.gate("tx-dry build", self.tx_dry is not None,
                  self.tx_dry or f"version {ver!r} doesn't say tx-dry (Ferrite v0.1 can't tell over the link): flash "
                                 "a tx-dry image with --flash, or check the boot screen's TX DRY line and give "
                                 "--i-know-its-tx-dry")
        try:
            t = self.radio.term("txtest")
            ok = "receive-only" not in t
        except TermError as e:
            t, ok = str(e), "receive-only" not in str(e)
        self.gate("a transmit build (txtest answers)", ok, t.strip()[:120])

    def gate_key(self, mhz_allowed, purpose):
        """Every gate on the slot, right before a KEY on."""
        st = self.radio.slot(self.slot_n)
        f = st["freq10"]
        self.gate(f"{purpose}: frequency approved", f in [mhz10(x) for x in mhz_allowed],
                  f"{f / 1e5:.5f} MHz; allowed {', '.join(f'{x:.3f}' for x in mhz_allowed)}")
        self.gate(f"{purpose}: on the VFO", st["channel"] is None and not st["aprs"], f"channel {st['channel']}")
        self.gate(f"{purpose}: no offset, no split", st["offset"] == 0 and not st["split"],
                  f"offset {st['offset'] / 1e5:+.5f} MHz{' split' if st['split'] else ''}: set the shift to simplex "
                  "on the radio, or give --clear-offset")
        self.gate(f"{purpose}: power low", st["power"] == 0, ["low", "mid", "high"][st["power"] % 3])
        self.gate(f"{purpose}: not scanning", not st["scanning"])
        self.gate(f"{purpose}: slot {self.slot_letter} active", st["active"])
        self.gate(f"{purpose}: tx-dry", self.tx_dry is not None, self.tx_dry or "")
        return st

    # -- the radio's state --
    def setting(self, path):
        t = self.radio.term(f"get {path}").strip()
        m = re.match(rf"{re.escape(path)} [^:]*: (.*?)(?: \(not the default\))?$", t)
        if not m:
            raise BenchError(f"can't read setting {path}: {t!r}")
        return m.group(1)

    def save_state(self):
        line = self.radio.term(f"slot {self.slot_letter}")
        s = parse_slot_line(line)
        sv = dict(slot=s, active=None, end_tone=None, timeout=None)
        for n in range(3):
            if self.radio.slot(n)["active"]:
                sv["active"] = "ABC"[n]
        if s["channel"] is not None:
            # Its VFO's own state is what changes; the channel itself is never touched.
            vfo = parse_slot_line(self.radio.term(f"slot {self.slot_letter} vfo"))
            sv["vfo"] = vfo
            self.restore_notes.append(f"slot {self.slot_letter} was on channel {s['channel']}: its VFO is put back, "
                                      "but press V/M on the radio to return to the channel (the slot command can't)")
        else:
            sv["vfo"] = s
        sv["end_tone"] = self.setting(END_TONE)
        sv["timeout"] = self.setting(TX_TIMEOUT)
        self.saved = sv
        self.out(f"saved: {s['line']}; active {sv['active']}; end tone {sv['end_tone']}; TX timeout {sv['timeout']}")
        return sv

    def restore(self):
        sv = self.saved
        if not sv:
            return
        steps = [("txtest off", "txtest off")]
        v = sv["vfo"]
        words = [f"slot {self.slot_letter} vfo {v['freq']}", v.get("mode", "").lower(), v.get("power", "")]
        if v.get("txtone"):
            words += ["txtone", v["txtone"], "rxtone", v.get("rxtone", "off")]
        steps.append(("slot", " ".join(w for w in words if w)))
        steps.append(("end tone", f"set {END_TONE} {sv['end_tone']}"))
        steps.append(("TX timeout", f"set {TX_TIMEOUT} {sv['timeout']}"))
        if sv["active"]:
            steps.append(("active slot", f"slot {sv['active']} active"))
        for what, line in steps:
            try:
                self.radio.term(line)
            except Exception as e:  # keep putting the rest back
                self.restore_notes.append(f"restore {what} ({line}) failed: {e}")
        with contextlib.suppress(Exception):
            now = parse_slot_line(self.radio.term(f"slot {self.slot_letter}"))
            for k in ("freq", "mode", "power", "txtone", "rxtone", "offset"):
                if now.get(k) != v.get(k):
                    self.restore_notes.append(f"slot {self.slot_letter} {k}: was {v.get(k)}, now {now.get(k)}")
        self.out("restored: " + ("; ".join(self.restore_notes) if self.restore_notes else "everything as it was"))

    def setup_slot(self, mhz, mode):
        L = self.slot_letter
        self.radio.term(f"slot {L} vfo {mhz:.5f} {mode} low tone off")
        st = self.radio.slot(self.slot_n)
        if (st["offset"] or st["split"]) and self.a.clear_offset and not st["split"]:
            # The keypad's own rule (entry::tune_vfo): a shift is scaled to the new band's, zero where the band has
            # none; a tune there and back clears it. The original shift is noted as not put back.
            self.radio.term(f"slot {L} vfo 160.00000")
            self.radio.term(f"slot {L} vfo {mhz:.5f}")
            self.restore_notes.append(f"slot {L}'s VFO shift was cleared (--clear-offset): set it again on the radio")
        if not st["active"]:
            self.radio.term(f"slot {L} active")
        self.radio.term(f"set {END_TONE} Off")
        t = self.setting(TX_TIMEOUT)
        secs = int(re.match(r"(\d+)", t).group(1)) if re.match(r"\d+", t) else 10 ** 6  # Off: none
        if secs < self.cfg["timeout_min_s"]:
            self.radio.term(f"set {TX_TIMEOUT} {self.cfg['timeout_raise']}")

    # -- keying --
    def key_session(self, seconds, purpose, alive_s=None, stop_on_unkey=True):
        """Key (LINK-20), keepalives every 0.5 s (only for the first `alive_s` s if given), KEY off after `seconds`;
        always KEY off in `finally`. Times on the bench's clock."""
        ev = dict(purpose=purpose, t_on=None, accepted=None, refused=None, t_last_alive=None, unkeyed=None,
                  t_unkeyed=None, t_off=None, off_answered=None)
        c = self.clock
        try:
            self.keyed = True
            r = self.radio.key_on(self.slot_code)
            ev["t_on"] = c.time()
            if r is None:
                if self.radio.cable:
                    raise SafetyError("KEY on wasn't answered within 0.6 s: a `tx` build holds its answers while "
                                      "keyed (TX-43), a tx-dry build doesn't. Unkeyed at once; stopping.")
                ev["accepted"] = True  # not on the cable: answers may be late
            elif r[0] != 0:
                ev["accepted"], ev["refused"] = False, r[1]
                return ev
            else:
                ev["accepted"] = True
            stop = ev["t_on"] + seconds
            while c.time() < stop:
                if alive_s is None or c.time() < ev["t_on"] + alive_s:
                    self.radio.key_alive()
                    ev["t_last_alive"] = c.time()
                u = self.radio.poll_unkey()
                if u and ev["unkeyed"] is None:
                    ev["unkeyed"], ev["t_unkeyed"] = u, c.time()
                    self.out(f"  the radio unkeyed: {u}")
                    if stop_on_unkey:
                        break
                c.sleep(min(0.5, max(stop - c.time(), 0)))
        finally:
            ev["off_answered"] = self.radio.key_off()
            ev["t_off"] = c.time()
            self.keyed = False
        return ev

    def emergency_unkey(self):
        if self.keyed and self.radio:
            with contextlib.suppress(Exception):
                self.radio.key_off()
            self.keyed = False

    # -- the steps --
    def capture_keyed(self, mhz, key_s, path, purpose, alive_s=None, stop_on_unkey=True, lead=None, tail=None):
        lead = self.cfg["sdr_lead_s"] if lead is None else lead
        tail = self.cfg["sdr_tail_s"] if tail is None else tail
        total = lead + key_s + tail
        self.sdr.start(mhz, total, path)
        t0 = self.clock.time()
        try:
            self.clock.sleep(lead)
            ev = self.key_session(key_s, purpose, alive_s=alive_s, stop_on_unkey=stop_on_unkey)
            self.clock.sleep(max(t0 + total - self.clock.time(), 0))
            raw = self.sdr.finish()
        except BaseException:
            self.sdr.stop()
            raise
        ev["t_capture"] = t0
        return ev, raw

    def run_suite(self, run, sess_dir):
        name, mhz, mode = run["name"], run["mhz"], run["mode"]
        self.out(f"\n== {name}: {mhz:.3f} MHz {mode.upper()}, slot {self.slot_letter}")
        self.setup_slot(mhz, mode)
        armed = self.radio.term("txtest suite")
        if "armed" not in armed:
            raise BenchError(f"txtest suite not armed: {armed.strip()}")
        self.gate_key(self.cfg["test_freqs"], name)
        path = os.path.join(sess_dir, name + ".u8")
        ev, raw = self.capture_keyed(mhz, self.cfg["suite_key_s"], path, name)
        if ev["refused"]:
            raise BenchError(f"{name}: KEY refused: {ev['refused']}")
        self.radio.term("txtest off")
        return dict(run=run, capture=path, events=ev, raw_bytes=int(len(raw)))

    def run_refusals(self, sess_dir):
        out = []
        L = self.slot_letter
        allowed = [r["mhz"] for r in self.cfg["refusals"]]
        for r in self.cfg["refusals"]:
            f = r["mhz"]
            self.out(f"\n== refusal at {f:.3f} MHz (expect: {r['reason']})")
            res = dict(mhz=f, expect=r["reason"])
            try:
                self.radio.term(f"slot {L} vfo {f:.5f} fm low tone off")
            except TermError as e:
                res.update(ok=False, why=f"couldn't tune: {e}")
                out.append(res)
                continue
            self.gate_key(allowed, f"refusal {f:.3f}")
            path = os.path.join(sess_dir, f"refusal-{f:.3f}.u8")
            ev, raw = self.capture_keyed(f, 1.0, path, f"refusal {f:.3f}", lead=1.5, tail=1.5)
            runs = carrier_runs(raw)
            reason_ok = bool(ev["refused"]) and r["reason"].lower() in ev["refused"].lower()
            res.update(refused=ev["refused"], accepted=ev["accepted"], carrier=[list(x) for x in runs],
                       ok=reason_ok and not runs and not ev["accepted"])
            if ev["accepted"]:
                res["why"] = "the radio KEYED (unkeyed at once)"
            elif not reason_ok:
                res["why"] = f"refused, but the reason differs: {ev['refused']!r}"
            elif runs:
                res["why"] = f"a carrier on the SDR: {runs}"
            self.out(f"  {'pass' if res['ok'] else 'FAIL'}: {ev['refused'] or 'not refused'}; "
                     f"{'no carrier' if not runs else 'carrier ' + str(runs)}")
            if not self.a.keep_captures:
                with contextlib.suppress(OSError):
                    os.remove(path)
            out.append(res)
        return out

    def run_keepalive(self, sess_dir):
        ka = self.cfg["keepalive"]
        run = self.cfg["runs"][0]
        self.out(f"\n== keepalive: keepalives for {ka['alive_s']} s, then none (drop expected within {ka['max_drop_s']} s)")
        self.setup_slot(run["mhz"], "fm")
        self.gate_key(self.cfg["test_freqs"], "keepalive")
        path = os.path.join(sess_dir, "keepalive.u8")
        ev, raw = self.capture_keyed(run["mhz"], ka["alive_s"] + ka["wait_s"], path, "keepalive",
                                     alive_s=ka["alive_s"], stop_on_unkey=False)
        res = analyse_keepalive(raw, ev, ka)
        json.dump(jsonable(dict(events=ev, cfg=ka, kind="keepalive")), open(path[:-3] + ".json", "w"), indent=1)
        self.out(f"  {'pass' if res['ok'] else 'FAIL'}: {res['text']}")
        return res

    def run_timeout(self, sess_dir):
        tt = self.cfg["timeout_test"]
        run = self.cfg["runs"][0]
        self.out(f"\n== TX timeout: set {tt['set']}, keyed {tt['key_s']} s (carrier ~{tt['expect_s']} s expected)")
        self.setup_slot(run["mhz"], "fm")
        self.radio.term(f"set {TX_TIMEOUT} {tt['set']}")
        try:
            self.gate_key(self.cfg["test_freqs"], "timeout")
            path = os.path.join(sess_dir, "timeout.u8")
            ev, raw = self.capture_keyed(run["mhz"], tt["key_s"], path, "timeout", stop_on_unkey=False)
        finally:
            with contextlib.suppress(Exception):
                self.radio.term(f"set {TX_TIMEOUT} {self.cfg['timeout_raise']}")
        res = analyse_timeout(raw, ev, tt)
        json.dump(jsonable(dict(events=ev, cfg=tt, kind="timeout")), open(path[:-3] + ".json", "w"), indent=1)
        self.out(f"  {'pass' if res['ok'] else 'FAIL'}: {res['text']}")
        return res

    def on_air(self, sess_dir, runs, hello, info, flashed_dry=None):
        """Everything keyed, inside one save/restore; returns the captures and the extra checks' results."""
        caps, extras = [], {}
        self.check_build(hello, info, flashed_dry)
        try:
            self.save_state()
            for run in runs:
                caps.append(self.run_suite(run, sess_dir))
            if self.a.refusals:
                extras["refusals"] = self.run_refusals(sess_dir)
            if self.a.keepalive:
                extras["keepalive"] = self.run_keepalive(sess_dir)
            if self.a.timeout:
                extras["timeout"] = self.run_timeout(sess_dir)
        finally:
            self.emergency_unkey()
            self.restore()
        return caps, extras


def analyse_keepalive(raw, ev, ka):
    runs = carrier_runs(raw)
    if not ev.get("accepted"):
        return dict(ok=False, text=f"not keyed: {ev.get('refused')}", carrier=runs)
    if not runs:
        return dict(ok=False, text="no carrier on the SDR", carrier=runs)
    on = max(runs, key=lambda r: r[1] - r[0])
    carrier_s = on[1] - on[0]
    alive_span = (ev["t_last_alive"] or ev["t_on"]) - ev["t_on"]
    drop = carrier_s - alive_span
    ok = 0 < drop <= ka["max_drop_s"]
    return dict(ok=ok, carrier_s=carrier_s, alive_span_s=alive_span, drop_s=drop, unkeyed=ev.get("unkeyed"),
                text=f"carrier {carrier_s:.2f} s; last keepalive {alive_span:.2f} s after the key-up; dropped "
                     f"{drop:.2f} s after it (limit {ka['max_drop_s']} s)" + (f"; the radio said: {ev['unkeyed']}" if ev.get("unkeyed") else ""))


def analyse_timeout(raw, ev, tt):
    runs = carrier_runs(raw)
    if not ev.get("accepted"):
        return dict(ok=False, text=f"not keyed: {ev.get('refused')}", carrier=runs)
    if not runs:
        return dict(ok=False, text="no carrier on the SDR", carrier=runs)
    on = max(runs, key=lambda r: r[1] - r[0])
    carrier_s = on[1] - on[0]
    ok = abs(carrier_s - tt["expect_s"]) <= tt["tol_s"]
    return dict(ok=ok, carrier_s=carrier_s, unkeyed=ev.get("unkeyed"),
                text=f"carrier {carrier_s:.2f} s (expected {tt['expect_s']:.0f} ± {tt['tol_s']} s)"
                     + (f"; the radio said: {ev['unkeyed']}" if ev.get("unkeyed") else ""))


# ---- The manual's rules: what each measurement checks (bench/results.json, tools/record_results.py) ----
#
# A check is tagged with a rule only where it measures a number or outcome the rule states; "part" marks a rule
# the check covers only in part (recorded ◐ on a pass), "all" one it covers as the rule states it (✓). The rest of
# the suite's numbers (REG_40 steps, 4FSK, jitter, carrier offset) are regression numbers with no rule figure: they
# stay in the run comparison and are not tagged.

END_TONES = [  # TONE-10's list: (slug, [(Hz, ms), ...])
    ("beep", [(1000, 150)]),
    ("two-tone-up", [(800, 100), (1200, 100)]),
    ("two-tone-down", [(1200, 100), (800, 100)]),
    ("chirp", [(1000, 60), (1250, 60), (1500, 60)]),
    ("quindar", [(2475, 250)]),
]
SUITE_SUBTONES = [67.0, 100.0, 151.4, 250.3]  # TERM-34 section 6, then DCS 023 normal
RULE_TOL = dict(
    endtone_hz_frac=0.005,  # the generator's tone, ±0.5 %
    endtone_ms=15,          # the analysis' edges are 5 ms steps; a tone's swell blurs its start
    swell_tau_ms=(80, 20),  # TONE-10: time constant about 80 ms
    swell_t90_ms=(190, 40),  # TONE-10: 90 % at about 190 ms
    afsk_dev=(3000, 300),   # APRS-14: about 3 kHz (wide)
    afsk_twist_db=1.0,      # APRS-14: twist -0.2 dB measured; within ±1 dB
    afsk_half=0.1,          # TERM-34: the 50 % flags at half the 100 % deviation, ±10 %
    data_flat_db=1.0,       # TERM-33: `data` flat within 1 dB from 250 Hz to 2.5 kHz (against 1 kHz)
    stock_tone=0.05,        # TONE-19: stock's 0x50A, 1000 Hz 2.43 kHz and 2475 Hz 4.30 kHz, ±5 %
    ctcss_hz=0.2,
    ctcss_spread=0.05,      # one calibration byte: every CTCSS at the same deviation, ±5 %
    keepalive_cut_s=2.4,    # LINK-20: the carrier ends 2 s after the last keepalive, "not 2.4 s and more"
)


def _chk(what, ok, got=""):
    return dict(what=what, ok=bool(ok), got=got)


def _f(v, spec="{:.0f}"):
    return "-" if v is None else (spec.format(v) if isinstance(v, (int, float)) else str(v))


def rule_tests(session, runs):
    """The rule-tagged checks of one session: session.json's extras (refusals, keepalive, timeout, reboot) and each
    suite run's metrics. Each: dict(name, rules, scope {rule: part|all}, result, checks, summary, why)."""
    tests = []

    def add(name, rules, checks, summary, scope="part"):
        ok = all(c["ok"] for c in checks)
        bad = [c for c in checks if not c["ok"]]
        tests.append(dict(name=name, module="bench", rules=list(rules),
                          scope={r: (scope[r] if isinstance(scope, dict) else scope) for r in rules},
                          result="pass" if ok and checks else "fail", checks=checks, summary=summary,
                          why="; ".join(f"{c['what']}: got {c['got']}" for c in bad[:3])))

    ex = session.get("extras") or {}
    refusal_rules = [  # (expected reason word, test name, rules)
        ("band plan", "refusal_band_plan", ["TX-1", "SAFE-12"]),
        ("1.25 m", "refusal_125m", ["TX-10", "SAFE-12"]),
        ("emergency", "refusal_emergency", ["TX-35", "SAFE-12"]),
    ]
    for r in ex.get("refusals") or []:
        for word, name, rules in refusal_rules:
            if word in (r.get("expect") or ""):
                said = r.get("refused") or ""
                add(name, rules, [_chk(f"remote KEY on {r['mhz']:.3f} MHz refused", not r.get("accepted"),
                                       said or "keyed"),
                                  _chk(f"the reason names {word}", word in said, said),
                                  _chk("no carrier on the SDR", not r.get("carrier"), r.get("carrier"))],
                    f"{r['mhz']:.3f} MHz refused \"{said.replace('invalid: ', '')}\", no carrier")
    ka = ex.get("keepalive")
    if ka:
        drop = ka.get("drop_s")
        lim = RULE_TOL["keepalive_cut_s"]
        add("keepalive_cut", ["LINK-20", "TX-47"],
            [_chk("keyed and on the SDR", drop is not None, ka.get("text")),
             _chk(f"carrier ends under {lim} s after the last keepalive (a cut: no 400 ms hold)",
                  drop is not None and 0 < drop < lim, _f(drop, "{:.2f} s"))],
            f"carrier ended {_f(drop, '{:.2f}')} s after the last keepalive")
    tt = ex.get("timeout")
    if tt:
        cs = tt.get("carrier_s")
        add("tx_timeout", ["TX-30", "SAFE-17"],
            [_chk("carrier about 30 s at the 30 s setting (±1.5 s)", cs is not None and abs(cs - 30.0) <= 1.5,
                  _f(cs, "{:.2f} s")),
             _chk("the radio ended it as a timeout", "timeout" in (tt.get("unkeyed") or ""), tt.get("unkeyed"))],
            f"30 s timeout: carrier {_f(cs, '{:.1f}')} s, \"{(tt.get('unkeyed') or '').replace('invalid: ', '')}\"")
    rb = session.get("reboot")
    if rb:
        add("reboot", ["LINK-24"], [_chk("HELLO again, home, `dump` unchanged", rb.get("ok"), rb.get("text"))],
            f"REBOOT: {rb.get('text')}")

    for j in runs:
        m, name, wide = j.get("metrics") or {}, j["run"], j.get("mode") == "fm"
        if not m.get("markers_found"):
            add(f"suite@{name}", ["TERM-34"], [_chk("the suite's markers on the SDR", False, m.get("markers_found"))],
                "no markers", "all")
            continue
        d100 = [m.get("afsk.dac100.mark_dev"), m.get("afsk.dac100.space_dev")]
        d50 = [m.get("afsk.dac50.mark_dev"), m.get("afsk.dac50.space_dev")]
        sections = {1: "sweep.data.low_hz", 2: "sweep.voice.low_hz", 3: "afsk.dac100.mark_dev",
                    4: "reg40.0x43C.dev", 5: "tone1k.steady_dev", 6: "ctcss.100.0.hz", 7: "fsk4.symbols"}
        half = (sum(d50) / sum(d100)) if None not in d100 + d50 and sum(d100) else None
        add(f"suite@{name}", ["TERM-34"],
            [_chk("all 7 section markers found", m.get("markers_found") == 7, m.get("markers_found")),
             _chk("the 1 kHz hold after the last step", m.get("hold") == "yes", m.get("hold")),
             _chk("every section measured", all(m.get(k) is not None for k in sections.values()),
                  ", ".join(str(n) for n, k in sections.items() if m.get(k) is None) or "all"),
             _chk("section 3's 50 % flags at half the 100 % deviation (±10 %)",
                  half is not None and abs(half - 0.5) <= 0.5 * RULE_TOL["afsk_half"], _f(half, "{:.3f}"))],
            f"{name}: 7 markers, hold, every section measured; AFSK 50 % at {_f(half, '{:.2f}')} of 100 %", "all")
        # TERM-33's `data` set-up: flat within 1 dB from 250 Hz to 2.5 kHz (the sweep against its 1 kHz point)
        c = (j.get("curves") or {}).get("sweep", {}).get("data")
        if c:
            pts = [(f, db) for f, db in zip(c["freqs"], c["db"]) if 250 <= f <= 2500 and db is not None]
            ref = next((db for f, db in zip(c["freqs"], c["db"]) if abs(f - 1000) < 1), None)
            if pts and ref is not None:
                worst = max(pts, key=lambda p: abs(p[1] - ref))
                add(f"data_flat@{name}", ["TERM-33"],
                    [_chk("`data` sweep within ±1 dB of 1 kHz from 250 Hz to 2.5 kHz",
                          abs(worst[1] - ref) <= RULE_TOL["data_flat_db"], f"{worst[1] - ref:+.2f} dB at {worst[0]:.0f} Hz")],
                    f"{name}: `data` {min(p[1] for p in pts) - ref:+.1f} to {max(p[1] for p in pts) - ref:+.1f} dB, 250 Hz-2.5 kHz")
        # TONE-10: the end tones' frequencies and lengths, and the first tone's swell
        checks, found = [], []
        for slug_, steps in END_TONES:
            for hz, ms in steps:
                k = f"endtone.{slug_}.{hz}"
                got_hz, got_ms = m.get(k + ".hz"), m.get(k + ".ms")
                checks.append(_chk(f"{slug_} {hz} Hz", m.get(k + ".found") == "yes" and got_hz is not None
                                   and abs(got_hz - hz) <= hz * RULE_TOL["endtone_hz_frac"], _f(got_hz, "{:.1f} Hz")))
                checks.append(_chk(f"{slug_} {hz} Hz lasts {ms} ms (±{RULE_TOL['endtone_ms']})", got_ms is not None
                                   and abs(got_ms - ms) <= RULE_TOL["endtone_ms"], _f(got_ms, "{:.0f} ms")))
                found.append(f"{hz}/{_f(got_ms)}")
        tau, t90 = m.get("tone1k.tau_ms"), m.get("tone1k.t90_ms")
        checks.append(_chk("swell time constant about 80 ms", tau is not None
                           and abs(tau - RULE_TOL["swell_tau_ms"][0]) <= RULE_TOL["swell_tau_ms"][1], _f(tau, "{:.0f} ms")))
        checks.append(_chk("90 % at about 190 ms", t90 is not None
                           and abs(t90 - RULE_TOL["swell_t90_ms"][0]) <= RULE_TOL["swell_t90_ms"][1], _f(t90, "{:.0f} ms")))
        add(f"end_tones@{name}", ["TONE-10"], checks,
            f"{name}: end tones Hz/ms {' '.join(found)}; swell τ {_f(tau)} ms, 90 % {_f(t90)} ms")
        if wide:
            # APRS-14: the beacon's AFSK level (section 3 runs at it), wide FM
            mean = sum(d100) / 2 if None not in d100 else None
            tw = m.get("afsk.dac100.twist_db")
            add(f"afsk_level@{name}", ["APRS-14"],
                [_chk("AFSK deviation about 3.0 kHz (±0.3)", mean is not None
                      and abs(mean - RULE_TOL["afsk_dev"][0]) <= RULE_TOL["afsk_dev"][1], _f(mean, "{:.0f} Hz")),
                 _chk("twist within ±1 dB", tw is not None and abs(tw) <= RULE_TOL["afsk_twist_db"], _f(tw, "{:+.2f} dB"))],
                f"{name}: AFSK {_f(d100[0])}/{_f(d100[1])} Hz, twist {_f(tw, '{:+.2f}')} dB")
            # TONE-19's stock figures (the suite's section 5 plays at stock's REG_40 0x50A)
            a, q = m.get("endtone.beep.1000.dev"), m.get("endtone.quindar.2475.dev")
            add(f"stock_tone_level@{name}", ["TONE-19"],
                [_chk("1000 Hz at 0x50A about 2.43 kHz (±5 %)", a is not None and abs(a / 2430 - 1) <= RULE_TOL["stock_tone"], _f(a, "{:.0f} Hz")),
                 _chk("2475 Hz at 0x50A about 4.30 kHz (±5 %)", q is not None and abs(q / 4300 - 1) <= RULE_TOL["stock_tone"], _f(q, "{:.0f} Hz"))],
                f"{name}: stock's 0x50A: 1000 Hz {_f(a)} Hz, 2475 Hz {_f(q)} Hz (Ferrite's 0x43C levels not in the suite)")
        # TONE-1 (generator sub-tones at one calibration deviation) and TONE-20 (D023N goes out normal)
        hz = {t: m.get(f"ctcss.{t:.1f}.hz") for t in SUITE_SUBTONES}
        dev = [m.get(f"ctcss.{t:.1f}.dev") for t in SUITE_SUBTONES]
        spread = (max(dev) / min(dev) - 1) if None not in dev and min(dev) > 0 else None
        add(f"subtones@{name}", ["TONE-1"],
            [_chk(f"CTCSS {t} Hz (±{RULE_TOL['ctcss_hz']})", v is not None and abs(v - t) <= RULE_TOL["ctcss_hz"],
                  _f(v, "{:.2f} Hz")) for t, v in hz.items()]
            + [_chk("every CTCSS at one deviation (±5 %)", spread is not None and spread <= RULE_TOL["ctcss_spread"],
                    _f(spread, "{:.1%}")),
               _chk("DCS on the air", m.get("dcs.code") not in (None, "none"), m.get("dcs.code"))],
            f"{name}: CTCSS 67.0-250.3 Hz at {_f(dev[1])} Hz, DCS {m.get('dcs.code')} at {_f(m.get('dcs.dev'))} Hz "
            "(the calibration byte isn't read back)")
        add(f"dcs_polarity@{name}", ["TONE-20"],
            [_chk("D023N decodes as 023 normal", m.get("dcs.code") == "023 N", m.get("dcs.code"))],
            f"{name}: D023N decoded {m.get('dcs.code')} (transmit side only)")
    return tests


def rule_summary(sess_dir):
    """bench/results.json for a session, in uitest's shape (tool, started, port, sim, build, version, commit, label,
    counts, rules {rule: result, test, why, scope, note}, tests), from session.json and the <run>.json files."""
    session = json.load(open(os.path.join(sess_dir, "session.json")))
    runs = []
    for r in session.get("runs") or []:
        p = os.path.join(sess_dir, r + ".json")
        if os.path.exists(p):
            runs.append(json.load(open(p)))
    tests = rule_tests(session, runs)
    rules = {}
    for t in tests:
        for rid in t["rules"]:
            x = rules.setdefault(rid, dict(result="pass", test=[], why="", scope="all", notes=[]))
            x["test"].append(t["name"])
            if t["result"] != "pass":
                x["result"] = "fail"
                x["why"] = x["why"] or (f"{t['name'].split('@')[1]}: " if "@" in t["name"] else "") + t["why"]
            if t["scope"][rid] == "part":
                x["scope"] = "part"
            x["notes"].append(t["summary"])
    for x in rules.values():
        names = []
        for n in x["test"]:
            base = n.split("@")[0]
            if base not in names:
                names.append(base)
        runs_ = [n.split("@")[1] for n in x["test"] if "@" in n]
        x["test"] = ", ".join(names)
        x["runs"] = list(dict.fromkeys(runs_))
        notes = list(dict.fromkeys(x.pop("notes")))
        # one run's numbers stand for the rest (each run passed or the why says which failed)
        x["note"] = notes[0] + (f"; the same checks on {', '.join(list(dict.fromkeys(runs_))[1:])}"
                                if len(runs_) > 1 else "") if runs_ else "; ".join(notes)
    b = session.get("build") or {}
    stamp = os.path.basename(os.path.normpath(sess_dir))[:15]
    port = session.get("port")
    sim = bool(session.get("sim")) or str(port or "").startswith("tcp:") or port == "fake"
    commit = session.get("commit") or b.get("host")
    return dict(tool="bench.py", started=datetime.datetime.strptime(stamp, "%Y%m%d-%H%M%S").isoformat()
                if re.match(r"\d{8}-\d{6}$", stamp) else None,
                port=port, sim=sim, source=session.get("source"), build="tx-dry" if session.get("tx_dry") else None,
                version=b.get("version"), commit=commit, label=b.get("label"),
                counts={k: sum(1 for t in tests if t["result"] == k) for k in ("pass", "fail")},
                rules=rules, tests=tests)


def write_rule_results(sess_dir):
    s = rule_summary(sess_dir)
    d = os.path.join(sess_dir, "bench")
    os.makedirs(d, exist_ok=True)
    p = os.path.join(d, "results.json")
    json.dump(jsonable(s), open(p, "w"), indent=1, ensure_ascii=False)
    return p, s


# ---- Analysis, storage, the page ----


def analyse_run(cfg, a, run, capture, sess_dir, session, build, raw=None, events=None):
    rd = os.path.join(sess_dir, run["name"])
    os.makedirs(rd, exist_ok=True)
    say(f"analysing {run['name']} ({capture}) ...")
    if raw is None:
        raw = np.fromfile(capture, np.uint8)
    with quiet(None if a.verbose else os.path.join(rd, "analysis.log")):
        R = sdrsuite.analyse(raw, rd, run["name"], run["mhz"])
    m, curves = metrics_of(R)
    j = dict(run=run["name"], config_key=config_key(run), mhz=run["mhz"], mode=run["mode"], slot=cfg["slot"],
             session=session, build=build, date=datetime.datetime.now().isoformat(timespec="seconds"),
             ok=bool(R.get("ok")), capture=os.path.abspath(capture), metrics=m, curves=curves,
             files=dict(suite_html=os.path.relpath(R["html"], sess_dir) if R.get("html") else None,
                        suite_txt=os.path.relpath(R["txt"], sess_dir) if R.get("txt") else None),
             events=jsonable(events or {}))
    prev = previous_run(cfg, run, session)
    if prev:
        rows = compare(cfg, m, prev["metrics"])
        j["previous"] = dict(session=prev["session"], build=prev.get("build"), curves=prev.get("curves"))
        j["comparison"] = [list(r) for r in rows]
        nflag = sum(r[5] in ("CHANGED", "missing") for r in rows)
        say(f"  {run['name']}: {len(m)} numbers; against {prev['session']}: {nflag} beyond tolerance")
    else:
        say(f"  {run['name']}: {len(m)} numbers; no earlier run of {config_key(run)} to compare with")
    json.dump(jsonable(j), open(os.path.join(sess_dir, run["name"] + ".json"), "w"), indent=1)
    return j


CSS = """
:root{--bg:#fbfbfa;--card:#ffffff;--ink:#141413;--muted:#5e5d59;--line:#e3e2dd;--accent:#2a78d6;--accent2:#d0632a;
--ok:#1a7f4b;--bad:#c4321f;--warn:#a86b00;--okbg:#e6f4ec;--badbg:#fbe9e6;--warnbg:#fbf1dc;color-scheme:light}
@media (prefers-color-scheme:dark){:root:not([data-theme=light]){--bg:#1a1a19;--card:#232322;--ink:#f2f1ec;
--muted:#b5b4ab;--line:#3a3a37;--accent:#5b9cf0;--accent2:#f0894f;--ok:#5ccb8c;--bad:#ff7a66;--warn:#e8b14a;
--okbg:#1d3328;--badbg:#3d211c;--warnbg:#3a2f17;color-scheme:dark}}
:root[data-theme=dark]{--bg:#1a1a19;--card:#232322;--ink:#f2f1ec;--muted:#b5b4ab;--line:#3a3a37;--accent:#5b9cf0;
--accent2:#f0894f;--ok:#5ccb8c;--bad:#ff7a66;--warn:#e8b14a;--okbg:#1d3328;--badbg:#3d211c;--warnbg:#3a2f17;color-scheme:dark}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--ink);font:15px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}
main{max-width:1040px;margin:0 auto;padding:24px 16px 48px}h1{font-size:24px;margin:0 0 2px}h2{font-size:19px;margin:32px 0 8px}
h3{font-size:16px;margin:18px 0 6px}.sub{color:var(--muted);margin:0 0 16px}a{color:var(--accent)}
.card{background:var(--card);border:1px solid var(--line);border-radius:10px;padding:14px 16px;margin:12px 0}
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(190px,1fr));gap:8px;margin:10px 0}
.tile{border:1px solid var(--line);border-radius:8px;padding:8px 10px}.k{color:var(--muted);font-size:12.5px}.v{font-weight:600;font-variant-numeric:tabular-nums}
.wrap{overflow-x:auto}table{border-collapse:collapse;width:100%;font-size:13.5px;font-variant-numeric:tabular-nums}
td,th{border-bottom:1px solid var(--line);padding:5px 8px;text-align:left;white-space:nowrap}th{color:var(--muted);font-weight:600}
td.n{text-align:right}.pill{display:inline-block;border-radius:99px;padding:0 8px;font-size:12.5px;font-weight:600}
.ok{color:var(--ok);background:var(--okbg)}.bad{color:var(--bad);background:var(--badbg)}.warn{color:var(--warn);background:var(--warnbg)}
.muted{color:var(--muted)}details summary{cursor:pointer;color:var(--accent);margin:6px 0}
svg text{fill:var(--muted);font-size:11px;font-family:inherit}svg .grid{stroke:var(--line)}svg .axis{stroke:var(--muted)}
.legend span{display:inline-flex;align-items:center;gap:6px;margin-right:14px;font-size:13px;color:var(--muted)}
.sw{width:18px;height:0;border-top:2px solid}.sw.d{border-top-style:dashed}
ul.notes{margin:6px 0;padding-left:20px}
"""


def fmt(v):
    if v is None:
        return "–"
    if isinstance(v, str):
        return v
    if isinstance(v, int) or (isinstance(v, float) and v.is_integer() and abs(v) < 1e6):
        return f"{v:.0f}"
    a = abs(v)
    return f"{v:.3f}" if a < 10 else f"{v:.2f}" if a < 100 else f"{v:.1f}" if a < 1000 else f"{v:.0f}"


def sweep_svg(cur, prev):
    """The two sweeps (data, voice) now (solid) and before (dashed): log frequency, plain Hz tick labels."""
    if not cur:
        return ""
    W, H, l, r, t, b = 640, 260, 46, 12, 10, 34
    f0, f1, y0, y1 = 45, 9000, -30, 6

    def X(f):
        return l + (math.log10(f) - math.log10(f0)) / (math.log10(f1) - math.log10(f0)) * (W - l - r)

    def Y(d):
        return t + (y1 - max(min(d, y1), y0)) / (y1 - y0) * (H - t - b)

    g = []
    for f in (50, 100, 200, 300, 500, 1000, 2000, 3000, 5000, 8000):
        g.append(f'<line class="grid" x1="{X(f):.1f}" x2="{X(f):.1f}" y1="{t}" y2="{H - b}"/>'
                 f'<text x="{X(f):.1f}" y="{H - b + 15}" text-anchor="middle">{f}</text>')
    for d in (-30, -24, -18, -12, -6, -3, 0, 6):
        dash = ' stroke-dasharray="3 3"' if d == -3 else ""
        g.append(f'<line class="grid" x1="{l}" x2="{W - r}" y1="{Y(d):.1f}" y2="{Y(d):.1f}"{dash}/>'
                 f'<text x="{l - 6}" y="{Y(d) + 4:.1f}" text-anchor="end">{d:+d}</text>'.replace(">+0<", ">0<"))
    g.append(f'<text x="{(l + W - r) / 2}" y="{H - 2}" text-anchor="middle">Hz</text>')
    g.append(f'<text x="12" y="{(t + H - b) / 2}" text-anchor="middle" transform="rotate(-90 12 {(t + H - b) / 2})">dB vs 1 kHz</text>')
    lines = []
    for src, dash in ((prev or {}, "5 4"), (cur, "")):
        for name, col in (("data", "var(--accent)"), ("voice", "var(--accent2)")):
            c = src.get(name)
            if not c:
                continue
            pts = " ".join(f"{X(f):.1f},{Y(d):.1f}" for f, d in zip(c["freqs"], c["db"]) if f0 <= f <= f1)
            st = f"stroke:{col};stroke-width:{1.4 if dash else 2}" + (f";stroke-dasharray:{dash};opacity:.75" if dash else "")
            lines.append(f'<polyline fill="none" style="{st}" points="{pts}"/>')
    leg = ('<div class=legend><span><i class=sw style="border-color:var(--accent)"></i>data now</span>'
           '<span><i class=sw style="border-color:var(--accent2)"></i>voice now</span>'
           + ('<span><i class="sw d" style="border-color:var(--muted)"></i>previous run</span>' if prev else "") + "</div>")
    return (f'<div class="wrap"><svg viewBox="0 0 {W} {H}" width="100%" style="max-width:{W}px" role="img" '
            f'aria-label="TX audio response sweep">{"".join(g)}{"".join(lines)}</svg></div>{leg}')


def pill(status):
    cls = {"ok": "ok", "pass": "ok", "info": "ok", "CHANGED": "bad", "FAIL": "bad", "missing": "bad", "new": "warn"}.get(status, "warn")
    return f"<span class='pill {cls}'>{html.escape(status)}</span>"


def summary_page(session, sess_dir, runs, extras, meta):
    e = html.escape
    parts = []
    nflag = sum(sum(r[5] in ("CHANGED", "missing") for r in j.get("comparison", [])) for j in runs)
    notes = meta.get("warnings", []) + meta.get("restore_notes", [])
    head = (f"<h1>Ferrite {'TX ' if runs or extras else ''}bench</h1><p class=sub>{e(session)} · build "
            f"{e(meta['build']['label'])} · {e(meta['source'])}"
            + (f" · commit {e(meta['commit'])}" if meta.get("commit") else "") + "</p>")
    if meta.get("decision"):
        d = meta["decision"]
        head += (f"<div class=card><b>Why this ran</b><p>{e(d['what'])}</p><p class=muted>{e(d['why'])}</p>"
                 + (f"<details><summary>{len(d['files'])} files changed</summary><ul class=notes>"
                    + "".join(f"<li>{e(f)} <span class=muted>({e(c)})</span></li>" for f, c in d["files"])
                    + "</ul></details>" if d.get("files") else "") + "</div>")
    if not runs and not extras:
        parts.append(head)
        head = None
    over = [("Runs", str(len(runs))), ("Beyond tolerance", str(nflag)),
            ("Extra checks", ", ".join(f"{k} {'pass' if all_ok(v) else 'FAIL'}" for k, v in extras.items()) or "none"),
            ("TX-dry known by", meta.get("tx_dry") or "–")]
    if head is not None:
        parts.append(head + "<div class=tiles>" + "".join(
            f"<div class=tile><div class=k>{e(k)}</div><div class=v>{e(v)}</div></div>" for k, v in over) + "</div>")
    if notes:
        parts.append("<div class=card><b>Notes</b><ul class=notes>" + "".join(f"<li>{e(n)}</li>" for n in notes) + "</ul></div>")
    for j in runs:
        m = j["metrics"]
        prev = j.get("previous")
        rows = j.get("comparison", [])
        flagged = [r for r in rows if r[5] not in ("ok", "info")]
        tiles = "".join(f"<div class=tile><div class=k>{e(k)}</div><div class=v>{e(v)}</div></div>" for k, v in headline(m))
        link = f"<a href='{e(j['files']['suite_html'])}'>sdrsuite page</a>" if j["files"].get("suite_html") else ""
        cmp_txt = (f"against {e(prev['session'])}: " + (f"{pill('CHANGED')} {len([r for r in rows if r[5] in ('CHANGED', 'missing')])} beyond tolerance"
                                                      if any(r[5] in ("CHANGED", "missing") for r in rows) else pill("ok") + " all within tolerance")
                   if prev else "<span class=muted>no earlier run of this configuration</span>")

        def table(rs):
            return ("<div class=wrap><table><tr><th>number</th><th>now</th><th>before</th><th>change</th><th>tolerance</th><th></th></tr>"
                    + "".join(f"<tr><td>{e(k)}</td><td class=n>{e(fmt(a))}</td><td class=n>{e(fmt(b))}</td>"
                              f"<td class=n>{'' if d is None else e(('+' if d > 0 else '') + fmt(d))}</td><td>{e(tol)}</td><td>{pill(s)}</td></tr>"
                              for k, a, b, d, tol, s in rs) + "</table></div>")
        body = f"<p>{cmp_txt} · {link}</p><div class=tiles>{tiles}</div>"
        body += sweep_svg(j.get("curves", {}).get("sweep"), (prev or {}).get("curves", {}) and prev["curves"].get("sweep"))
        if rows:
            if flagged:
                body += "<h3>Changed, missing or new</h3>" + table(flagged)
            body += f"<details><summary>All {len(rows)} numbers</summary>{table(rows)}</details>"
        else:
            body += ("<details><summary>All numbers</summary><div class=wrap><table>" + "".join(
                f"<tr><td>{e(k)}</td><td class=n>{e(fmt(v))}</td></tr>" for k, v in sorted(m.items())) + "</table></div></details>")
        parts.append(f"<h2>{e(j['run'])} <span class=muted>· {j['mhz']:.3f} MHz {e(j['mode'].upper())}</span> "
                     f"{pill('pass' if j['ok'] else 'FAIL')}</h2><div class=card>{body}</div>")
    if extras:
        parts.append("<h2>Extra checks</h2>")
        if "refusals" in extras:
            parts.append("<div class=card><h3>Remote PTT refused out of band (LINK-20)</h3><div class=wrap><table>"
                         "<tr><th>MHz</th><th>expected</th><th>the radio said</th><th>carrier</th><th></th></tr>" + "".join(
                             f"<tr><td>{r['mhz']:.3f}</td><td>{e(r['expect'])}</td><td>{e(r.get('refused') or r.get('why') or '–')}</td>"
                             f"<td>{'none' if not r.get('carrier') else e(str(r['carrier']))}</td><td>{pill('pass' if r['ok'] else 'FAIL')}</td></tr>"
                             for r in extras["refusals"]) + "</table></div></div>")
        for k, title in (("keepalive", "Keepalive lost: the carrier drops (LINK-20)"), ("timeout", "TX timeout")):
            if k in extras:
                r = extras[k]
                parts.append(f"<div class=card><h3>{e(title)} {pill('pass' if r['ok'] else 'FAIL')}</h3><p>{e(r['text'])}</p></div>")
    if meta.get("ui"):
        import uitest
        parts.append(uitest.ui_section(meta["ui"]))
    if meta.get("reboot"):
        r = meta["reboot"]
        parts.append(f"<h2>Reboot check {pill('pass' if r['ok'] else 'FAIL')}</h2><div class=card><p>{e(r['text'])}</p></div>")
    gl = meta.get("gates", [])
    if gl:
        seen, rows = set(), []
        for g in gl:
            key = (g["gate"].split(": ")[-1], g["ok"])
            if key in seen:
                continue
            seen.add(key)
            rows.append(f"<tr><td>{e(g['gate'])}</td><td>{pill('pass' if g['ok'] else 'FAIL')}</td><td>{e(g['detail'])}</td></tr>")
        parts.append("<h2>Safety gates</h2><div class=card><div class=wrap><table><tr><th>gate</th><th></th><th>detail</th></tr>"
                     + "".join(rows) + "</table></div></div>")
    b = meta["build"]
    parts.append(f"<p class=muted>Radio {e(b.get('version') or '–')} · host checkout {e(b.get('host') or '–')}"
                 f"{' · image ' + e(b['image']) if b.get('image') else ''} · port {e(meta.get('port') or '–')} · "
                 f"slot {e(meta.get('slot', 'A'))} · results {e(sess_dir)}</p>")
    return (f"<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>"
            f"<title>Ferrite bench</title><style>{CSS}</style></head><body><main>{''.join(parts)}</main></body></html>")


def all_ok(v):
    if isinstance(v, list):
        return all(x.get("ok") for x in v)
    return bool(v.get("ok"))


def write_session(cfg, sess_dir, session, runs, extras, meta):
    meta = dict(meta, session=session, runs=[j["run"] for j in runs], extras=extras,
                flagged={j["run"]: [r[0] for r in j.get("comparison", []) if r[5] in ("CHANGED", "missing")] for j in runs})
    json.dump(jsonable(meta), open(os.path.join(sess_dir, "session.json"), "w"), indent=1)
    write_rule_results(sess_dir)
    page = os.path.join(sess_dir, "summary.html")
    open(page, "w").write(summary_page(session, sess_dir, runs, jsonable(extras), meta))
    return page


def record_session(sess_dir, docs):
    """--record: the session's bench and UI results into the test plan (tools/record_results.py)."""
    import record_results
    for line in record_results.record_paths([sess_dir], docs):
        say(line)


def new_session(cfg, label):
    os.makedirs(cfg["results"], exist_ok=True)
    while True:  # one session a second: the names sort by time
        stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
        if not any(x.startswith(stamp) for x in os.listdir(cfg["results"])):
            break
        time.sleep(0.3)
    session = f"{stamp}_{slug(label)[:40]}"
    d = os.path.join(cfg["results"], session)
    os.makedirs(d)
    return session, d


# ---- Flash ----


def do_flash(cfg, a, image):
    dry, sha = image_is_tx_dry(image)
    if not dry:
        raise SafetyError(f"{image} is not a tx-dry image (no TX DRY boot line in it): build it with tools/mkbtf.sh tx-dry")
    if not os.path.exists(cfg["backup"]):
        raise BenchError(f"no flash backup at {cfg['backup']}")
    say(f"flashing {image} (tx-dry, sha256 {sha}) with flash.py --backup {cfg['backup']}")
    r = subprocess.run([sys.executable, os.path.join(HERE, "flash.py"), cfg["port"], image, "--backup", cfg["backup"]])
    if r.returncode != 0:
        raise BenchError(f"flash.py failed ({r.returncode})")
    say("waiting for Ferrite to boot ...")
    end = time.time() + 90
    while time.time() < end:
        time.sleep(3)
        try:
            rad = Radio(cfg["port"])
            return rad, os.path.basename(image), sha
        except Exception:
            continue
    raise BenchError("the radio didn't answer HELLO within 90 s of the flash")


# ---- Plan ----


def plan(cfg, a, runs):
    p = []
    L = cfg["slot"].upper()
    p.append(f"port {cfg['port']}; slot {L}; approved test frequencies {', '.join(f'{f:.3f}' for f in cfg['test_freqs'])} MHz")
    if a.flash:
        p.append(f"flash: check {a.flash} has the TX DRY boot line; flash.py {cfg['port']} {a.flash} --backup {cfg['backup']}; "
                 "wait for HELLO (up to 90 s)")
    p.append("connect: HELLO (the LINK-35 build byte: tx-dry with the bench bit, else refused), INFO (version), "
             "`txtest` (a transmit build?)")
    p.append("gate: tx-dry known: version says dry / flashed a TX DRY image here / --i-know-its-tx-dry"
             + (" [given: a warning will be printed]" if a.i_know_its_tx_dry else " [not given]"))
    p.append(f"save: `slot {L}` (and `slot {L} vfo` if on a channel), the active slot (SLOT), `get {END_TONE}`, `get {TX_TIMEOUT}`")
    total_key = 0
    for r in runs:
        p.append(f"run {r['name']}: `slot {L} vfo {r['mhz']:.5f} {r['mode']} low tone off`, `slot {L} active` if not, "
                 f"`set {END_TONE} Off`, TX timeout to {cfg['timeout_raise']} if under {cfg['timeout_min_s']} s, `txtest suite`; "
                 f"gates (SLOT: frequency approved, VFO, offset 0, no split, power low, not scanning, active); rtl_sdr at "
                 f"{r['mhz']:.3f} MHz for {cfg['sdr_lead_s'] + cfg['suite_key_s'] + cfg['sdr_tail_s']:.1f} s; key {cfg['suite_key_s']} s "
                 f"(keepalive 0.5 s; KEY on must be answered within {KEY_ANSWER_S} s on the cable); KEY off; `txtest off`")
        total_key += cfg["suite_key_s"]
    if a.refusals:
        for r in cfg["refusals"]:
            p.append(f"refusal {r['mhz']:.3f}: `slot {L} vfo {r['mhz']:.5f} fm low tone off`; rtl_sdr 4 s; KEY on, expect refused "
                     f"({r['reason']!r}); KEY off at once whatever the answer; no carrier expected")
    if a.keepalive:
        k = cfg["keepalive"]
        p.append(f"keepalive: at {cfg['runs'][0]['mhz']:.3f} FM; key, keepalives {k['alive_s']} s then none, KEY off after "
                 f"{k['alive_s'] + k['wait_s']} s; carrier must end within {k['max_drop_s']} s of the last keepalive")
        total_key += k["alive_s"] + k["wait_s"]
    if a.timeout:
        t = cfg["timeout_test"]
        p.append(f"timeout: `set {TX_TIMEOUT} {t['set']}`; key {t['key_s']} s with keepalives; carrier {t['expect_s']} ± {t['tol_s']} s; "
                 f"timeout back to {cfg['timeout_raise']} (then to the saved value)")
        total_key += t["key_s"]
    p.append("restore (finally, whatever happened): KEY off, `txtest off`, the slot's VFO (freq, mode, power, tones), "
             "end tone, TX timeout, the active slot; check the slot reads back as saved")
    p.append(f"analyse each capture (sdrsuite), store under {cfg['results']}/<date-time>_<build>/, compare with the newest "
             "earlier run of the same configuration, write summary.html")
    say("dry plan (nothing is touched):")
    for i, s in enumerate(p, 1):
        say(f" {i:2d}. {s}")
    say(f"keyed in all: about {total_key:.0f} s; rtl_sdr {'found' if shutil.which('rtl_sdr') else 'NOT found'}")


# ---- Fakes for the selftest ----


class FakeRadio:
    """The radio as the bench sees it, on a virtual clock: slots, settings, txtest, KEY with the band plan's
    refusals, keepalive loss (2 s) and the TX timeout; every carrier logged for FakeSdr."""

    BANDS = [(144.0, 148.0), (420.0, 450.0)]

    def __init__(self, clock, version="v0.1.0", rx_only=False, hold_answers=False, offset=0, cable=True,
                 ignore_freq=False, stuck_power=None, fail_alive_after=None, build="not given"):
        self.build = build
        self.clock, self.version, self.rx_only, self.hold, self.cable = clock, version, rx_only, hold_answers, cable
        self.ignore_freq, self.stuck_power, self.fail_alive_after = ignore_freq, stuck_power, fail_alive_after
        self.slots = [dict(ch=None, freq=146.52, mode="FM", power="low", tx="100.0", rx="off", offset=offset),
                      dict(ch=11, freq=443.475, mode="NFM", power="low", tx="100.0", rx="off", offset=500000),
                      dict(ch=None, freq=118.95, mode="AM", power="low", tx="off", rx="off", offset=0)]
        self.vfo_b = dict(ch=None, freq=446.0, mode="FM", power="low", tx="off", rx="off", offset=0)
        self.active = 0
        self.set = {END_TONE: "Beep", TX_TIMEOUT: "60 s"}
        self.txtest = None
        self.key = None  # dict(on, last, slot, kind)
        self.carriers = []  # (t_on, t_off, mhz, kind)
        self.calls = []
        self.unkey_msgs = []
        self.alive_n = 0

    def close(self):
        pass

    def hello(self):
        return dict(protocol=2, version=self.version, build=self.build, bench=self.build == "tx-dry")

    def info(self):
        return dict(version=self.version, channels=21, zones=6, presets=4, region=2, callsign="TEST")

    def _line(self, n):
        s = self.slots[n]
        where = f'ch {s["ch"] + 1} "Rpt"' if s["ch"] is not None else "vfo"
        tone = f"tone={s['tx']}" if s["tx"] == s["rx"] else f"txtone={s['tx']} rxtone={s['rx']}"
        off = f" offset={s['offset'] / 1e5:+.5f}" if s["offset"] else ""
        return f"{'ABC'[n]}{'*' if self.active == n else ' '} {where} {s['freq']:.5f} mode={s['mode']} power={s['power']} {tone}{off}\n"

    def term(self, line):
        self.calls.append(line)
        if self.key:
            raise TermError(f"{line}: transmitting: nothing changes while keyed (TX-22)")
        w = line.split()
        if w[0] == "txtest":
            if self.rx_only:
                raise TermError("txtest: the receive-only build has no transmit: txtest needs a tx or tx-dry build")
            if len(w) > 1 and w[1] == "suite":
                if int(re.match(r"\d+", self.set[TX_TIMEOUT]).group()) < 90:
                    raise TermError("txtest suite: the suite runs 60 s: set the TX timeout (0 8 1 1) to 90 s or more first")
                self.txtest = "suite"
            elif len(w) > 1 and w[1] == "off":
                self.txtest = None
            return f"  txtest: {'suite at 100 % armed for the next PTT key-up' if self.txtest else 'off (nothing armed)'}\n"
        if w[0] == "get":
            p = " ".join(w[1:])
            return f"{p} {'End tone' if p == END_TONE else 'TX timeout'}: {self.set[p]}\n"
        if w[0] == "set":
            p = " ".join(w[1:5])
            self.set[p] = " ".join(w[5:])
            return f"{p} x: {self.set[p]}\n"
        if w[0] == "slot":
            if len(w) == 1:
                return "".join(self._line(n) for n in range(3))
            n = "ABC".index(w[1].upper())
            s = self.slots[n]
            i = 2
            while i < len(w):
                x = w[i].lower()
                if x == "vfo":
                    if s["ch"] is not None:
                        self.slots[n] = s = dict(self.vfo_b)
                elif x == "active":
                    self.active = n
                elif x in ("fm", "nfm", "am"):
                    s["mode"] = x.upper()
                elif x in ("low", "mid", "high"):
                    s["power"] = self.stuck_power or x
                elif x in ("tone", "txtone", "rxtone"):
                    i += 1
                    if x in ("tone", "txtone"):
                        s["tx"] = w[i]
                    if x in ("tone", "rxtone"):
                        s["rx"] = w[i]
                elif x[0].isdigit():
                    if s["ch"] is not None:
                        raise TermError(f"slot {w[1]}: the slot is on a channel: add vfo to tune its VFO")
                    if not self.ignore_freq:
                        f = float(x)
                        s["offset"] = 0 if not (144 <= f <= 148 or 420 <= f <= 450) else s["offset"]
                        s["freq"] = f
                i += 1
            return self._line(n)
        raise TermError(f"{w[0]}: no such command")

    def slot(self, n):
        s = self.slots[n]
        return dict(active=self.active == n, channel=s["ch"], freq10=mhz10(s["freq"]), mode=["FM", "NFM", "AM"].index(s["mode"]),
                    power=["low", "mid", "high"].index(s["power"]), rx_tone=0 if s["rx"] == "off" else 1,
                    tx_tone=0 if s["tx"] == "off" else 1, offset=s["offset"], split=False, aprs=False, scanning=False)

    def _refuse(self, f):
        if abs(f - 243.0) < 0.001 or abs(f - 121.5) < 0.001:
            return "invalid: emergency frequency: only PTT on the radio, twice"
        if 222 <= f <= 225:
            return "invalid: 1.25 m locked"
        if not any(a <= f <= b for a, b in self.BANDS):
            return "invalid: outside the band plan"
        return None

    def key_on(self, slot_code):
        self.calls.append("KEY on")
        s = self.slots[self.active]
        why = self._refuse(s["freq"])
        if why:
            return 9, why
        t = self.clock.time()
        self.key = dict(on=t, last=t, mhz=s["freq"], kind="suite" if self.txtest else "carrier")
        self.txtest = None
        return None if self.hold else (0, "")

    def _check(self):
        k = self.key
        if not k:
            return None
        t = self.clock.time()
        tmo = int(re.match(r"\d+", self.set[TX_TIMEOUT]).group())
        end = min(k["last"] + 2.0, k["on"] + tmo)
        if t >= end:
            self.carriers.append((k["on"], end, k["mhz"], k["kind"]))
            self.key = None
            msg = "invalid: TX timeout" if end == k["on"] + tmo else "invalid: keepalive lost: unkeyed"
            self.unkey_msgs.append(msg)
            return msg
        return None

    def key_alive(self):
        self.alive_n += 1
        if self.fail_alive_after and self.alive_n > self.fail_alive_after:
            raise BenchError("the cable fell out (fake)")
        self._check()
        if self.key:
            self.key["last"] = self.clock.time()

    def poll_unkey(self):
        self._check()
        return self.unkey_msgs.pop(0) if self.unkey_msgs else None

    def key_off(self):
        self.calls.append("KEY off")
        self._check()
        if self.key:
            self.carriers.append((self.key["on"], self.clock.time(), self.key["mhz"], self.key["kind"]))
            self.key = None
        return True


class FakeSdr:
    """Synthetic captures from the fake radio's carrier log: the suite (sdrsuite's model radio) or a bare carrier."""

    def __init__(self, radio, clock, seed=7):
        self.r, self.clock, self.rng = radio, clock, np.random.default_rng(seed)

    def start(self, mhz, seconds, path):
        self.mhz, self.seconds, self.path, self.t0 = mhz, seconds, path, self.clock.time()

    def finish(self):
        fs = sdrtx.FS
        cs = [c for c in self.r.carriers if c[1] > self.t0 and c[0] < self.t0 + self.seconds and abs(c[2] - self.mhz) < 0.01]
        if cs and cs[0][3] == "suite":
            on, off = cs[0][0] - self.t0, cs[0][1] - self.t0
            raw, _ = sdrsuite.synth_iq(self.rng, pre=on, hold=max(off - on - sdrsuite.SUITE_S, 0.2),
                                       post=max(self.seconds - off, 0.2))
        else:
            n = int(self.seconds * fs)
            raw = np.empty(2 * n, np.uint8)
            chunk = 1 << 20
            for a in range(0, n, chunk):
                idx = np.arange(a, min(a + chunk, n))
                t = idx / fs + self.t0
                on = np.zeros(len(idx), bool)
                for c in cs:
                    on |= (t >= c[0]) & (t < c[1])
                x = np.where(on, 60.0, 0.0) * np.exp(-2j * np.pi * sdrtx.OFF * idx / fs) \
                    + self.rng.normal(0, 1.5, len(idx)) + 1j * self.rng.normal(0, 1.5, len(idx))
                raw[2 * a: 2 * idx[-1] + 2: 2] = np.clip(np.round(x.real + 127.5), 0, 255)
                raw[2 * a + 1: 2 * idx[-1] + 2: 2] = np.clip(np.round(x.imag + 127.5), 0, 255)
        raw.tofile(self.path)
        return raw

    def stop(self):
        pass


class SimSdr(FakeSdr):
    """For --selftest --sim: the simulator keys nothing on air, so its carriers are taken from the bench's own key
    events (on to the last keepalive + 2 s, or the KEY off), the suite from sdrsuite's model."""

    def __init__(self, clock, bench_ref):
        super().__init__(None, clock)
        self.bench_ref = bench_ref
        self.r = self

    @property
    def carriers(self):
        ev = self.bench_ref.get("last_ev")
        if not ev or not ev.get("accepted"):
            return []
        end = min(ev["t_off"], (ev["t_last_alive"] or ev["t_on"]) + 2.0)
        return [(ev["t_on"], end, self.mhz, "suite" if ev["purpose"] in [r["name"] for r in DEFAULTS["runs"]] else "carrier")]


# ---- Main flows ----


def load_cfg(path):
    cfg = json.loads(json.dumps(DEFAULTS))
    if path:
        cfg.update(json.load(open(path)))
    return cfg


def pick_runs(cfg, names):
    if not names:
        return cfg["runs"]
    if names == "none":  # only the extra checks (--refusals/--keepalive/--timeout): no 60 s suite keying
        return []
    by = {r["name"]: r for r in cfg["runs"]}
    out = []
    for n in names.split(","):
        if n not in by:
            sys.exit(f"no run {n!r}; the runs: {', '.join(by)}")
        out.append(by[n])
    return out


def find_captures(cfg, specs):
    """--load: DIRs searched (recursively) for <run>.u8 or an alias's; or RUN=FILE."""
    found = {}
    names = {r["name"]: r for r in cfg["runs"]}
    for spec in specs:
        if "=" in spec and not os.path.isdir(spec):
            n, p = spec.split("=", 1)
            found[n] = p
            continue
        for dp, _, fs in os.walk(spec):
            for f in sorted(fs):
                if not f.endswith(".u8"):
                    continue
                stem = f[:-3]
                n = stem if stem in names else cfg["aliases"].get(stem)
                if n in names and n not in found:
                    found[n] = os.path.join(dp, f)
            for f in fs:
                if f in ("keepalive.u8", "timeout.u8") and os.path.exists(os.path.join(dp, f[:-3] + ".json")):
                    found.setdefault("_" + f[:-3], os.path.join(dp, f))
    return found


def do_load(cfg, a):
    caps = find_captures(cfg, a.load)
    runs = [r for r in cfg["runs"] if r["name"] in caps and (not a.runs or r["name"] in a.runs.split(","))]
    if not runs:
        sys.exit(f"no captures found in {a.load} (looked for {', '.join(r['name'] + '.u8' for r in cfg['runs'])} and "
                 f"{', '.join(k + '.u8' for k in cfg['aliases'])})")
    label = a.build or ("load-" + slug(os.path.basename(os.path.normpath(a.load[0]))))
    session, sess_dir = new_session(cfg, label)
    build = dict(label=label, version=None, host=host_build(), source="load")
    say(f"re-analysing {len(runs)} capture(s) into {sess_dir}")
    out = [analyse_run(cfg, a, r, caps[r["name"]], sess_dir, session, build) for r in runs]
    extras = {}
    for k, fn in (("keepalive", analyse_keepalive), ("timeout", analyse_timeout)):
        p = caps.get("_" + k)
        if p:
            side = json.load(open(p[:-3] + ".json"))
            extras[k] = fn(np.fromfile(p, np.uint8), side["events"], side["cfg"])
    page = write_session(cfg, sess_dir, session, out, extras,
                         dict(build=build, source="re-analysed captures from " + ", ".join(
                             os.path.basename(os.path.normpath(x)) for x in a.load), port=None, slot=cfg["slot"]))
    finish_report(out, extras, page)
    return out, page


def finish_report(runs, extras, page):
    say("\nsummary:")
    for j in runs:
        rows = j.get("comparison", [])
        fl_ = [r for r in rows if r[5] in ("CHANGED", "missing")]
        say(f"  {j['run']:9s} {'ok  ' if j['ok'] else 'FAIL'}  " + "  ".join(f"{k}: {v}" for k, v in headline(j["metrics"])[:4]))
        if j.get("previous"):
            say(f"            against {j['previous']['session']}: " + (", ".join(
                f"{r[0]} {fmt(r[2])} -> {fmt(r[1])}" for r in fl_[:8]) + (" ..." if len(fl_) > 8 else "") if fl_ else "all within tolerance"))
    for k, v in extras.items():
        say(f"  {k}: {'pass' if all_ok(v) else 'FAIL'}")
    say(f"\npage: {page}")


def do_bench(cfg, a, radio=None, sdr=None, clock=None):
    runs = pick_runs(cfg, a.runs)
    clock = clock or Clock()
    image = sha = None
    if a.flash:
        radio, image, sha = do_flash(cfg, a, a.flash)
    radio = radio or Radio(cfg["port"])
    bench = Bench(cfg, a, radio=radio, sdr=sdr or Sdr(cfg, clock), clock=clock)
    _CURRENT.append(bench)
    try:
        hello = radio.hello() if hasattr(radio, "hello") else {}
        info = radio.info()
        say(f"radio: firmware {info['version']}, callsign {info.get('callsign') or '-'}")
        label = a.build or (f"{info['version']}-img{sha}" if sha else f"{info['version']}-host{host_build()}")
        build = dict(label=label, version=info["version"], host=host_build(), image=f"{image} {sha}" if image else None)
        session, sess_dir = new_session(cfg, label)
        say(f"session {sess_dir}")
        meta = dict(build=build, source="on air", port=getattr(radio, "spec", "fake"), slot=cfg["slot"],
                    commit=host_build(), kinds=["rf"],
                    sim=not isinstance(radio, Radio) or not radio.cable)  # selftest or simulator: never recorded
        try:
            caps, extras = bench.on_air(sess_dir, runs, hello, info, flashed_dry=image)
        except SafetyError as e:
            meta.update(gates=bench.gatelog, warnings=bench.warnings + [str(e)], restore_notes=bench.restore_notes,
                        tx_dry=bench.tx_dry, refused=str(e))
            json.dump(jsonable(meta), open(os.path.join(sess_dir, "session.json"), "w"), indent=1)
            say(f"\n{e}")
            raise
        meta.update(gates=bench.gatelog, warnings=bench.warnings, restore_notes=bench.restore_notes, tx_dry=bench.tx_dry)
    finally:
        bench.emergency_unkey()
        _CURRENT.remove(bench)
    out = []
    for c in caps:
        out.append(analyse_run(cfg, a, c["run"], c["capture"], sess_dir, session, build, events=c["events"]))
        if not a.keep_captures:
            with contextlib.suppress(OSError):
                os.remove(c["capture"])
    page = write_session(cfg, sess_dir, session, out, extras, meta)
    finish_report(out, extras, page)
    return out, extras, page, bench


_CURRENT = []


def _last_ditch():
    for b in list(_CURRENT):
        b.emergency_unkey()


atexit.register(_last_ditch)


# ---- Change-aware selection (--auto) ----

# (glob, class, why): first match wins. Paths are in the ferrite checkout. Classes: link (link, store, boot,
# ports: both sides and a reboot check), rf (radio, drivers, DSP, TX, calibration, firmware tasks: the SDR suite on
# tx-dry and the UI smoke set), ui (screens, drawing, app logic, docs: the UI tests on the normal build), host
# (tests, the simulators, CI, PC tools that don't touch the radio: nothing on the radio).
CLASSES = [
    # Link, store, boot, ports, the image itself.
    ("crates/ferrite-app/src/link.rs", "link", "the link"),
    ("crates/ferrite-app/src/link/*", "link", "the link"),
    ("crates/ferrite-core/src/link.rs", "link", "the link protocol"),
    ("crates/ferrite-core/src/memmap.rs", "link", "the flash map"),
    ("crates/ferrite-store/*", "link", "the store"),
    ("crates/ferrite-app/src/persist.rs", "link", "what is saved"),
    ("crates/ferrite-app/src/ports.rs", "link", "the ports"),
    ("crates/ferrite-radio/src/cable.rs", "link", "the cable"),
    ("crates/ferrite-radio/src/flash.rs", "link", "the SPI flash"),
    ("firmware/src/tasks/link.rs", "link", "the link task"),
    ("firmware/src/tasks/storage.rs", "link", "the storage task"),
    ("firmware/src/store.rs", "link", "the store"),
    ("firmware/src/main.rs", "link", "boot"),
    ("firmware/src/board.rs", "link", "boot"),
    ("firmware/src/bringup.rs", "link", "boot"),
    ("firmware/src/report.rs", "link", "the boot report"),
    ("firmware/src/hal/uart.rs", "link", "the UART"),
    ("firmware/src/hal/clock.rs", "link", "clocks"),
    ("firmware/src/hal/irq.rs", "link", "interrupts"),
    ("firmware/src/hal/sys.rs", "link", "the system"),
    ("firmware/memory.x", "link", "the memory map"),
    ("firmware/build.rs", "link", "the build"),
    ("firmware/Cargo.toml", "link", "the firmware's features"),
    ("drivers/w25q/*", "link", "the SPI flash driver"),
    ("Cargo.toml", "link", "dependencies"),
    ("Cargo.lock", "link", "dependencies"),
    ("rust-toolchain.toml", "link", "the toolchain"),
    (".cargo/*", "link", "the build"),
    ("tools/ferrite_link.py", "link", "the PC side of the link"),
    ("tools/flash.py", "link", "flashing"),
    ("tools/btf.py", "link", "the image format"),
    ("tools/mkbtf.sh", "link", "the image build"),
    # Radio, drivers, DSP, transmit, calibration.
    ("firmware/src/tasks/radio/*", "rf", "the Radio task"),
    ("firmware/src/tasks/dsp.rs", "rf", "DSP"),
    ("firmware/src/tasks/decode.rs", "rf", "decoders"),
    ("firmware/src/tasks/aprs.rs", "rf", "APRS"),
    ("firmware/src/tasks/gps.rs", "rf", "GPS"),
    ("firmware/src/tasks/input.rs", "ui", "keys"),
    ("firmware/src/tasks/ui.rs", "ui", "the UI task"),
    ("firmware/src/display.rs", "ui", "the display"),
    ("firmware/src/hal/backlight.rs", "ui", "the backlight"),
    ("firmware/*", "rf", "firmware"),
    ("drivers/st7789/*", "ui", "the display driver"),
    ("drivers/keypad/*", "ui", "the keypad driver"),
    ("drivers/*", "rf", "a chip driver"),
    ("crates/ferrite-radio/*", "rf", "chip recipes"),
    ("crates/ferrite-core/src/tx*.rs", "rf", "transmit"),
    ("crates/ferrite-core/src/*cal*.rs", "rf", "calibration"),
    ("crates/ferrite-core/src/tonegen.rs", "rf", "tones"),
    ("crates/ferrite-core/src/audio.rs", "rf", "audio"),
    ("crates/ferrite-core/src/settle.rs", "rf", "chip timing"),
    ("crates/ferrite-core/src/rxsleep.rs", "rf", "receive sleep"),
    ("crates/ferrite-core/src/vox.rs", "rf", "VOX"),
    ("crates/ferrite-core/src/ssb.rs", "rf", "SSB"),
    ("crates/ferrite-core/src/xb.rs", "rf", "crossband"),
    ("crates/ferrite-core/src/same*.rs", "rf", "SAME"),
    ("crates/ferrite-core/src/decoders.rs", "rf", "decoders"),
    ("crates/ferrite-core/src/stuck.rs", "rf", "the PTT check"),
    ("crates/ferrite-core/src/gpspower.rs", "rf", "GPS power"),
    ("crates/ferrite-core/src/chipid.rs", "rf", "chip IDs"),
    ("crates/ferrite-core/src/acc.rs", "rf", "the accumulator"),
    ("tools/bench.py", "rf", "the bench itself"),
    ("tools/sdr*.py", "rf", "the SDR analysis"),
    # The UI and the app's logic.
    ("crates/ferrite-app/src/term/*", "ui", "the terminal"),
    ("crates/ferrite-app/*", "ui", "the app"),
    ("crates/ferrite-ui/*", "ui", "drawing"),
    ("crates/ferrite-core/*", "ui", "core logic"),
    ("fonts/*", "ui", "fonts"),
    ("assets/*", "ui", "assets"),
    ("tools/uitest.py", "ui", "the UI tests"),
    ("tools/uitests/*", "ui", "the UI tests"),
    ("README.md", "ui", "docs"),
    ("*.md", "ui", "docs"),
    # Nothing on the radio.
    ("*/tests/*", "host", "host tests"),
    ("*/tests.rs", "host", "host tests"),
    ("*/tests_*.rs", "host", "host tests"),
    ("sim/*", "host", "the simulator"),
    ("sim-web/*", "host", "the web simulator"),
    (".github/*", "host", "CI"),
    ("crates/ferrite-checks/*", "host", "the rule checks"),
    ("tools/*", "host", "a PC tool"),
]
CLASS_ORDER = {"host": 0, "ui": 1, "rf": 2, "link": 3}


# Screen files several screens share: a change there runs every UI test, not a page's.
SHARED_SCREEN_FILES = {"mod", "shared", "lists", "forms", "parts", "table", "nav", "layouts"}


def screen_pages(paths):
    """The pages (screen file stems) when every changed UI file is one screen's own file
    (crates/ferrite-app/src/screens/<page>.rs, not a shared one), else None: --auto then runs only the UI tests
    that visit those screens (uitest.py --page)."""
    pages = set()
    for f in paths:
        m = re.fullmatch(r"crates/ferrite-app/src/screens/(\w+)\.rs", f)
        if not m or m.group(1) in SHARED_SCREEN_FILES or m.group(1).startswith("tests"):
            return None
        pages.add(m.group(1))
    return sorted(pages) or None


def classify(path):
    """(class, why) of one changed file; unknown files count as link (both sides and a reboot: the safe side)."""
    for glob, cls, why in CLASSES:
        if fnmatch.fnmatch(path, glob):
            # Host tests inside src/ (tests.rs, tests/...) don't change the image.
            if cls != "host" and re.search(r"(^|/)tests?(_\w+)?(\.rs|/)", path) and path.endswith(".rs"):
                return "host", "host tests"
            return cls, why
    return "link", "not classified: both sides to be safe"


def git_ferrite(*a):
    r = subprocess.run(["git", "-C", FERRITE, *a], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def changed_files(base):
    """Files changed from base to HEAD (and uncommitted ones); None if base isn't in this checkout."""
    if git_ferrite("cat-file", "-e", f"{base}^{{commit}}") is None:
        return None
    out = git_ferrite("diff", "--name-only", f"{base}..HEAD") or ""
    out += git_ferrite("diff", "--name-only", "HEAD") or ""
    return sorted({x for x in out.splitlines() if x})


def last_runs(results):
    """The newest on-radio session per side: {"ui": (session, commit), "rf": (...)}. Simulator, selftest and
    --load sessions don't count."""
    found = {}
    if not os.path.isdir(results):
        return found
    for name in sorted(os.listdir(results)):
        p = os.path.join(results, name, "session.json")
        try:
            m = json.load(open(p))
        except (OSError, ValueError):
            continue
        if m.get("source") not in ("on air", "radio") or str(m.get("port", "")).startswith("tcp:"):
            continue
        commit = (m.get("commit") or (m.get("build") or {}).get("host") or "").rstrip("+")
        if not commit or commit == "unknown":
            continue
        kinds = m.get("kinds") or (["rf"] if m.get("runs") else [])
        for k in kinds:
            found[k] = (name, commit)
    return found


def decide(cfg, a):
    """What --auto runs and why: dict(ui=None|"full"|"smoke", rf=bool, reboot=bool, what, why, files)."""
    head = (git_ferrite("rev-parse", "--short=7", "HEAD") or "?").strip()
    if a.all:
        return dict(ui="full", rf=True, reboot=True, what="everything (--all)", why="forced", files=[], head=head)
    if a.ui or a.rf:
        return dict(ui="full" if a.ui else ("smoke" if a.rf else None), rf=bool(a.rf), reboot=False,
                    what=("UI tests" if a.ui else "") + (" and " if a.ui and a.rf else "") + ("the SDR suite" if a.rf else ""),
                    why="forced (--ui/--rf)", files=[], head=head)
    last = last_runs(cfg["results"])
    if a.diff_files is not None:
        files = {k: [f for f in a.diff_files.split(",") if f] for k in ("ui", "rf")}
        bases = {k: "(--diff-files)" for k in ("ui", "rf")}
    else:
        files, bases = {}, {}
        for k in ("ui", "rf"):
            if a.since:
                bases[k] = a.since
            elif k in last:
                bases[k] = last[k][1]
            else:
                bases[k] = None
            files[k] = changed_files(bases[k]) if bases[k] else None
    if files["ui"] is None and files["rf"] is None:
        why = ("no earlier on-radio run with a commit in " + cfg["results"]) if not last else             f"the last runs' commits ({', '.join(c for _, c in last.values())}) aren't in this checkout"
        return dict(ui="full", rf=True, reboot=True, what="everything: the first run", why=why, files=[], head=head)
    cls = {k: [(f, *classify(f)) for f in (v or [])] for k, v in files.items()}
    # A side with no earlier on-radio run runs in full (the first time: everything on that side).
    rf_cls = {c for _, c, _ in cls["rf"]} if files["rf"] is not None else {"rf"}
    ui_cls = {c for _, c, _ in cls["ui"]} if files["ui"] is not None else {"ui"}
    first = [k.upper() for k in ("ui", "rf") if files[k] is None]
    allf = sorted({(f, f"{c}: {w}") for k in cls for f, c, w in cls[k]})
    link = "link" in rf_cls | ui_cls
    rf = link or "rf" in rf_cls
    ui_full = link or bool(ui_cls & {"ui"})
    since = "; ".join(f"{k.upper()} since {bases[k]}" + (f" ({last[k][0]})" if k in last and bases[k] == last[k][1] else "")
                      for k in ("ui", "rf") if bases.get(k))
    names = lambda c: ", ".join(f for f, cc, _ in sorted({x for k in cls for x in cls[k]}) if cc == c)  # noqa: E731
    if link:
        d = dict(ui="full", rf=True, reboot=True, what="both: the UI tests and the SDR suite on tx-dry, then a reboot check",
                 why=f"link/store/boot/port changes ({names('link')})")
    elif rf:
        d = dict(ui="full" if ui_full else "smoke", rf=True, reboot=False,
                 what="the SDR suite on tx-dry + " + ("the UI tests" if ui_full else "the UI smoke set"),
                 why=f"radio/driver/DSP/TX/calibration changes ({names('rf')})"
                 + (f"; UI changes too ({names('ui')})" if ui_full else ""))
    elif ui_full and screen_pages([f for k in cls for f, c, _ in cls[k] if c == "ui"]):
        pages = screen_pages([f for k in cls for f, c, _ in cls[k] if c == "ui"])
        d = dict(ui="full", pages=pages, rf=False, reboot=False,
                 what=f"the UI tests that visit {', '.join(pages)} (--page) on the normal build only",
                 why=f"only screen files changed ({names('ui')})")
    elif ui_full:
        d = dict(ui="full", rf=False, reboot=False, what="the UI tests on the normal build only (no TX, no SDR)",
                 why=f"UI/app-only changes ({names('ui')})")
    elif allf:
        d = dict(ui=None, rf=False, reboot=False, what="nothing on the radio",
                 why=f"host-only changes (tests, simulators, CI, PC tools: {names('host')}); --ui or --rf to run anyway")
    else:
        d = dict(ui=None, rf=False, reboot=False, what="nothing on the radio", why="nothing changed since the last runs")
    if first:
        d["why"] += f"; no earlier on-radio {' or '.join(first)} run: that side runs in full"
    d.update(files=allf, head=head, since=since)
    return d


def print_decision(d):
    say(f"decision: {d['what']}")
    say(f"  why: {d['why']}" + (f" [{d['since']}]" if d.get("since") else "") + f"; HEAD {d['head']}")
    for f, c in d.get("files", [])[:40]:
        say(f"    {c:28s} {f}")
    if len(d.get("files", [])) > 40:
        say(f"    ... {len(d['files']) - 40} more")


def reboot_check(port, log=say):
    """LINK-24 restart (into the bootloader, which starts Ferrite again after ~200 ms), HELLO again within 60 s,
    home on the screen, and every setting, channel and zone as before (`dump`)."""
    r = fl.Radio(port)
    before = r.cmd("dump")
    r.link.ok(fl.REBOOT)
    r.close()
    t0 = time.time()
    time.sleep(3)
    while True:
        try:
            r = fl.Radio(port)
            break
        except Exception:
            if time.time() - t0 > 60:
                return dict(ok=False, text="no HELLO within 60 s of the restart")
            time.sleep(2)
    up = time.time() - t0
    time.sleep(2)
    screen = r.ui()["screen"]["id"]
    after = r.cmd("dump")
    r.close()
    la, lb = [x for x in before.splitlines() if not x.startswith("#")], [x for x in after.splitlines() if not x.startswith("#")]
    diff = [x for x in la if x not in lb] + [x for x in lb if x not in la]
    ok = screen == "Home" and not diff
    text = f"back after {up:.0f} s; screen {screen}; dump {'unchanged' if not diff else 'changed: ' + '; '.join(diff[:6])}"
    log(f"reboot check: {'pass' if ok else 'FAIL'}: {text}")
    return dict(ok=ok, text=text)


def do_auto(cfg, a):
    """--auto (and --all, --ui, --rf): decide, then run what was decided, one session and one page for both."""
    import uitest
    d = decide(cfg, a)
    print_decision(d)
    if a.dry_plan:
        if d["rf"]:
            say("")
            plan(cfg, a, pick_runs(cfg, a.runs))
        if d["ui"]:
            say(f"UI: tools/uitest.py --port {cfg['port']}{' --smoke' if d['ui'] == 'smoke' else ''}"
                f"{' --page ' + ','.join(d['pages']) if d.get('pages') else ''} (remote keys and readback; "
                "no PTT; settings and slots put back)")
        if d["reboot"]:
            say("reboot: LINK-24 restart, HELLO within 60 s, home on the screen, `dump` unchanged")
        return 0
    if not (d["ui"] or d["rf"] or d["reboot"]):
        return 0
    bad = False
    sess_dir = page = None
    meta = None
    if d["rf"]:
        try:
            out, extras, page, _ = do_bench(cfg, a)
        except SafetyError as e:
            say(f"SDR suite refused: {e}")
            return 2
        sess_dir = os.path.dirname(page)
        meta = json.load(open(os.path.join(sess_dir, "session.json")))
        runs = out
        bad |= any(not j["ok"] for j in out) or any(not all_ok(v) for v in extras.values())
    else:
        runs, extras = [], {}
    ui_summary = None
    if d["ui"]:
        ua = uitest.main_args(["--port", cfg["port"]] + (["--smoke"] if d["ui"] == "smoke" else [])
                              + (["--page", ",".join(d["pages"])] if d.get("pages") else []))
        sel = uitest.select(uitest.load_tests(), ua, cfg["port"].startswith("tcp:"))
        if d.get("pages") and not sel:
            say(f"no UI test visits {', '.join(d['pages'])}: the full set instead")
            ua = uitest.main_args(["--port", cfg["port"]])
            sel = uitest.select(uitest.load_tests(), ua, cfg["port"].startswith("tcp:"))
        sim = cfg["port"].startswith("tcp:")
        ui_summary = uitest.run(ua, cfg["port"], sim, sel)
        if sess_dir is None:
            label = f"{ui_summary['version']}-{ui_summary['commit']}-{ui_summary['build']}"
            session, sess_dir = new_session(cfg, label)
            meta = dict(build=dict(label=label, version=ui_summary["version"], host=host_build()),
                        source="simulator" if sim else "radio",
                        port=cfg["port"], commit=host_build(), kinds=[])
        uitest.write_results(ui_summary, sess_dir)
        meta["kinds"] = sorted(set(meta.get("kinds", [])) | {"ui"})
        meta["ui"] = dict(counts=ui_summary["counts"], set="smoke" if d["ui"] == "smoke" else "full")
        c = ui_summary["counts"]
        bad |= bool(c["fail"] or c["error"] or ui_summary["left_changed"])
    if d["reboot"]:
        rb = reboot_check(cfg["port"])
        meta["reboot"] = rb
        bad |= not rb["ok"]
    meta["decision"] = {k: d[k] for k in ("what", "why", "files", "head")}
    json.dump(jsonable(meta), open(os.path.join(sess_dir, "session.json"), "w"), indent=1)
    if d["rf"] or d["reboot"]:
        write_rule_results(sess_dir)
    page_meta = dict(meta, ui=ui_summary) if ui_summary else meta
    page = os.path.join(sess_dir, "summary.html")
    open(page, "w").write(summary_page(os.path.basename(sess_dir), sess_dir, runs, jsonable(extras), page_meta))
    say(f"\npage: {page}")
    if getattr(a, "record", False):
        record_session(sess_dir, a.docs)
    return 1 if bad else 0


def selftest_auto():
    """The selection rules on example diffs (no radio): each example's decision."""
    examples = [
        (["crates/ferrite-app/src/screens/home.rs", "crates/ferrite-ui/src/widgets.rs"], "ui", False, False),
        (["crates/ferrite-app/src/screens/zones.rs", "crates/ferrite-app/src/screens/browse.rs"], ("ui", ["browse", "zones"]), False, False),
        (["crates/ferrite-app/src/screens/zones.rs", "crates/ferrite-app/src/screens/lists.rs"], "ui", False, False),
        (["firmware/src/tasks/radio/tx.rs", "crates/ferrite-app/src/screens/home.rs"], "full", True, False),
        (["drivers/bk4829/src/lib.rs"], "smoke", True, False),
        (["crates/ferrite-core/src/txcal.rs"], "smoke", True, False),
        (["crates/ferrite-store/src/lib.rs"], "full", True, True),
        (["crates/ferrite-app/src/link.rs"], "full", True, True),
        (["sim/src/main.rs", "crates/ferrite-app/src/tests/qmenu.rs", ".github/workflows/ci.yml"], None, False, False),
        (["some/new/thing.rs"], "full", True, True),
    ]
    ok = True
    for files, ui, rf, rb in examples:
        a = argparse.Namespace(all=False, ui=False, rf=False, since=None, diff_files=",".join(files))
        d = decide(dict(results="/nonexistent"), a)
        ui, pages = ui if isinstance(ui, tuple) else (ui, None)
        want_ui = "full" if ui == "ui" else ui
        good = (d["ui"], d["rf"], d["reboot"], d.get("pages")) == (want_ui, rf, rb, pages)
        ok &= good
        say(f"  [{'pass' if good else 'FAIL'}] {', '.join(files)}: {d['what']}")
    return ok


# ---- Selftest ----


def selftest(a):
    tmp = a.out or tempfile.mkdtemp(prefix="bench-selftest-")
    ok = True
    lines = []

    def check(what, cond, got=""):
        nonlocal ok
        ok &= bool(cond)
        lines.append(f"  [{'pass' if cond else 'FAIL'}] {what}{': ' + str(got) if got != '' else ''}")
        say(lines[-1])

    def args(**kw):
        d = dict(runs=None, refusals=False, keepalive=False, timeout=False, i_know_its_tx_dry=False, clear_offset=False,
                 flash=None, build=None, keep_captures=True, verbose=False, load=None)
        d.update(kw)
        return argparse.Namespace(**d)

    cfg = load_cfg(a.config)
    cfg["results"] = os.path.join(tmp, "results")
    say(f"== selftest in {tmp}")

    def gated(what, radio_kw, a_kw, expect_in):
        clk = FakeClock()
        r = FakeRadio(clk, **radio_kw)
        try:
            do_bench(cfg, args(runs="vhf-wide", **a_kw), radio=r, sdr=FakeSdr(r, clk), clock=clk)
            check(what, False, "it keyed")
        except SafetyError as e:
            restored = r.slots[0]["freq"] == 146.52 and r.set == {END_TONE: "Beep", TX_TIMEOUT: "60 s"} and r.txtest is None
            unkeyed = r.key is None and r.calls.count("KEY on") == r.calls.count("KEY off")
            check(what, expect_in in str(e) and restored and r.key is None and unkeyed,
                  f"{str(e)[:90]}; keyed {'KEY on' in r.calls}; restored {restored}")
            return r

    # The gates.
    r = gated("gate: no tx-dry evidence, no flag -> refused before anything", {}, {}, "tx-dry build")
    check("  ... and nothing was changed or keyed", r.calls == [], r.calls)
    gated("gate: a receive-only build", {"version": "v0.1.0 tx-dry", "rx_only": True}, {}, "a transmit build")
    gated("gate: the radio says tx (LINK-35), even with --i-know-its-tx-dry", {"build": "tx"},
          {"i_know_its_tx_dry": True}, "says it runs a tx build")
    gated("gate: the radio says receive (LINK-35)", {"build": "receive"}, {"i_know_its_tx_dry": True},
          "says it runs a receive build")
    gated("gate: frequency not approved (the radio ignores the tune)", {"version": "v0.1.0-txdry", "ignore_freq": True}, {},
          "frequency approved")
    gated("gate: power not low", {"stuck_power": "high"}, {"i_know_its_tx_dry": True}, "power low")
    gated("gate: an offset on the VFO", {"offset": -60000}, {"i_know_its_tx_dry": True}, "no offset")
    r = gated("gate: a tx build holds KEY's answer on the cable (TX-43) -> unkeyed at once", {"hold_answers": True},
              {"i_know_its_tx_dry": True}, "wasn't answered")
    check("  ... KEY off right after KEY on", r.calls[r.calls.index("KEY on") + 1] == "KEY off", r.calls[-6:])
    # A failure while keyed: KEY off and the restore still happen.
    clk = FakeClock()
    r = FakeRadio(clk, fail_alive_after=10)
    try:
        do_bench(cfg, args(runs="vhf-wide", i_know_its_tx_dry=True), radio=r, sdr=FakeSdr(r, clk), clock=clk)
        check("a failure while keyed", False, "no error")
    except BenchError as e:
        check("a failure while keyed: KEY off sent, everything restored", "KEY off" in r.calls and r.key is None
              and r.slots[0]["freq"] == 146.52 and r.set[TX_TIMEOUT] == "60 s" and r.set[END_TONE] == "Beep", str(e))
    # --clear-offset makes the offset gate pass, and says it wasn't put back.
    clk = FakeClock()
    r = FakeRadio(clk, offset=-60000, version="v0.1.0 TX DRY")
    b = Bench(cfg, args(clear_offset=True), radio=r, sdr=FakeSdr(r, clk), clock=clk)
    b.check_build(r.hello(), r.info())
    b.save_state()
    b.setup_slot(145.495, "fm")
    st = b.gate_key(cfg["test_freqs"], "clear-offset")
    b.restore()
    check("--clear-offset: the gate passes and the lost shift is noted", st["offset"] == 0 and any("shift" in n for n in b.restore_notes))
    # Images: tx-dry or not (flash refuses the latter before flash.py runs).
    import btf
    for name, body, want in (("dry", b"x" * 4000 + b"TX DRY: PA drive held at 0", True), ("tx", b"x" * 4100, False)):
        p = os.path.join(tmp, f"img-{name}.btf")
        open(p, "wb").write(btf.pack(body.ljust(16384, b"\xff")))
        check(f"image check: {name} image {'is' if want else 'is not'} tx-dry", image_is_tx_dry(p)[0] == want)
    try:
        do_flash(dict(cfg, port="-"), args(), os.path.join(tmp, "img-tx.btf"))
        check("--flash refuses a non-tx-dry image", False)
    except SafetyError as e:
        check("--flash refuses a non-tx-dry image", True, str(e)[:60])

    # The whole flow: one suite run and the three extra checks, on the fake radio and synthetic IQ.
    say("\n== the whole flow: vhf-wide + refusals + keepalive + timeout (fake radio, synthetic IQ)")
    clk = FakeClock()
    r = FakeRadio(clk, version="v0.1.0 TX DRY")
    before = json.dumps(r.slots), dict(r.set), r.active
    out, extras, page, bench = do_bench(cfg, args(runs="vhf-wide", refusals=True, keepalive=True, timeout=True, build="selftest"),
                                        radio=r, sdr=FakeSdr(r, clk), clock=clk)
    j = out[0]
    check("suite run: all seven markers measured", j["metrics"].get("markers_found") == 7, j["metrics"].get("markers_found"))
    check("suite run: DCS 023 N decoded", j["metrics"].get("dcs.code") == "023 N", j["metrics"].get("dcs.code"))
    check("suite run: 4FSK no errors", j["metrics"].get("fsk4.errors") == 0, j["metrics"].get("fsk4.errors"))
    check("refusals: all three refused with their reasons, no carrier", all(x["ok"] for x in extras["refusals"]),
          [x.get("refused") for x in extras["refusals"]])
    check("keepalive: dropped within 2.5 s", extras["keepalive"]["ok"], extras["keepalive"]["text"])
    check("timeout: carrier ~30 s", extras["timeout"]["ok"], extras["timeout"]["text"])
    check("restored exactly (slots, settings, active slot)", (json.dumps(r.slots), dict(r.set), r.active) == before)
    check("every KEY on has its KEY off", r.calls.count("KEY on") == r.calls.count("KEY off") and r.key is None,
          f"{r.calls.count('KEY on')} on, {r.calls.count('KEY off')} off")
    check("the gates logged and all passed", bench.gatelog and all(g["ok"] for g in bench.gatelog), len(bench.gatelog))
    pg = open(page).read()
    check("summary page: self-contained, both themes, plain Hz ticks", "prefers-color-scheme:dark" in pg and "data-theme=dark" in pg
          and ">1000</text>" in pg and not any(x in pg for x in ("<script", "src=", "@import", "<link")), page)

    # The comparison: the same capture again (all within tolerance), then a perturbed copy (flags).
    say("\n== --load the saved capture: compared with the run above")
    cap = j["capture"]
    out2, page2 = do_load(cfg, args(load=[os.path.dirname(cap)], build="selftest-load"))
    rows = out2[0].get("comparison", [])
    check("--load: compared with the previous session", out2[0].get("previous", {}).get("session") == j["session"])
    check("--load: same capture, nothing beyond tolerance", rows and not any(x[5] in ("CHANGED", "missing") for x in rows),
          [x[0] for x in rows if x[5] != "ok"][:5])
    m2 = dict(j["metrics"])
    m2["sweep.data.high_hz"] += 100
    m2["dcs.code"] = "023 I"
    m2["fsk4.errors"] = 3
    m2["afsk.dac100.mark_dev"] *= 1.02
    del m2["tone1k.tau_ms"]
    flagged = {x[0]: x[5] for x in compare(cfg, m2, j["metrics"]) if x[5] != "ok"}
    check("compare: changes beyond tolerance flagged, small ones not", flagged == {"sweep.data.high_hz": "CHANGED", "dcs.code": "CHANGED",
                                                                                  "fsk4.errors": "CHANGED", "tone1k.tau_ms": "missing"}, flagged)
    say("\n== --auto: the selection rules on example diffs")
    check("--auto: each example diff picks the right side", selftest_auto())
    say("\n".join(["", "selftest:"] + lines))
    say("selftest: " + ("all pass" if ok else "FAILED") + f"\npage: {page}\npage: {page2}")
    return ok


def selftest_sim(a):
    """The bench against the simulator's link (real frames, the terminal's own commands), the SDR faked."""
    cfg = load_cfg(a.config)
    tmp = a.out or tempfile.mkdtemp(prefix="bench-sim-")
    cfg["results"] = os.path.join(tmp, "results")
    radio = Radio(a.sim)
    h = radio.hello_ans
    if h.get("build") != "tx-dry" or not h.get("bench"):
        # Decision 151: the simulators run without the bench feature (no txtest), and say so in LINK-35's byte.
        say(f"selftest --sim: skipped: the simulator says it is a {h.get('build')} build"
            f"{' with' if h.get('bench') else ' without'} the bench feature; the bench needs a tx-dry bench build "
            "(txtest). The plain --selftest covers the flow and the gates.")
        radio.close()
        return True
    ok = True
    # The simulator's VFO A may carry a repeater shift; --clear-offset is what a radio with one would need.
    clk = Clock()
    ref = {}

    class Rec(Bench):
        def key_session(self, *k, **kw):
            ev = super().key_session(*k, **kw)
            ref["last_ev"] = ev
            return ev

    sdr = SimSdr(clk, ref)
    before = radio.term("slot A")
    timeout0 = None
    a2 = argparse.Namespace(runs="vhf-wide", refusals=True, keepalive=False, timeout=False, i_know_its_tx_dry=True,
                            clear_offset=True, flash=None, build="sim", keep_captures=False, verbose=False, load=None)
    cfg["suite_key_s"] = 63.5
    say(f"== the simulator at {a.sim}: vhf-wide (keyed 63.5 s in real time) + refusals; SDR faked")
    b = Rec(cfg, a2, radio=radio, sdr=sdr, clock=clk)
    hello, info = radio.hello(), radio.info()
    timeout0 = b.setting(TX_TIMEOUT)
    sess, sd = new_session(cfg, "sim")
    try:
        caps, extras = b.on_air(sd, pick_runs(cfg, "vhf-wide"), hello, info)
    finally:
        b.emergency_unkey()
    after = radio.term("slot A")
    j = analyse_run(cfg, a2, caps[0]["run"], caps[0]["capture"], sd, sess, dict(label="sim"))
    page = write_session(cfg, sd, sess, [j], extras, dict(build=dict(label="sim"), source="simulator", gates=b.gatelog,
                                                          warnings=b.warnings, restore_notes=b.restore_notes, tx_dry=b.tx_dry))
    for what, cond, got in (
            ("sim: the gates passed (the warning for --i-know-its-tx-dry printed)", all(g["ok"] for g in b.gatelog) and b.warnings, len(b.gatelog)),
            ("sim: the run keyed and was measured", j["metrics"].get("markers_found") == 7, j["metrics"].get("markers_found")),
            ("sim: the simulator refused each out-of-band key-up with its reason", all(x["ok"] for x in extras["refusals"]),
             [x.get("refused") for x in extras["refusals"]]),
            ("sim: slot A put back (the shift note aside)", before.strip().split(" offset")[0] == after.strip().split(" offset")[0], f"{before.strip()} / {after.strip()}"),
            ("sim: the TX timeout put back", b.setting(TX_TIMEOUT) == timeout0, timeout0)):
        ok &= bool(cond)
        say(f"  [{'pass' if cond else 'FAIL'}] {what}: {got}")
    radio.close()
    say(f"page: {page}\nselftest --sim: {'all pass' if ok else 'FAILED'}")
    return ok


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--config", help="JSON overriding any of DEFAULTS")
    ap.add_argument("--port", help="the cable (default from the config), tcp:HOST:PORT for the simulator, or ble:NAME_OR_ADDRESS for Bluetooth")
    ap.add_argument("--slot", help="the slot to use (A)")
    ap.add_argument("--runs", help="comma list of run names (default: all in the config)")
    ap.add_argument("--results", help="results root")
    ap.add_argument("--refusals", action="store_true", help="remote key refused at 160, 222, 243 MHz; no RF")
    ap.add_argument("--keepalive", action="store_true", help="keepalives stopped while keyed: carrier drop <= 2.5 s")
    ap.add_argument("--timeout", action="store_true", help="TX timeout 30 s, keyed 45 s: carrier ~30 s")
    ap.add_argument("--flash", metavar="IMAGE", help="flash this tx-dry image first (flash.py --backup)")
    ap.add_argument("--i-know-its-tx-dry", action="store_true", help="the radio runs a tx-dry build (can't be read over the link)")
    ap.add_argument("--clear-offset", action="store_true", help="clear the VFO's repeater shift by a tune to 160 MHz and back")
    ap.add_argument("--build", help="the build's label for the results (default: version + image or host git hash)")
    ap.add_argument("--no-keep-captures", dest="keep_captures", action="store_false", help="delete the .u8 after analysis")
    ap.add_argument("--verbose", action="store_true", help="sdrsuite's full report on the terminal too")
    ap.add_argument("--dry-plan", action="store_true", help="print the plan; touch nothing")
    ap.add_argument("--load", nargs="+", metavar="DIR|RUN=FILE", help="re-analyse saved captures")
    ap.add_argument("--selftest", action="store_true", help="fake radio + synthetic IQ: the whole flow and the gates")
    ap.add_argument("--sim", metavar="tcp:HOST:PORT", help="with --selftest: against the simulator's link")
    ap.add_argument("--out", help="selftest: where its results go")
    ap.add_argument("--auto", action="store_true", help="pick the UI tests and/or the SDR suite from what changed since "
                    "the last on-radio runs (git diff); prints the decision and why")
    ap.add_argument("--all", action="store_true", help="with --auto: everything (UI tests, SDR suite, reboot check)")
    ap.add_argument("--ui", action="store_true", help="with --auto: the UI tests (full set)")
    ap.add_argument("--rf", action="store_true", help="with --auto: the SDR suite on tx-dry (+ the UI smoke set)")
    ap.add_argument("--since", metavar="COMMIT", help="with --auto: diff from this commit, not the last runs'")
    ap.add_argument("--diff-files", metavar="A,B,...", help="with --auto: decide as if these files had changed")
    ap.add_argument("--record", action="store_true", help="after the run: each rule its checks name gets a result "
                    "row in docs/test-plan/*.md (tools/record_results.py; on-air runs only)")
    ap.add_argument("--docs", default=os.path.join(ROOT, "docs"), help="the docs checkout for --record")
    ap.add_argument("--rule-results", nargs="+", metavar="SESSION", help="(re)write bench/results.json for saved "
                    "sessions from their session.json and <run>.json (nothing keyed)")
    a = ap.parse_args()
    cfg = load_cfg(a.config)
    for k in ("port", "slot", "results"):
        if getattr(a, k):
            cfg[k] = getattr(a, k)
    if a.selftest:
        sys.exit(0 if (selftest_sim(a) if a.sim else selftest(a)) else 1)
    if a.rule_results:
        for d in a.rule_results:
            p, s = write_rule_results(d)
            say(f"{p}: " + ", ".join(f"{k} {'pass' if v['result'] == 'pass' else 'FAIL'}"
                                     + (" (part)" if v["scope"] == "part" else "") for k, v in sorted(s["rules"].items())))
        return
    if a.auto or a.all or a.ui or a.rf:
        sys.exit(do_auto(cfg, a))
    if a.dry_plan:
        plan(cfg, a, pick_runs(cfg, a.runs))
        return
    if a.load:
        do_load(cfg, a)
        return

    def on_term(*_):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, on_term)
    try:
        out, extras, page, _ = do_bench(cfg, a)
    except SafetyError:
        sys.exit(2)
    except KeyboardInterrupt:
        sys.exit("stopped (unkeyed and restored)")
    if a.record:
        record_session(os.path.dirname(page), a.docs)
    bad = any(not j["ok"] for j in out) or any(not all_ok(v) for v in extras.values())
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
