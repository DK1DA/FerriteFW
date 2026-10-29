"""steps.py PORT TONE.wav|- PLAN: one Bluetooth session; PLAN's 'pre' lines before the key-up, then key (remote PTT),
the tone from 1 s in, and for each step its lines then `af` READS times; unkey; PLAN's 'post' lines. Prints, per step,
the PA2 swing ((max-min)/2 counts) and the crossings' frequency."""
import sys, time, subprocess, re, json
sys.path.insert(0, "/Users/infamy/Code/rt950pro/ferrite/tools")
import ferrite_link as fl
port, wav, plan = sys.argv[1], sys.argv[2], json.load(open(sys.argv[3]))
TC = float(sys.argv[4]) if len(sys.argv) > 4 else time.time()
P = fl.Port(port); link = fl.Link(P); link.hello(); fl.ble_fast(link)
def term(line, tries=3):
    for k in range(tries):
        try:
            return term1(line)
        except TimeoutError:
            time.sleep(0.5); link.answers()
    return ""
KEYED = [False]
def alive():
    if KEYED[0]: link.send(fl.KEY, bytes([fl.PTT, fl.PTT_ALIVE]))
def term1(line):
    alive()
    s, rest = link.ask(fl.CMD, line.encode()); out = b""
    while s == 0:
        alive()
        out += rest[1:]
        if rest[0] == 0: break
        if len(rest) == 1: time.sleep(0.05)
        s, rest = link.ask(fl.MORE)
    return out.decode("utf-8", "replace").strip()
def say(x): print(x, flush=True)
for c in plan.get("pre", []): say("pre: " + term(c).splitlines()[0][:110])
link.send(fl.KEY, bytes([fl.PTT, fl.PTT_ON, fl.SLOTS["active"]])); KEYED[0] = True; t0 = time.time(); time.sleep(0.6); link.answers()
player = None
try:
    for c in plan.get("keyed", []): say("keyed: " + term(c).splitlines()[0][:110])
    if wav != "-": player = subprocess.Popen(["afplay", wav]); time.sleep(1.0)
    for name, lines in plan["steps"]:
        for c in lines:
            if c.startswith("add "):  # add CHIP REG N: read, add N, write
                _, chip, reg, d = c.split(); cur = int(term(f"bk {chip} {reg}").split()[3], 16)
                c = f"bk {chip} {reg} {(cur + int(d)) & 0xFFFF:04X}"
            if c.startswith("set ") or c.startswith("clr "):  # set|clr CHIP REG MASK: read-modify-write
                op, chip, reg, mask = c.split(); cur = int(term(f"bk {chip} {reg}").split()[3], 16); m = int(mask, 16)
                c = f"bk {chip} {reg} {(cur | m) if op == 'set' else (cur & ~m):04X}"
            r = term(c)
            say(f"         {c} -> {r[:230]}")
            if "refused" in r or "<-" not in r and c.split()[0] == "bk" and len(c.split()) == 4: say("  ! " + r[:120])
        time.sleep(0.3)
        sw, hz = [], []
        if plan.get("show"):
            outs = []
            for c in plan["show"]:
                r = term(c).replace("\n", " / ")
                outs.append(r[:int(plan.get("width", 70))])
            say(f"{time.time() - t0:5.1f} s  [{time.time() - TC:5.1f} cap]  {name:16s} " + "  ||  ".join(outs))
            continue
        if plan.get("read64"):
            v = [term(f"bk {plan['read64']} 64").split()[3] for _ in range(3)]
            say(f"{time.time() - t0:5.1f} s  {name:28s} REG_64 {' '.join(v)}  [{time.time() - TC:5.1f} cap]")
            continue
        for _ in range(plan.get("reads", 2)):
            link.send(fl.KEY, bytes([fl.PTT, fl.PTT_ALIVE])); link.answers()
            a = term("af"); m = re.search(r"min (\d+), max (\d+)", a); h = re.search(r"(\d+) Hz if", a)
            if m and h:
                sw.append((int(m.group(2)) - int(m.group(1))) / 2); hz.append(int(h.group(1)))
        k30 = (term("bk 1 30").split() + ["?"] * 4)[3]
        say(f"{time.time() - t0:5.1f} s  [{time.time() - TC:5.1f} cap]  chip1 {k30}  {name:28s} swing {' '.join('%6.1f' % v for v in sw)}  Hz {' '.join('%5d' % v for v in hz)}")
finally:
    KEYED[0] = False
    link.send(fl.KEY, bytes([fl.PTT, fl.PTT_OFF])); time.sleep(0.5); link.answers()
    if player: player.kill()
    for c in plan.get("post", []): term(c)
    P.close()
say("unkeyed, restored")
