#!/usr/bin/env python3
"""Record a by-hand result into the test plan: one hand check (docs/test-plan/hand-checks.md) fills every entry that
names it with a `Hand check:` line, or a single rule its own entry.

    tools/record_hand.py PTT-BOOT ✓ "popup and log as expected"
    tools/record_hand.py PTT-BOOT ✓ "fine" SAFE-14.4=✗:"key test opened"     # one covered rule differs
    tools/record_hand.py TX-30.5 ◐ "carrier ended at 31 s"                    # one rule alone
    tools/record_hand.py PTT-BOOT ✓ --fw "tx-dry" --date 2026-10-14 --dry-run

The result is ✓, ◐ or ✗ (pass, partial, fail also work). Each row is `| date | firmware | mark | hand CHECK: note |`
in the entry's Results table, in date order (the placeholder row goes); the same row again is not added twice.
--fw defaults to the newest radio session under ../bench-results (its commit and build), --date to today. A
`hand` row is a row written by hand for tools/record_results.py: it never touches it. Then run tools/testplan.py.
"""
import argparse
import datetime
import json
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import record_results as rr  # noqa: E402
import testplan  # noqa: E402

MARKS = {"✓": "✓", "pass": "✓", "ok": "✓", "◐": "◐", "partial": "◐", "✗": "✗", "fail": "✗"}


def mark_of(s):
    m = MARKS.get(s.strip().lower())
    if not m:
        raise SystemExit(f"result {s!r}: use ✓, ◐ or ✗ (pass, partial, fail)")
    return m


def newest_fw(root):
    """The newest radio session's "commit (build)", or "?"."""
    try:
        runs = rr.find_results([], root, newest=False)
    except OSError:
        return "?"
    for _, _, f in reversed(runs):
        r = json.load(open(f))
        if rr.radio_run(r)[0]:
            return f"{r.get('commit') or '?'} ({r.get('build') or '?'})"
    return "?"


def covered(docs, name):
    """The rules a hand check covers (or [name] when it is a rule with an entry)."""
    entries = testplan.parse_sources(pathlib.Path(docs) / "test-plan")
    rules = [rid for rid, e in entries.items() if e.get("hand_check") == name]
    if rules:
        return sorted(rules, key=testplan.rule_key), entries
    if name in entries:
        return [name], entries
    known = testplan.parse_hand_checks(pathlib.Path(docs) / "test-plan" / testplan.HAND_CHECKS)
    raise SystemExit(f"{name}: no entry names it" + (" (a hand check nothing points at)" if name in known else
                                                     " (neither a hand check nor a rule with an entry)"))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("check", help="a hand check (PTT-BOOT) or a rule (TX-30.5)")
    ap.add_argument("result", help="✓ ◐ ✗ (or pass, partial, fail)")
    ap.add_argument("rest", nargs="*", help='the note, then RULE=MARK[:note] for a covered rule that differs')
    ap.add_argument("--fw", help="the firmware (default: the newest radio session's commit and build)")
    ap.add_argument("--date", help="YYYY-MM-DD (default today)")
    ap.add_argument("--docs", default=str(rr.DOCS), help="the docs checkout (default %(default)s)")
    ap.add_argument("--results", default=str(rr.RESULTS), help="the results root, for --fw's default")
    ap.add_argument("--dry-run", action="store_true", help="say what would change, write nothing")
    a = ap.parse_args()

    mark = mark_of(a.result)
    note, over = [], {}
    for x in a.rest:
        m = re.match(r"^([A-Z]+-\d+(?:\.\d+)?)=([^:]+)(?::(.*))?$", x)
        if m:
            over[m.group(1)] = (mark_of(m.group(2)), (m.group(3) or "").strip())
        else:
            note.append(x)
    note = " ".join(note).strip()
    rules, entries = covered(a.docs, a.check)
    bad = sorted(set(over) - set(rules))
    if bad:
        raise SystemExit(f"{' '.join(bad)}: not covered by {a.check} (it covers {' '.join(rules)})")
    day = datetime.date.fromisoformat(a.date) if a.date else datetime.date.today()
    date = day.strftime("%b %-d, %Y")
    fw = (a.fw or newest_fw(a.results)).replace("|", "\\|")
    stamp = day.strftime("%Y%m%d") + "-000000"
    rows, say = [], []
    for rid in rules:
        mk, why = over.get(rid, (mark, ""))
        text = f"hand {a.check}" + (f": {note}" if note else "") + (f"; {rid}: {why}" if why else "")
        text = re.sub(r"\s+", " ", text).replace("|", "\\|")
        row = f"| {date} | {fw} | {mk} | {text} |"
        have = ["| " + " | ".join(c) + " |" for c in entries[rid]["results"]]
        if row in have:
            say.append(f"  {rid}: already there")
            continue
        rows.append((rid, "hand", stamp, row))
        say.append(f"  {rid}: {row}")
    changed, same, missing = rr.apply(rows, a.docs, a.dry_run) if rows else ([], [], [])
    print(f"{a.check}: " + ("the rule alone" if rules == [a.check] else f"{len(rules)} rules"))
    print("\n".join(say))
    print(f"{'would record' if a.dry_run else 'recorded'} {len(changed)} rows into {a.docs}/test-plan"
          + (f"; no entry for {' '.join(missing)}" if missing else "")
          + ("" if a.dry_run or not changed else "; now run tools/testplan.py"))


if __name__ == "__main__":
    sys.exit(main())
