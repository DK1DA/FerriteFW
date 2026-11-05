"""How much flag lead (TXDELAY) a receiving radio needs before an APRS frame decodes (PWR-26.9.1, APRS-56): one radio
in KISS mode on the cable sends (Dry TX on), the other listens on the same APRS frequency, read over Bluetooth after
each frame: its `decode` counters (decoded, start lost, missed), its last capture start delay, each stage's
count (APRS-56.4: squelch opens, short opens, spikes and settling ignored, captures, data heard, frames) and its
newest log row.

    aprslead.py KISS_PORT RX_PORT    # either may be ble:ID [--delays 300,200,150,100,70,50,30,10] [--reps 4] [--call VE7GPL-7] [--gap 4] [--rest 0]

KISS sets TX delay itself (command 1, 10 ms units) and persistence 255, so each frame goes out the moment the
channel reads clear. The sender's KISS port is 115200 baud.
"""
import argparse, os, re, subprocess, sys, time
import serial

LINK = [sys.executable, __file__.replace("aprslead.py", "ferrite_link.py")]
FEND, FESC, TFEND, TFESC = 0xC0, 0xDB, 0xDC, 0xDD


def addr(call, last):
    c, _, s = call.partition("-")
    return bytes(ord(x) << 1 for x in c.upper().ljust(6)[:6]) + bytes([0x60 | (int(s or 0) << 1) | (1 if last else 0)])


def kiss(cmd, data):
    out = bytearray([FEND, cmd])
    for b in data:
        out += bytes([FESC, TFEND]) if b == FEND else bytes([FESC, TFESC]) if b == FESC else bytes([b])
    return bytes(out + bytes([FEND]))


STAGES = ("sq_opens", "sq_short", "sq_spikes", "sq_settle", "captures", "heard", "late", "frames", "noise", "missed")


def counters(port):
    ui = subprocess.run(LINK + [port, "ui"], capture_output=True, text=True).stdout
    out = {}
    for line, pre in (("decode", ""), ("aprs_sq", "sq_")):  # APRS-56.4's stages, SQL-19.4's opens
        m = re.search(rf"^{line} (.*)$", ui, re.M)
        if m:
            out.update({pre + k: int(v) for k, v in re.findall(r"(\w+)=(\d+)", m.group(1))})
    return out


def newest(port):
    out = subprocess.run(LINK + [port, "log", "1"], capture_output=True, text=True).stdout.strip().splitlines()
    out = [x for x in out if x.startswith("#")]
    return out[0] if out else ""


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("kiss"); ap.add_argument("rx")
    ap.add_argument("--delays", default="300,200,150,100,70,50,30,10")
    ap.add_argument("--reps", type=int, default=4)
    ap.add_argument("--call", default="VE7GPL-7")
    ap.add_argument("--gap", type=float, default=4.0, help="seconds after a frame before reading the receiver")
    ap.add_argument("--rest", type=float, default=0.0, help="seconds of quiet before each frame (the receiver's clock drops)")
    ap.add_argument("--batch", action="store_true", help="read the receiver only before and after: no link traffic between frames holds its clock up")
    a = ap.parse_args()
    ble = a.kiss.startswith("ble:")  # KISS on Bluetooth (KISS-5): the sender's FFE1, as the link tool opens it
    if ble:
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        from ferrite_link import BlePort
    else:
        k = serial.Serial(a.kiss, 115200)
        k.write(kiss(0x02, b"\xff") + kiss(0x03, b"\x00"))
    before = counters(a.rx)
    last_log = newest(a.rx)
    print("delay_ms rep  result        capture_ms  stages (+ since the frame before)")
    tally = {}
    for d in [int(x) for x in a.delays.split(",")]:
        for r in range(a.reps):
            time.sleep(a.rest)
            if ble:  # a fresh connection each frame: a dropped one would skip sends unseen
                k = BlePort(a.kiss[4:])
                k.flush = lambda: None
                k.write(kiss(0x02, b"\xff") + kiss(0x03, b"\x00"))
            k.write(kiss(0x01, bytes([max(0, min(255, d // 10))])))
            body = addr("APZFER", False) + addr(a.call, True) + b"\x03\xf0" + f">lead T{d} #{r}".encode()
            sent = time.time()  # wall clock, to line the frame up with an SDR recording
            k.write(kiss(0x00, body))
            k.flush()
            if ble:
                time.sleep(1)  # the paced writes out before the connection closes
                k.close()
            time.sleep(a.gap)
            if a.batch:
                continue
            now = counters(a.rx)
            dd = {x: now.get(x, 0) - before.get(x, 0) for x in ("frames", "noise", "missed")}
            res = "decoded" if dd["frames"] else "start lost" if dd["noise"] else "missed" if dd["missed"] else "nothing"
            log = newest(a.rx)
            stages = " ".join(f"{x}+{now[x] - before.get(x, 0)}" for x in STAGES if x in now)
            timeline = " ".join(f"{x}={now[x]}" for x in ("sq_lag_ms", "tick_ms", "first_data_ms", "clock_wait_ms", "clock_wait_max") if x in now)  # APRS-56.4
            print(f"{d:8} {r:3}  {res:12}  {now.get('capture_ms', -1):10}  {timeline}  {stages}  {log if log != last_log else ''}  t={sent:.2f}", flush=True)
            tally.setdefault(d, []).append(res)
            before, last_log = now, log
    if a.batch:
        now = counters(a.rx)
        sent = sum(a.reps for _ in a.delays.split(","))
        print("batch: " + " ".join(f"{x}+{now.get(x, 0) - before.get(x, 0)}" for x in STAGES if x in now) + f" of {sent} sent")
        print("timing: " + " ".join(f"{x}={now[x]}" for x in ("capture_ms", "capture_max", "sq_lag_ms", "tick_ms", "first_data_ms") if x in now))
        return
    print("\ndelay_ms  decoded/sent")
    for d, rs in tally.items():
        print(f"{d:8}  {rs.count('decoded')}/{len(rs)}  " + ", ".join(f"{x} {rs.count(x)}" for x in sorted(set(rs)) if x != "decoded"))


main()
