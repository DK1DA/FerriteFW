"""The manual's rules each bench.py check measures, and how a session's checks add up per rule: shared by
tools/bench.py (bench/results.json) and tools/record_results.py (which re-reads an older session's checks with these
tags, so a rule split since it ran, decision 160, still gets its rows).

A check is tagged with a rule only where it measures a number or outcome the rule states; "part" marks a rule the
check covers only in part (recorded ◐ on a pass), "all" one it covers as the rule states it (✓). Rules that mixed
several claims are split into sub-rules (TX-30.1, ...), so a check names the sub-rule it measures whole wherever one
exists. No numpy here: record_results.py imports it.
"""

# check name (before any "@run"): {rule: "all" | "part"}
RULE_TAGS = {
    # session extras: the refusals, the keepalive, the timeout, the reboot (LINK-20.2: every check, these three of it)
    "refusal_band_plan": {"TX-1.1": "all", "SAFE-12.1": "all", "LINK-20.2": "part"},
    "refusal_125m": {"TX-10.1": "all", "SAFE-12.2": "all", "LINK-20.2": "part"},
    "refusal_emergency": {"TX-35.1": "all", "LINK-20.9": "part"},  # LINK-20.9 also wants the toast
    "keepalive_cut": {"LINK-20.4": "all", "TX-47.3": "all", "SAFE-14.6": "all"},
    "tx_timeout": {"TX-30.3": "all", "SAFE-17.1": "all", "LINK-20.7": "part"},  # LINK-20.7: the timeout of its stops
    "reboot": {"LINK-24.1": "all"},
    # each suite run
    "suite": {"TERM-34": "all"},
    "data_flat": {"TERM-33.10": "all"},
    "end_tones": {"TONE-10.2": "all"},
    "tone_swell": {"TONE-10.4": "part"},  # the chip's swell; the lead-in that keeps it off the air isn't in the suite
    "afsk_level": {"APRS-14.2": "all"},
    "stock_tone_level": {"TONE-19.5": "all"},
    "subtones": {"TONE-1.1": "all", "TONE-1.2": "part"},  # TONE-1.2: one deviation; the byte isn't read back
    "dcs_polarity": {"TONE-20.1": "all"},
}


def tags(name):
    """{rule: scope} for a check named NAME or NAME@run (empty for one with no rule)."""
    return dict(RULE_TAGS.get(name.split("@")[0], {}))


def per_rule(tests):
    """{rule: result, test, why, scope, runs, note} from tagged checks (each: name, rules, scope, result, why,
    summary): a rule fails if any of its checks does; its scope is part if any check covers part of it."""
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
    return rules
