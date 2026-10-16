#!/usr/bin/env python3
"""Record on-radio results into the test plan: each rule a uitest or bench run checked gets a row in its entry's
Results table in docs/test-plan/*.md (date, firmware = commit + build, ✓/◐/✗, a short note).

    tools/record_results.py                       # every session under ../bench-results, oldest first
    tools/record_results.py --newest              # the newest radio session with UI results and the newest with bench
    tools/record_results.py SESSION_DIR|results.json ...    # these
    tools/record_results.py --dry-run             # say what would change, write nothing

Reads <session>/ui/results.json (tools/uitest.py), <session>/tworadio/results.json (tools/tworadio.py: the same
shape, a rule a test checks only part of marked "part") and <session>/bench/results.json (tools/bench.py: each check
tagged with the rules it measures; `bench.py --rule-results SESSION` writes it for an older session). Only radio
runs are recorded: a simulator or selftest run (sim true, a tcp: port) and a re-analysis of saved captures (bench
--load) are skipped, so a simulator pass never lands as a radio pass.

The rows: uitest pass ✓, fail or error ✗, a known deviation (xfail) ◐, skipped not recorded; tworadio the same,
with a pass of a test that checks only part of the rule ◐ ("covers part of the rule"); bench pass ✓ where
the check covers the rule as stated and ◐ where it covers part of it (the note says so; ✓ when the entry's last
result before it is a ✓, which a part check can confirm but not take away), fail ✗. An automated
row's note starts with its tool ("uitest NAME", "tworadio NAME", "bench NAME") and ends "; run YYYYMMDD-HHMMSS". Per rule and tool
only the newest automated row is kept: a newer run replaces the older automated rows (the same run again
rewrites its row only if the row changed), an older run changes nothing. Rows written by hand (any other note) are never touched. Rows stay in date order, so
the entry's last row is its latest result. A rule's How doesn't matter: a host-covered rule gets its radio
confirmation the same way. Rules without an entry are reported.

Split rules (decision 160): a rule split into sub-rules (TX-30 -> TX-30.1, ...) gets no rows itself. A run's rules
are read from its tests with today's tags, so an older run lands on the sub-rules: a uitest by the rules its test is
tagged with now (tools/uitests; a test that was itself split into narrower tests is re-read check by check,
CHECK_SPLITS: which of its checks belong to which rule, and a rule none of whose checks ran gets no row), a bench
check by tools/bench_rules.py's RULE_TAGS (the table bench.py writes with). What can't be re-pointed is reported,
never recorded on the parent.
"""
import argparse
import datetime
import json
import os
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import bench_rules  # noqa: E402
import testplan  # noqa: E402
ROOT = HERE.parent.parent
RESULTS = ROOT / "bench-results"
DOCS = ROOT / "docs"

ENTRY = re.compile(r"^### ([A-Z]+-\d+(?:\.\d+)?)\s*$")

# uitests split into narrower ones after they ran: the old test's checks (by the start of their "what") per rule.
CHECK_SPLITS = {
    ("uitest", "relay_cancels_watch_and_says_so"): {  # now relay_cancels_watch, relay_cancel_toast, _status_row
        "WATCH-11.1": ["watch on: TW", "A tuned to 28.4 MHz", "cancelled: no TW or DW", "the other cards say relay",
                       "back on 2 m: watch resumes", "B watched again"],
        "WATCH-11.2": ["a toast: Watch off"],
        "WATCH-11.3": ["Status's Watch row"],
        "SBAR-17": ["Status's Watch row"],
    },
}
UI_ORDER = {"fail": 4, "error": 4, "xfail": 3, "xpass": 2, "pass": 2, "skip": 1}
AUTO = re.compile(r"^(uitest|tworadio|bench) \S")
RUN = re.compile(r"run (\d{8}-\d{6})")
PLACEHOLDER = re.compile(r"^\|\s*\|\s*\|\s*☐\s*\|\s*\|\s*$")
HEAD = ["| Date | Firmware | Result | Notes |", "| --- | --- | --- | --- |"]


def date_key(s):
    """A row's date as YYYYMMDD ("Oct 14, 2026", "2026-10-12"); unknown dates sort first."""
    s = s.strip()
    for fmt in ("%b %d, %Y", "%Y-%m-%d", "%B %d, %Y"):
        try:
            return datetime.datetime.strptime(s, fmt).strftime("%Y%m%d")
        except ValueError:
            pass
    return "00000000"


