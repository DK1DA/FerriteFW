#!/usr/bin/env python3
"""The test plan: every numbered rule in docs/manual.md, the automated tests that cover it, and the manual
(radio or bench) steps for the rest.

    python3 tools/testplan.py              # writes ../docs/test-plan.md
    python3 tools/testplan.py --init       # adds a draft entry to docs/test-plan/*.md for every rule without one
    python3 tools/testplan.py --coverage   # short report; exit 1 if a rule has neither a test nor an entry, or a
                                           # test cites a rule the manual doesn't have

A rule is covered by an automated test when its ID (e.g. SCAN-21) appears in the test's /// doc comment, its
attributes, its name or a comment inside its body. Ranges such as "SCAN-16 – SCAN-19" count every rule in them.
Tests are #[test] functions in ferrite (crates, drivers, sim, sim-web, firmware) and the udigimodes crates; one
under sim/ or sim-web/ counts as a simulator test, the rest as host tests.

The hand-kept steps live in docs/test-plan/<chapter>.md, one entry per rule:

    ### TX-12
    Method: bench
    Equipment: dummy load, RF power meter
    Steps: ...
    Pass: ...
    Results:
    | Date | Firmware | Result | Notes |
    | --- | --- | --- | --- |
    | 2026-10-12 | 1a2b3c4 | pass | |

Method is one or more of host, sim, radio, bench, n/a (a moved or retired rule). --init never rewrites an entry
that exists; edit them by hand.
"""
import argparse, pathlib, re, sys

HERE = pathlib.Path(__file__).resolve().parent
FERRITE = HERE.parent
WORK = FERRITE.parent
DOCS = WORK / "docs"

RULE = re.compile(r"^> - \*\*([A-Z]+)-(\d+)\*\*\s*(.*)$")
CHAPTER = re.compile(r"^## (?:(\d+[a-z]?)\. |Appendix ([A-Z])\. )(.*)$")


def slug(s):
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")


# ---- the manual ----

def parse_manual(path):
    rules, chapters, status = [], [], {}
    chap = None
    section = ""
    for line in path.read_text().splitlines():
        m = CHAPTER.match(line)
        if m:
            num, app, title = m.groups()
            if num:
                key = f"{int(re.match(r'\d+', num).group()):02d}{num.lstrip('0123456789')}-{slug(title)}"
                label = f"{num}. {title}"
            else:
                key = f"appendix-{app.lower()}-{slug(title)}"
                label = f"Appendix {app}. {title}"
            chap = {"key": key, "label": label, "anchor": slug(line[3:])}
            chapters.append(chap)
            status[key] = []
            section = ""
            continue
        if line.startswith("## "):
            chap = None
            continue
        if chap is None:
            continue
        if line.startswith("### "):
            section = line[4:].strip()
            continue
        m = RULE.match(line)
        if m:
            p, n, text = m.groups()
            text = text.strip()
            retired = bool(re.match(r"\(?(Moved to|Retired|Dropped)", text, re.I))
            rules.append({"id": f"{p}-{n}", "prefix": p, "num": int(n), "text": text, "chap": chap["key"],
                          "retired": retired})
        elif section == "Status" and "bench" in line.lower() and line.startswith("- "):
            status[chap["key"]].append(line[2:].strip())
    return rules, chapters, status


# ---- the tests ----

def id_pattern(prefixes):
    alt = "|".join(sorted(prefixes, key=len, reverse=True))
    return re.compile(rf"\b({alt})-(\d+)(?:\s*(?:–|—|\.\.|to)\s*(?:({alt})-)?(\d+))?\b")


def ids_in(text, pat):
    out = set()
    for m in pat.finditer(text):
        p, a, p2, b = m.groups()
        a = int(a)
        if b is not None and (p2 is None or p2 == p) and int(b) >= a and int(b) - a <= 40:
            out.update(f"{p}-{k}" for k in range(a, int(b) + 1))
        else:
            out.add(f"{p}-{a}")
            if b is not None and p2:
                out.add(f"{p2}-{b}")
    return out


