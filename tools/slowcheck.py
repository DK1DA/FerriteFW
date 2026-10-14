#!/usr/bin/env python3
"""Check the fast/slow flash split of a firmware ELF (firmware/memory.x).

    python3 tools/slowcheck.py target/thumbv7em-none-eabihf/release/ferrite

Prints the region sizes, the fast functions that can call into .slow_text (directly or through other fast
functions: only Reset/main, the thread executor and the UI task should), and whether any interrupt handler in the
vector table can reach .slow_text by direct calls. Function pointers into .slow_text stored in fast code or data
are listed too (vtables, task pointers). Needs arm-none-eabi-objdump / readelf / nm on the PATH.
Exit status 1 if an interrupt handler reaches slow code.
"""
import bisect, re, struct, subprocess, sys

def run(*a):
    return subprocess.run(a, capture_output=True, text=True, check=True).stdout

def main(elf):
    sec = {}
    for l in run("arm-none-eabi-readelf", "-SW", elf).splitlines():
        m = re.search(r"\]\s+(\S+)\s+\S+\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)", l)
        if m:
            sec[m.group(1)] = tuple(int(m.group(i), 16) for i in (2, 3, 4))
    lo, _, n = sec[".slow_text"]
    hi = lo + n
    fast_end = max(sec[s][0] + sec[s][2] for s in (".rodata", ".fast_ui"))
    gaps = sum(sec[s][2] for s in (".fast_gap1", ".fast_gap2") if s in sec)
    print(f"fast  0x08003000-0x{fast_end:08x}: {fast_end - 0x08003000} B (.text {sec['.text'][2]}, .rodata {sec['.rodata'][2]}, "
          f".fast_ui {sec['.fast_ui'][2]}, by the tags {gaps})")
    budget = next((int(l.split()[0], 16) for l in run("arm-none-eabi-nm", elf).splitlines() if l.endswith(" __fast_budget")), None)
    if budget is not None:
        print(f"fast budget {budget} B: {fast_end - 0x08003000} used, {budget - (fast_end - 0x08003000)} left")
    print(f"slow  .slow_rodata {sec['.slow_rodata'][2]} B, .slow_text {n} B")

    fast_secs = [a for s in (".fast_gap1", ".fast_gap2", ".fast_ui") if s in sec for a in ("-j", s)]
    dis = run("arm-none-eabi-objdump", "-d", "--no-show-raw-insn", "-j", ".text", *fast_secs, "-j", ".slow_text", elf)
    start, edges, cur = {}, {}, None
    for l in dis.splitlines():
        m = re.match(r"^([0-9a-f]+) <(.*)>:", l)
        if m:
            cur = m.group(2)
            start[cur] = int(m.group(1), 16)
            edges[cur] = set()
            continue
        m = re.search(r"\s(?:b\w*|cbn?z)(?:\.w|\.n)?\s+(?:\S+, )?[0-9a-f]+ <([^>+]*)", l)
        if m and cur and m.group(1) != cur:
            edges[cur].add(m.group(1))
    slow = {f for f, a in start.items() if lo <= a < hi}

    reach, changed = set(slow), True
    while changed:
        changed = False
        for f, es in edges.items():
            if f not in reach and es & reach:
                reach.add(f)
                changed = True
    callers = sorted(f for f in reach - slow if not f.startswith("OUTLINED_FUNCTION"))
    print("fast functions that can call into .slow_text:")
    for f in callers:
        print("   ", f)

    data = open(elf, "rb").read()
    syms = sorted((int(p[0], 16), p[3]) for p in (l.split() for l in run("arm-none-eabi-nm", "-S", "--defined-only", elf).splitlines()) if len(p) == 4)
    addrs = [a for a, _ in syms]
    owner = lambda a: syms[bisect.bisect_right(addrs, a) - 1][1]
    ptrs = set()
    for name in (".rodata", ".data", ".vector_table"):
        a, off, size = sec[name]
        for i in range(0, size - 3, 4):
            v = struct.unpack_from("<I", data, off + i)[0]
            if v & 1 and lo <= v & ~1 < hi:
                # Unnamed constants (jump tables) take the name of the symbol before them: show the target too.
                ptrs.add(f"{name} 0x{a + i:08x} (after {owner(a + i)[:60]}) -> {owner(v & ~1)[:80]}")
    cur = None
    for l in dis.splitlines():  # literal pools in fast code
        m = re.match(r"^([0-9a-f]+) <(.*)>:", l)
        if m:
            cur = m.group(2) if not lo <= int(m.group(1), 16) < hi else None
            continue
        m = re.search(r"\.word\s+0x([0-9a-f]+)", l)
        if cur and m and int(m.group(1), 16) & 1 and lo <= (int(m.group(1), 16) & ~1) < hi:
            ptrs.add(f"literal in {cur[:80]} -> {owner(int(m.group(1), 16) & ~1)[:80]}")
    print("fast data holding pointers into .slow_text:", "none" if not ptrs else "")
    for p in sorted(ptrs):
        print("   ", p)

    vt_addr, vt_off, vt_size = sec[".vector_table"]
    names = {a: f for f, a in start.items()}
    handlers = set()
    for i in range(8, vt_size, 4):  # skip the initial SP and Reset (thread mode)
        v = struct.unpack_from("<I", data, vt_off + i)[0] & ~1
        if v in names:
            handlers.add(names[v])
    seen, stack = set(handlers), list(handlers)
    while stack:
        for g in edges.get(stack.pop(), ()):
            if g not in seen:
                seen.add(g)
                stack.append(g)
    bad = seen & slow
    print(f"interrupt handlers: {len(handlers)}, functions they reach: {len(seen)}, of those in .slow_text: {len(bad)}")
    for f in sorted(bad):
        print("   ", f)
    return 1 if bad else 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "target/thumbv7em-none-eabihf/release/ferrite"))
