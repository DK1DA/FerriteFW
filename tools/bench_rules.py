"""The manual's rules each bench.py check measures, and how a session's checks add up per rule: shared by
tools/bench.py (bench/results.json) and tools/record_results.py (which re-reads an older session's checks with these
tags, so a rule split since it ran, decision 160, still gets its rows).

A check is tagged with a rule only where it measures a number or outcome the rule states; "part" marks a rule the
check covers only in part (recorded ◐ on a pass), "all" one it covers as the rule states it (✓). Rules that mixed
several claims are split into sub-rules (TX-30.1, ...), so a check names the sub-rule it measures whole wherever one
exists. A rule's result in a session is ✓ when a check covering all of it passed and none of its checks failed (a
part check passing beside it doesn't take that away), ◐ when only part checks covered it.

The sweeps (bench.py --sweep) name their checks NAME, or NAME~part when the sweep didn't cover every item it has (a
--count, a stop, or --from without --resume): a partial sweep covers each of its rules only in part.
No numpy here: record_results.py imports it.
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
    # the sweeps (bench.py --sweep): refusals-wide, ctcss, dcs, endtones, sublevel, afsk-level
    "refusal_wide_band_plan": {"TX-1.1": "all", "SAFE-12.1": "all", "LINK-20.2": "part"},
    "refusal_wide_125m": {"TX-10.1": "all", "SAFE-12.2": "all", "LINK-20.2": "part"},
    "refusal_wide_emergency": {"TX-35.1": "all", "LINK-20.9": "part"},  # LINK-20.9 also wants the toast
    "ctcss_freq": {"TONE-1.1": "all"},  # all 50 tones within 0.2 Hz (the entry's figure)
    "ctcss_level": {"TONE-1.2": "part"},  # one deviation for all 50; the calibration byte isn't read back
    "ctcss_typed": {"TONE-15": "part"},  # 60.0 and 260.0 Hz typed go out; the picker, card and lists aren't seen
    "dcs_normal": {"TONE-20.1": "all"},  # every code N decodes normal, never inverted
    "dcs_inverted": {"TONE-20.2": "all"},  # every code I decodes inverted, never normal
    "dcs_level": {"TONE-1.2": "part"},
    "dcs_typed": {"TONE-15": "part"},  # D000 and D777 typed go out
    "endtone_steps": {"TONE-10.2": "all", "TONE-10.1": "part"},  # on a real unkey; 10.1: on a VFO 0 8 5 4 picks it
    "endtone_level": {"TONE-19.4": "all", "TONE-10.3": "part"},  # 10.3: against TONE-19.4; voice peak not measured
    "endtone_lead_in": {"TONE-10.4": "all"},  # silent lead-in, then full level from the first step's start
    "endtone_hold": {"TONE-10.5": "all", "TX-47.1": "part", "LINK-20.6": "part"},  # 400 ms carrier after the tone
    "subtone_level": {"TONE-16": "part"},  # Low < Normal < High in even steps; the bytes aren't read back
    "afsk_level_steps": {"TERM-33.6": "part"},  # DAC AFSK at 10-150 %; afsk-bk's tone gain not swept
    # --suite tx-behaviour (tools/benchtests/tx_behaviour.py): around a key-up, by readback, terminal and SDR
    "txb_transmit_screen": {"LINK-20.12": "all", "TX-22.2": "part", "TX-12.2": "part"},  # the readback has no
    # frequency, power word, meter or tone line: those fields of TX-22.2, and TX-12.2's power name, aren't seen
    "txb_menu_does_nothing_keyed": {"MENU-30": "all"},
    "txb_red_led": {"TX-22.1": "all", "DISP-17.1": "all"},
    "txb_red_led_off_setting": {"DISP-17.3": "all"},
    "txb_backlight_on_transmit": {"DISP-26": "all"},
    "txb_wake_on_transmit": {"DISP-15.4": "all"},
    "txb_tx_bandwidth": {"TX-45": "all"},
    "txb_cable_while_keyed": {"TX-43.2": "all", "LINK-12": "part"},  # LINK-12: queuing is the tx build's
    "txb_gps_query_keyed": {"TERM-36.8": "all"},
    "txb_ptt_from_screens": {"KEY-7.3": "all", "TX-18": "part"},  # TX-18: PTT B and the side connector not keyed
    "txb_ptt_slots": {"KEY-7.5": "all", "LINK-20.1": "all"},
    "txb_reboot_refused_keyed": {"LINK-24.3": "all"},
    "txb_session_end_unkeys": {"LINK-20.5": "part"},  # the other port's refusal needs Bluetooth
    "txb_key_off_end_tone": {"LINK-20.6": "all"},
    "txb_txtest_armed_and_used": {"TERM-33.1": "part"},  # off after a restart not run
    "txb_txtest_filters": {"TERM-33.9": "all", "TERM-33.12": "all"},
    "txb_txtest_until_release": {"TERM-33.7": "part", "LINK-20.13": "all"},  # 4 s key-ups, not 10 s (duty)
    "txb_refusals_hf": {"TX-4": "part"},  # 28 MHz's allowed side not keyed (not an approved frequency)
    "txb_refusal_640": {"TX-5": "all"},
    "txb_refusal_band_edge": {"TX-6": "part"},  # the 144 MHz edge, simplex and shifted; reverse and trims not
    "txb_refusal_125m_extend": {"SAFE-12.4": "all"},
    "txb_refusal_emergency_toast": {"LINK-20.9": "all"},
    "txb_relay_still": {"WATCH-12": "part"},  # (2) only: no relay change while keyed or in the unkey wait
    "txb_battery_save_pins": {"TX-29": "part"},  # the pins while keyed; the chips' sleep and the unkey wait not
    "txb_rx_both_kept": {"SAFE-24": "part"},  # the hold kept across a key-up, refused on HF; not both watched
    "txb_broadcast_muted_keyed": {"TX-24": "part"},  # the state while keyed, not its order before the carrier
    "txb_trim_follows": {"CAL-13": "part"},  # the chip and the carrier follow the trim; the check's use not probed
}


def tags(name):
    """{rule: scope} for a check named NAME, NAME@run or NAME~part[@run] (empty for one with no rule); ~part (a
    partial sweep) makes every rule part."""
    base = name.split("@")[0]
    if base.endswith("~part"):
        return {rid: "part" for rid in RULE_TAGS.get(base[: -len("~part")], {})}
    return dict(RULE_TAGS.get(base, {}))


def per_rule(tests):
    """{rule: result, test, why, scope, runs, note} from tagged checks (each: name, rules, scope, result, why,
    summary): a rule fails if any of its checks does; its scope is all if any check covers all of it, else part."""
    rules = {}
    for t in tests:
        for rid in t["rules"]:
            x = rules.setdefault(rid, dict(result="pass", test=[], why="", scope="part", notes=[]))
            x["test"].append(t["name"])
            if t["result"] != "pass":
                x["result"] = "fail"
                x["why"] = x["why"] or (f"{t['name'].split('@')[1]}: " if "@" in t["name"] else "") + t["why"]
            if t["scope"][rid] == "all":
                x["scope"] = "all"
            x["notes"].append(t["summary"])
    for x in rules.values():
        names = []
        for n in x["test"]:
            base = n.split("@")[0].replace("~part", "")
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