def scan_tests(roots, pat):
    tests = []
    for root in roots:
        if not root.exists():
            continue
        for f in sorted(root.rglob("*.rs")):
            if "target" in f.parts:
                continue
            lines = f.read_text(errors="replace").splitlines()
            for i, line in enumerate(lines):
                if not re.match(r"\s*#\[(?:test|tokio::test|async_test)\]", line):
                    continue
                # the doc comment and attributes above
                head = []
                j = i - 1
                while j >= 0 and re.match(r"\s*(///|#\[|//)", lines[j]):
                    head.append(lines[j])
                    j -= 1
                # the fn and its body, by braces
                k = i + 1
                while k < len(lines) and not re.search(r"\bfn\s+\w+", lines[k]):
                    head.append(lines[k])
                    k += 1
                if k >= len(lines):
                    continue
                name = re.search(r"\bfn\s+(\w+)", lines[k]).group(1)
                depth, body, started = 0, [], False
                for L in lines[k:]:
                    body.append(L)
                    depth += L.count("{") - L.count("}")
                    if "{" in L:
                        started = True
                    if started and depth <= 0:
                        break
                comments = "\n".join(head) + "\n" + "\n".join(
                    c for L in body for c in re.findall(r"//.*$", L))
                rel = f.relative_to(WORK)
                kind = "sim" if any(p in ("sim", "sim-web") for p in rel.parts[:2]) else "host"
                tests.append({"name": name, "file": str(rel), "line": k + 1, "kind": kind,
                              "ids": ids_in(comments + "\n" + name.upper().replace("_", "-"), pat)})
    return tests


# ---- the hand-kept entries ----

ENTRY = re.compile(r"^### ([A-Z]+-\d+)\s*$")


def parse_sources(d):
    entries = {}
    for f in sorted(d.glob("*.md")):
        cur = None
        in_results = False
        for line in f.read_text().splitlines():
            m = ENTRY.match(line)
            if m:
                cur = {"id": m.group(1), "file": f.name, "method": "", "results": []}
                entries[cur["id"]] = cur
                in_results = False
                continue
            if cur is None:
                continue
            if line.startswith("## ") or line.startswith("# "):
                cur = None
                continue
            m = re.match(r"^(Method|Equipment|Steps|Pass):\s*(.*)$", line)
            if m:
                cur[m.group(1).lower()] = m.group(2).strip()
                in_results = False
            elif line.startswith("Results:"):
                in_results = True
            elif in_results and line.startswith("|") and not re.match(r"^\|\s*(Date|---)", line):
                cells = [c.strip() for c in line.strip().strip("|").split("|")]
                if cells and cells[0]:
                    cur["results"].append(cells)
    return entries


# ---- drafts ----

BENCH = [
    (r"deviation", "service monitor, or an SDR with a deviation readout"),
    (r"desens|harmonic|spur|third", "signal generator, attenuators, dummy load, an SDR"),
    (r"\bmA\b|µA|current", "multimeter in series with the battery, or a bench supply with a current readout"),
    (r"\bW\b|watt|power level|dummy load|PA drive|DAC2|ramp", "dummy load and RF power meter"),
    (r"on a scope|oscilloscope|settle|edge|\b\d+ ?ms\b.*(key|unkey|relay)", "oscilloscope on the named pin"),
    (r"temperature|thermometer", "thermometer"),
    (r"sensitiv|dBm|\bdB\b", "signal generator and attenuators"),
    (r"battery|volt|\bV\b", "bench supply or multimeter"),
]
RADIO = re.compile(r"relay|\bPA\b|BK4829|SI4732|\bGPS\b|Bluetooth|BT513|battery|\bLED|backlight|keypad light|"
                   r"speaker|SPI flash|\bflash\b|bootloader|cable|USART|UART|\bADC|\bDAC|REG_|\bP[A-E]\d|"
                   r"\bmic\b|audio|antenna|\bPTT\b|current|power off|switch(es)? off|at boot|self-test|"
                   r"key test|side connector|charger|calibration|squelch|RSSI|carrier|on the air|transmit",
                   re.I)