def cells(row):
    return [c.strip() for c in row.strip().strip("|").split("|")]


def auto_of(row):
    """(tool, run stamp) of an automated row, else None. A row from before the stamps counts as its day's start."""
    c = cells(row)
    if len(c) < 4 or not AUTO.match(c[3]):
        return None
    m = RUN.search(c[3])
    return c[3].split()[0], m.group(1) if m else date_key(c[0]) + "-000000"


# ---- the runs ----

def find_results(paths, root, newest):
    """[(stamp, kind, results.json path)], oldest first."""
    sess = []
    for p in paths or sorted(x for x in pathlib.Path(root).iterdir() if x.is_dir()):
        p = pathlib.Path(p)
        if p.is_file():
            sess.append(p)
            continue
        for kind in ("ui", "tworadio", "bench"):
            f = p / kind / "results.json"
            if f.exists():
                sess.append(f)
    out = []
    for f in sess:
        name = f.parent.parent.name
        m = re.match(r"(\d{8}-\d{6})", name)
        if not m:
            r = json.load(open(f))
            t = r.get("started") or ""
            stamp = re.sub(r"[-:]", "", t).replace("T", "-")[:15] if t else "00000000-000000"
        else:
            stamp = m.group(1)
        out.append((stamp, f.parent.name, f))
    out.sort()
    if newest and not paths:
        keep = {}
        for x in out:
            if radio_run(json.load(open(x[2])))[0]:
                keep[x[1]] = x
        out = sorted(keep.values())
    return out


def radio_run(r):
    """(True, "") for a radio run, else (False, why not)."""
    port = str(r.get("port") or "")
    if r.get("sim") or port.startswith("tcp:") or port == "fake":
        return False, "simulator or selftest"
    if r.get("tool") == "tworadio.py" and r.get("stopped"):
        return False, f"stopped early ({r['stopped'][:80]})"
    if r.get("tool") == "bench.py" and r.get("source") != "on air":
        return False, "not on air (a re-analysis)"
    return True, ""


def split_rules(docs):
    """{split parent: [its sub-rules]} from the manual."""
    rules, _, _ = testplan.parse_manual(pathlib.Path(docs) / "manual.md")
    return testplan.children(rules)


def ui_tags(d=None):
    """{uitest name: [the rules it is tagged with now]} (tools/uitests, or another directory of tests)."""
    out = {}
    for rid, tests in (testplan.scan_uitests(d) if d else testplan.scan_uitests()).items():
        for name, _ in tests:
            out.setdefault(name.split(".")[-1], []).append(rid)
    return out


def tworadio_tags():
    """{two-radio test name: [the rules it is tagged with now]} (tools/tworadio_tests)."""
    return ui_tags(HERE / "tworadio_tests")


def tworadio_rules(r, kids, tags, lost):
    """A tworadio run's {rule: result, test, why, scope}: ui_rules' worst result per rule, re-pointed to today's
    tags; scope "part" when no test that ran checks the whole rule (each test's `part` list)."""
    rules = ui_rules(r, kids, tags, lost)
    tests = r.get("tests") or []
    for rid, x in rules.items():
        if tests:
            full = any(rid in tags.get(t["name"], t.get("rules", [])) and rid not in t.get("part", [])
                       and t.get("result") != "skip" for t in tests)
        else:
            full = (r.get("rules", {}).get(rid) or {}).get("scope") != "part"
        x["scope"] = "all" if full else "part"
    return rules


def ui_rules(r, kids, tags, lost):
    """A uitest run's {rule: result, test, why}, rebuilt from its tests as uitest.py does (the worst result per
    rule, the first test on a tie), with split parents re-pointed to their sub-rules (`lost` gets what can't be)."""
    if not r.get("tests"):
        return {rid: x for rid, x in r.get("rules", {}).items() if rid not in kids or lost.append((rid, x["test"]))}
    rules = {}

    def put(rid, res, name, why):
        prev = rules.get(rid)
        if prev is None or UI_ORDER.get(res, 4) > UI_ORDER.get(prev["result"], 4):
            rules[rid] = dict(result=res, test=name, why=why)

    for t in r["tests"]:
        name, res, why = t["name"], t["result"], t.get("why", "")
        if res == "error":
            continue  # the test or a tool broke before it judged the radio (e.g. rtl_sdr): not a radio result
        split = CHECK_SPLITS.get(("uitest", name))
        if split:
            for rid, starts in split.items():
                cs = [c for c in t.get("checks", []) if any(c["what"].startswith(w) for w in starts)]
                if not cs:
                    continue  # the run never reached its checks
                bad = [c for c in cs if not c["ok"]]
                if not bad:
                    put(rid, "pass", name, "")
                else:
                    w = bad[0]["what"]
                    put(rid, res if res in ("fail", "error", "xfail") else "fail", name,
                        why if w in why else f"{w}: got {bad[0].get('got')}")
            continue
        for rid in t.get("rules", []):
            if rid not in kids:
                put(rid, res, name, why)
                continue
            subs = [x for x in tags.get(name, []) if x in kids[rid]]
            if not subs:
                lost.append((rid, name))
            for x in subs:
                put(x, res, name, why)
    return rules


