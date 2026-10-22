#!/usr/bin/env python3
"""The power bench: the radio on the DPS-150, each state set over the link, its current logged.

    tools/powerbench.py RADIO SUPPLY [--label 120MHz] [--seconds 60] [--settle 10] [--only NAME,...]
    tools/powerbench.py /dev/cu.usbserial-142220 /dev/cu.usbmodem161E49AD74961 --label 240MHz

RADIO is a ferrite_link port (cable, or ble:ID); SUPPLY the DPS-150's. Needs a bench build (the `lcd` command,
TERM-40). Each state: `lcd reset`, the state's terminal lines, `--settle` seconds, then `--seconds` of readings.
Prints a table of mA per state against the baseline, and appends every state to target/power/LABEL.csv.

The supply's output is never touched: the radio stays powered (set it and turn it on by hand, or with dps150.py).
Nothing here transmits.
"""
import argparse
import csv
import statistics
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from dps150 import Dps150, T_ALL  # noqa: E402
from ferrite_link import Radio  # noqa: E402

# The display states of the low-power bench (ui-backlog, "Ultra low power mode"); each against the baseline.
STATES = [
    ("baseline", []),
    ("bl 30", ["lcd bl 30"]),
    ("bl 0", ["lcd bl 0"]),
    ("keys 0", ["lcd keys 0"]),
    ("fps 1", ["lcd fps 1"]),
    ("idle", ["lcd idle on"]),
    ("partial", ["lcd partial on"]),
    ("panel asleep", ["lcd sleep on"]),
    ("low power set", ["lcd bl 30", "lcd keys 0", "lcd fps 1"]),
    ("baseline again", []),
]


def measure(d, seconds):
    rows, last, t0 = [], None, time.monotonic()
    while time.monotonic() - t0 < seconds:
        d.poll(0.1)
        t = d.state.get("t")
        if t is None or time.monotonic() - t > 1.0:
            d.get(T_ALL)
        if t is not None and t != last:
            last = t
            rows.append((d.state["v"], d.state["i"]))
    if not rows:
        sys.exit("no readings from the supply")
    amps = [1000 * r[1] for r in rows]
    return {"n": len(rows), "volts": statistics.mean(r[0] for r in rows), "mean_ma": statistics.mean(amps),
            "median_ma": statistics.median(amps), "min_ma": min(amps), "max_ma": max(amps)}


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("radio")
    ap.add_argument("supply")
    ap.add_argument("--label", default="run")
    ap.add_argument("--seconds", type=float, default=60)
    ap.add_argument("--settle", type=float, default=10)
    ap.add_argument("--only", help="comma-separated state names")
    a = ap.parse_args()
    states = [s for s in STATES if not a.only or s[0] in a.only.split(",")]
    out = Path(__file__).resolve().parents[1] / "target" / "power" / f"{a.label}.csv"
    out.parent.mkdir(parents=True, exist_ok=True)
    new = not out.exists()
    results = []
    with Radio(a.radio) as r, Dps150(a.supply) as d, open(out, "a", newline="") as f:
        w = csv.writer(f)
        if new:
            w.writerow(["label", "state", "time", "samples", "volts", "mean_ma", "median_ma", "min_ma", "max_ma"])
        if "lcd:" not in (r.cmd("lcd") or ""):
            sys.exit("the radio has no lcd command: flash a bench build (tools/mkbtf.sh tx-bench)")
        try:
            for name, lines in states:
                r.cmd("lcd reset")
                for line in lines:
                    r.cmd(line)
                print(f"{name}: settling {a.settle:.0f} s, then {a.seconds:.0f} s", flush=True)
                time.sleep(a.settle)
                d.poll()
                m = measure(d, a.seconds)
                results.append((name, m))
                w.writerow([a.label, name, time.strftime("%Y-%m-%d %H:%M:%S"), m["n"], f"{m['volts']:.3f}",
                            *(f"{m[k]:.1f}" for k in ("mean_ma", "median_ma", "min_ma", "max_ma"))])
                f.flush()
                print(f"  {m['mean_ma']:.1f} mA mean, {m['median_ma']:.1f} median ({m['min_ma']:.0f}-{m['max_ma']:.0f})")
        finally:
            r.cmd("lcd reset")
    base = results[0][1]["mean_ma"] if results and results[0][0] == "baseline" else None
    print(f"\n{a.label}: {results[0][1]['volts']:.2f} V" if results else "")
    print(f"{'state':16} {'mean mA':>8} {'median':>8} {'vs base':>8}")
    for name, m in results:
        diff = f"{m['mean_ma'] - base:+8.1f}" if base is not None else ""
        print(f"{name:16} {m['mean_ma']:8.1f} {m['median_ma']:8.1f} {diff}")
    print(f"(appended to {out})")


if __name__ == "__main__":
    main()