BENCH_RE = re.compile(r"deviation|desens|harmonic|spur|\bmA\b|µA|dummy load|power meter|oscilloscope|"
                      r"on a scope|dBm|sensitiv|thermometer|ramp", re.I)
HW_PREFIX = {"SAFE", "PWR", "CAL", "BOOT", "SELF", "INST", "BACKUP", "UPD", "TX", "XB"}
REVIEW_RE = re.compile(r"\btask\b|\bcrate|compiled|\bcode\b|\bbuild\b|cargo feature|\bmodule\b|\bRAM\b|\bstack\b|"
                       r"interrupt|no_std|\bISR\b|\bowner\b|source of truth|library|libraries", re.I)
AIR_RE = re.compile(r"receiv|transmit|carrier|signal|on the air|squelch|repeater|tone|beacon|frame|packet|heard",
                    re.I)


def checks(text):
    """The rule's clauses as numbered checks (split on ';')."""
    body = re.sub(r"\s*\((?:decided|decision|settled)[^)]*\)", "", text, flags=re.I).strip().rstrip(".")
    parts, depth, cur = [], 0, ""
    for ch in body:
        depth += (ch in "([") - (ch in ")]")
        if ch == ";" and depth == 0:
            parts.append(cur.strip())
            cur = ""
        else:
            cur += ch
    parts = [p for p in parts + [cur.strip()] if p]
    if len(parts) == 1:
        return parts[0] + "."
    return " ".join(f"({i}) {p}." for i, p in enumerate(parts, 1))


def draft(rule, tests, bench_notes):
    rid, text = rule["id"], rule["text"]
    if rule["retired"]:
        return {"method": "n/a", "steps": "Nothing to test: the rule moved or was retired (see its text).",
                "pass": "n/a", "equipment": ""}
    auto = sorted({t["kind"] for t in tests})
    methods = list(auto)
    hw = bool(RADIO.search(text)) or rule["prefix"] in HW_PREFIX
    bench = bool(BENCH_RE.search(text))
    review = bool(REVIEW_RE.search(text)) or rule["prefix"] == "GOAL"
    if bench:
        methods.append("bench")
    elif review:
        methods.append("review")
    elif hw:
        methods.append("radio")
    elif not methods:
        methods.append("sim")
    equip = ""
    if "bench" in methods:
        for pat, what in BENCH:
            if re.search(pat, text, re.I):
                equip = what
                break
        equip = equip or "a second radio"
    elif "radio" in methods:
        equip = "the radio with Ferrite" + (", a second radio" if AIR_RE.search(text) else "")
    notes = [n for n in bench_notes if rid in ids_in(n, ID_PAT)]
    where = {"sim": "In the simulator (`cargo sim` or the web sim)", "radio": "On the radio",
             "bench": "On the bench", "review": "Code review"}
    steps = []
    if auto:
        steps.append("Automated: " + ", ".join(f"`{t['name']}`" for t in tests[:4])
                     + (f" (+{len(tests) - 4})" if len(tests) > 4 else "") + ".")
    for m in ("sim", "radio", "bench", "review"):
        if m in methods and m not in auto:
            if m == "review":
                steps.append("Code review: find where the code does this and confirm it holds.")
            else:
                watch = {"bench": "the screen, the cable log and the instruments", "radio": "the screen and the "
                         "cable log", "sim": "the screen"}[m]
                steps.append(f"{where[m]}: set up each case and watch {watch}.")
    for n in notes[:2]:
        steps.append("Chapter bench check: " + n.rstrip(".") + ".")
    if rule["prefix"] in {"KEY", "SAFE", "PWR"} and "radio" in methods:
        steps.append("See also `docs/hwtest-checklist.md`.")
    if rule["prefix"] in {"BOOT", "INST", "BACKUP", "SELF", "UPD"} and "radio" in methods:
        steps.append("See also `docs/bringup.md`.")
    steps.append("Check: " + checks(text))
    return {"method": ", ".join(methods), "steps": " ".join(steps),
            "pass": "Every check holds, and nothing the rule doesn't name changes.", "equipment": equip}