def bench_rules_of(r, kids, lost):
    """A bench run's {rule: result, test, why, scope, note}, its checks re-tagged with today's RULE_TAGS (a check the
    table doesn't know keeps its own tags; one of those naming a split parent goes to `lost`)."""
    if not r.get("tests"):
        return {rid: x for rid, x in r.get("rules", {}).items()
                if rid not in kids or lost.append((rid, x.get("test", "")))}
    tests = []
    for t in r["tests"]:
        scope = bench_rules.tags(t["name"]) or {rid: s for rid, s in (t.get("scope") or {}).items()
                                                if rid not in kids or lost.append((rid, t["name"].split("@")[0]))}
        tests.append(dict(t, rules=list(scope), scope=scope))
    return bench_rules.per_rule(tests)


def rows_of(r, stamp, kids=None, tags=None, lost=None):
    """[(rule, tool, stamp, row text)] for one results.json."""
    date = datetime.datetime.strptime(stamp[:8], "%Y%m%d").strftime("%b %-d, %Y")
    fw = f"{r.get('commit') or '?'} ({r.get('build') or '?'})"
    tool = {"uitest.py": "uitest", "tworadio.py": "tworadio"}.get(r.get("tool"), "bench")
    kids, lost = kids or {}, lost if lost is not None else []
    if tool == "uitest":
        rules = ui_rules(r, kids, tags if tags is not None else ui_tags(), lost)
    elif tool == "tworadio":
        rules = tworadio_rules(r, kids, tags if tags is not None else tworadio_tags(), lost)
    else:
        rules = bench_rules_of(r, kids, lost)
    out = []
    for rid, x in sorted(rules.items()):
        res = x.get("result")
        if res == "skip":
            continue
        why = (x.get("why") or "").strip()
        if tool == "uitest":
            mark = {"pass": "✓", "xpass": "✓", "xfail": "◐"}.get(res, "✗")
            note = f"uitest {x['test']}" + (f": known deviation: {why}" if res == "xfail" else
                                            f": {why}" if res not in ("pass", "xpass") and why else "")
        elif tool == "tworadio":
            part = x.get("scope") == "part"
            mark = {"pass": "◐" if part else "✓", "xpass": "✓", "xfail": "◐"}.get(res, "✗")
            note = f"tworadio {x['test']}" + (f": {why}" if res not in ("pass", "xpass") and why else "") + (
                " (covers part of the rule)" if part and res == "pass" else "")
        else:
            part = x.get("scope") == "part"
            mark = "✗" if res != "pass" else ("◐" if part else "✓")
            note = f"bench {x['test']}: " + (why if res != "pass" else x.get("note", "")) + (
                " (covers part of the rule)" if part and res == "pass" else "")
        note = re.sub(r"\s+", " ", note).replace("|", "\\|")
        if len(note) > 300:
            note = note[:299].rstrip() + "…"
        out.append((rid, tool, stamp, f"| {date} | {fw} | {mark} | {note}; run {stamp} |"))
    return out


# ---- the test plan ----

