"""keyread.py PORT SECONDS [TONE.wav] [CMD ...]: one session: key (remote PTT), every 0.5 s run `bk 1 64` and `bk 1 30`
(plus any CMDs once at the start), play TONE.wav on this Mac from 2 s in, unkey."""
import sys, time, subprocess
sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent.parent))
import ferrite_link as fl
port, secs = sys.argv[1], float(sys.argv[2]); wav = sys.argv[3] if len(sys.argv) > 3 and sys.argv[3].endswith(".wav") else None
cmds = [c for c in sys.argv[3:] if not c.endswith(".wav") and not c.startswith("@")]
loop = [c[1:] for c in sys.argv[3:] if c.startswith("@")] or ["bk 1 64", "bk 1 30"]
P = fl.Port(port); link = fl.Link(P); link.hello(); fl.ble_fast(link)
def term(line):
    s, rest = link.ask(fl.CMD, line.encode()); out = b""
    while s == 0:
        out += rest[1:]
        if rest[0] == 0: break
        if len(rest) == 1: time.sleep(0.05)
        s, rest = link.ask(fl.MORE)
    return out.decode("utf-8", "replace").strip()
on = link.send(fl.KEY, bytes([fl.PTT, fl.PTT_ON, fl.SLOTS["active"]]))
time.sleep(0.6); link.answers()
for c in cmds: print("  " + term(c).replace("\n", " | "))
t0 = time.time(); player = None
try:
    while time.time() - t0 < secs:
        link.send(fl.KEY, bytes([fl.PTT, fl.PTT_ALIVE])); link.answers()
        if wav and player is None and time.time() - t0 > 2: player = subprocess.Popen(["afplay", wav])
        outs = [term(c).replace("\n", " / ") for c in loop]
        print(f"{time.time() - t0:5.1f} s{'  TONE' if player and player.poll() is None else '      '}  " + "  ||  ".join(o[:150] for o in outs), flush=True)
        time.sleep(0.3)
finally:
    link.send(fl.KEY, bytes([fl.PTT, fl.PTT_OFF])); time.sleep(0.5); link.answers()
    if player: player.kill()
print("unkeyed"); P.close()