def write_entry(e, rid, manual_entry):
    out = [f"### {rid}", f"Method: {e['method']}"]
    if e["equipment"]:
        out.append(f"Equipment: {e['equipment']}")
    out += [f"Steps: {e['steps']}", f"Pass: {e['pass']}"]
    if manual_entry:
        out += ["Results:", "| Date | Firmware | Result | Notes |", "| --- | --- | --- | --- |", "| | | ☐ | |"]
    return "\n".join(out) + "\n"


# ---- output ----

def cell(s, n=110):
    s = s.replace("|", "\\|").replace("\n", " ")
    return s if len(s) <= n else s[: n - 1].rstrip() + "…"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--docs", type=pathlib.Path, default=DOCS)
    ap.add_argument("--init", action="store_true", help="add draft entries for rules without one")
    ap.add_argument("--coverage", action="store_true", help="short report; exit 1 on gaps or stale tags")
    a = ap.parse_args()

    rules, chapters, status = parse_manual(a.docs / "manual.md")
    by_id = {r["id"]: r for r in rules}
    seen, dups = set(), []
    for r in rules:
        if r["id"] in seen:
            dups.append(r["id"])
        seen.add(r["id"])
    global ID_PAT
    ID_PAT = id_pattern({r["prefix"] for r in rules})
    roots = [FERRITE / "crates", FERRITE / "sim", FERRITE / "sim-web", FERRITE / "firmware", FERRITE / "drivers",
             WORK / "udigimodes" / "crates"]
    tests = scan_tests(roots, ID_PAT)
    covered = {}
    for t in tests:
        for i in t["ids"]:
            covered.setdefault(i, []).append(t)
    stale = sorted({i for i in covered if i not in by_id}, key=lambda s: (s.split("-")[0], int(s.split("-")[1])))
    src = a.docs / "test-plan"
    src.mkdir(exist_ok=True)

    if a.init:
        entries = parse_sources(src)
        added = 0
        for ch in chapters:
            missing = [r for r in rules if r["chap"] == ch["key"] and r["id"] not in entries]
            if not missing:
                continue
            f = src / f"{ch['key']}.md"
            if not f.exists():
                notes = status.get(ch["key"], [])
                head = [f"# Test plan: {ch['label']}", "",
                        f"Entries for the rules of [manual chapter {ch['label']}](../manual.md#{ch['anchor']}). "
                        "Drafts were generated by `ferrite/tools/testplan.py --init` from the rule text; refine "
                        "the steps and pass criteria as the feature is tested. Record radio and bench results "
                        "in each entry's Results table (date, firmware commit, pass or fail, notes); "
                        "`docs/test-plan.md` is regenerated from these files.", ""]
                if notes:
                    head += ["Bench checks from the chapter's Status:", ""] + [f"- {n}" for n in notes] + [""]
                f.write_text("\n".join(head))
            with f.open("a") as fh:
                for r in missing:
                    e = draft(r, covered.get(r["id"], []), status.get(ch["key"], []))
                    manual = any(m in e["method"] for m in ("radio", "bench", "review"))
                    fh.write("\n" + write_entry(e, r["id"], manual))
                    added += 1
        print(f"added {added} draft entries to {src}")

    entries = parse_sources(src)
    live = [r for r in rules if not r["retired"]]
    auto = [r for r in live if r["id"] in covered]
    none = [r for r in rules if r["id"] not in covered and r["id"] not in entries]
    methods = {}
    for r in rules:
        for m in [x.strip() for x in entries.get(r["id"], {}).get("method", "").split(",") if x.strip()]:
            methods[m] = methods.get(m, 0) + 1
    pct = 100 * len(auto) / max(len(live), 1)

    if a.coverage:
        print(f"rules: {len(rules)} ({len(live)} live), automated: {len(auto)} ({pct:.0f}%), "
              f"tests scanned: {len(tests)}")
        print("by method (entries): " + ", ".join(f"{k} {v}" for k, v in sorted(methods.items())))
        if none:
            print(f"no test and no entry ({len(none)}): " + " ".join(r["id"] for r in none))
        if stale:
            print(f"tests cite rules the manual doesn't have ({len(stale)}): "
                  + " ".join(f"{i} ({covered[i][0]['file']}:{covered[i][0]['line']})" for i in stale))
        if dups:
            print(f"rule IDs used twice in the manual ({len(dups)}): " + " ".join(dups))
        sys.exit(1 if none or stale or dups else 0)

    out = ["# Ferrite test plan", "",
           "Generated by `ferrite/tools/testplan.py` from [the manual](manual.md)'s numbered rules, the tests in "
           "ferrite and udigimodes (a test covers a rule when the rule's ID is in its doc comment, name "
           "or body comments), and the hand-kept entries in [`test-plan/`](test-plan/). Don't edit this file: "
           "edit the entries, then run the tool.", "",
           "## Summary", "",
           f"- Rules: {len(rules)} ({len(live)} live, {len(rules) - len(live)} moved or retired).",
           f"- Covered by an automated test: {len(auto)} of {len(live)} live rules ({pct:.0f}%), "
           f"from {len(tests)} tests scanned.",
           "- Entries by method: " + ", ".join(f"{k} {v}" for k, v in sorted(methods.items())) + ".",
           f"- Rules with neither a test nor an entry: {len(none)}"
           + (": " + " ".join(r["id"] for r in none) if none else "") + ".",
           f"- Rule IDs used twice in the manual: {len(dups)}" + (": " + " ".join(dups) if dups else "") + ".",
           f"- Rules cited by tests that the manual doesn't have: {len(stale)}"
           + (": " + ", ".join(f"{i} (`{covered[i][0]['file']}:{covered[i][0]['line']}`)" for i in stale)
              if stale else "") + "."]
    done = sum(1 for e in entries.values() if e["results"])
    out += [f"- Entries with a recorded radio, bench or review result: {done}.", ""]
    for ch in chapters:
        rs = [r for r in rules if r["chap"] == ch["key"]]
        if not rs:
            continue
        n_auto = sum(1 for r in rs if r["id"] in covered)
        out += [f"## {ch['label']}", "",
                f"{len(rs)} rules, {n_auto} automated. Entries: [`test-plan/{ch['key']}.md`]"
                f"(test-plan/{ch['key']}.md).", "",
                "| Rule | What | Method | Automated by | Manual steps and pass | Result |",
                "| --- | --- | --- | --- | --- | --- |"]
        for r in rs:
            e = entries.get(r["id"], {})
            ts = covered.get(r["id"], [])
            by = ", ".join(f"`{t['name']}`" for t in ts[:3]) + (f" +{len(ts) - 3}" if len(ts) > 3 else "")
            steps = e.get("steps", "")
            if steps.startswith("Automated:"):
                steps = ""
            link = f"[steps](test-plan/{e['file']}#{r['id'].lower()})" if e and steps else ""
            res = e.get("results", [])
            if res:
                last = res[-1]
                result = f"{last[2] if len(last) > 2 else ''} {last[0]}".strip()
            elif any(m in e.get("method", "") for m in ("radio", "bench", "review")):
                result = "☐"
            elif ts:
                result = "auto"
            else:
                result = ""
            what = cell(r["text"])
            out.append(f"| [{r["id"]}](manual.html#{r["id"]}) | {what} | {e.get('method', '')} | {by} | {link} | {result} |")
        out.append("")
    (a.docs / "test-plan.md").write_text("\n".join(out))
    print(f"wrote {a.docs / 'test-plan.md'}: {len(rules)} rules, {len(auto)} automated ({pct:.0f}%), "
          f"{len(none)} without test or entry, {len(stale)} stale tags")


ID_PAT = None

if __name__ == "__main__":
    main()