def apply(rows, docs, dry=False):
    """rows: [(rule, tool, stamp, row)]. Returns (changed rules, unchanged rules, rules without an entry)."""
    want = {}
    for rid, tool, stamp, row in rows:  # per rule and tool the newest
        k = (rid, tool)
        if k not in want or stamp >= want[k][0]:
            want[k] = (stamp, row)
    by_rule = {}
    for (rid, tool), v in want.items():
        by_rule.setdefault(rid, {})[tool] = v
    changed, same, seen = set(), set(), set()
    for f in sorted((pathlib.Path(docs) / "test-plan").glob("*.md")):
        lines = f.read_text().split("\n")
        out, i, dirty = [], 0, False
        while i < len(lines):
            m = ENTRY.match(lines[i])
            if not m or m.group(1) not in by_rule:
                out.append(lines[i])
                i += 1
                continue
            rid = m.group(1)
            seen.add(rid)
            j = i + 1
            while j < len(lines) and not lines[j].startswith("#"):
                j += 1
            body = lines[i:j]
            n = len(body)
            while body and not body[-1].strip():
                body.pop()
            tail = lines[i + len(body):j]
            new_body = entry_with(body, by_rule[rid])
            if new_body != body:
                changed.add(rid)
                dirty = True
            else:
                same.add(rid)
            out += new_body + (tail if tail else [""])
            i = i + n
        if dirty and not dry:
            f.write_text("\n".join(out))
    return sorted(changed), sorted(same), sorted(set(by_rule) - seen)


def entry_with(body, new):
    """The entry's lines with the new automated rows ({tool: (stamp, row)}) in place."""
    body = list(body)
    if not any(x.startswith("Results:") for x in body):
        body += ["Results:"] + HEAD
    r0 = next(k for k, x in enumerate(body) if x.startswith("Results:")) + 1
    r1 = r0
    while r1 < len(body) and body[r1].startswith("|"):
        r1 += 1
    table = [x for x in body[r0:r1] if not PLACEHOLDER.match(x)]
    head = [x for x in table if re.match(r"^\|\s*(Date|---)", x)] or HEAD
    rows = [x for x in table if x not in head]
    for tool, (stamp, row) in sorted(new.items()):
        older = [x for x in rows if (auto_of(x) or ("", ""))[0] == tool]
        if any(auto_of(x)[1] > stamp for x in older) or row in older:
            continue  # a newer run is already there, or this one as it is
        rows = [x for x in rows if x not in older]
        d = date_key(cells(row)[0])
        k = len(rows)
        while k > 0 and date_key(cells(rows[k - 1])[0]) > d:
            k -= 1
        m = re.search(r" \| ◐ \| (bench|tworadio) ", row)
        if m and "(covers part of the rule)" in row and k and cells(rows[k - 1])[2] == "✓":
            # a check of part of the rule passed after a full pass: it confirms, it doesn't take the ✓ away
            row = row.replace(m.group(0), f" | ✓ | {m.group(1)} ", 1).replace(
                "(covers part of the rule)", "(covers part of the rule; confirms the ✓ before)", 1)
        rows.insert(k, row)
    return body[:r0] + head + rows + body[r1:]


def record_paths(paths, docs=DOCS, root=RESULTS, newest=False, dry=False):
    """Record the given sessions or results.json files (all under root when none); returns report lines."""
    say = []
    rows = []
    kids, tags, tw_tags = split_rules(docs), ui_tags(), tworadio_tags()
    for stamp, kind, f in find_results(paths, root, newest):
        r = json.load(open(f))
        ok, why = radio_run(r)
        name = f.parent.parent.name
        if not ok:
            if paths:
                say.append(f"skipped {name} {kind}: {why}")
            continue
        lost = []
        rs = rows_of(r, stamp, kids, tw_tags if kind == "tworadio" else tags, lost)
        rows += rs
        say.append(f"{name} {kind}: {len(rs)} rules")
        for rid, test in sorted(set(lost)):
            say.append(f"  {rid} ({test}): a split rule, and the test names none of its sub-rules"
                       + (" (rewrite it: bench.py --rule-results)" if kind == "bench" else "") + "; not recorded")
    changed, same, missing = apply(rows, docs, dry)
    say.append(f"{'would record' if dry else 'recorded'} {len(changed)} rules into {docs}/test-plan"
               f" ({len(same)} already up to date)" + (f"; no entry for {' '.join(missing)}" if missing else ""))
    return say


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("paths", nargs="*", help="session directories or results.json files (default: all under --results)")
    ap.add_argument("--results", default=str(RESULTS), help="the results root (default %(default)s)")
    ap.add_argument("--docs", default=str(DOCS), help="the docs checkout (default %(default)s)")
    ap.add_argument("--newest", action="store_true", help="only the newest radio session of each kind (ui, bench)")
    ap.add_argument("--dry-run", action="store_true", help="report, write nothing")
    a = ap.parse_args()
    for line in record_paths(a.paths, a.docs, a.results, a.newest, a.dry_run):
        print(line)


if __name__ == "__main__":
    sys.exit(main())
