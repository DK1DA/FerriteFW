# Promo video claims and their sources

Every claim in `preview.story`, `compare.story` and `unique.story` (Oct 25, 2026), with where it comes from, so a block can be cut or defended. Stock claims come from our own analysis of V0.27/V0.29. nicFW claims come only from nicFW's public wiki, as recorded in `docs/research/nicfw-feature-comparison.md` (Oct 7, 2026), never from anything about nicFW's firmware itself.

## preview.story

| Block | Claim | Source |
| --- | --- | --- |
| Intro | Free firmware, written from scratch | licence decision 228; the clean-room rules |
| Three slots | Three slots, each with its own frequency, mode, tones and power | manual ch 6, ch 9 |
| Band scope | Hold 1, live; the knob moves the marker; strongest named | ch 21 (SCOPE rules) |
| A channel on one page | MENU 8 opens every field of a channel on one screen | ch 10 |
| Squelch you can see | the live signal against the opening level | ch 12 (SQL-12) |
| Crossband repeater | A to B and back; timeout; 30 minutes of activity | ch 19: XB-9, XB-16 |
| Messages | contacts, recent, heard and services as recipients; predictive text; acks and retries; stop or send again | ch 25 (APRS-6, APRS-13.1, APRS-13.2), decision 284; TEXT-12 |
| APRS on slot C | MENU 0 5 3; counts since power-on and in the last hour; the last station; C every other turn | APRS-13, APRS-17.2 (decision 282, option C), WATCH-17.2 (decision 279) |
| Event log | every decode and alert, newest first, kept through a restart; filter, open, Go; heard list back after a restart | INBOX-6, INBOX-6.1 (decision 281), INBOX-9, INBOX-10; APRS-4 (decision 282) |
| Shortwave | AM on LW, MW and SW on the SI4732, its own screen | ch 20a (HF mode). SSB is not claimed: not built (research/si4732-features.md). Note: the screen shows a "6 kHz" bandwidth chip that doesn't reach the chip yet (same review) |
| Ferrite Console | browser, cable or Bluetooth; channels, settings, APRS messages, heard list on a map; backup before install | Console README; decisions 218, 261, 285; Alex tried Console over Bluetooth on black Oct 24 |
| Outro | Free firmware, source public | licence memory; modest-tone rule |

## compare.story: against stock

| Block | Claim | Source |
| --- | --- | --- |
| A repeater that times out | Stock: no timeout in repeat mode, so a stuck carrier keeps it keyed | `analysis/crossband_repeat.md` §6 item 1 (0x08024CAE returns early in mode 2) [verified in the disassembly] |
| | Ferrite: a timeout per pass, a duty limit, 30 minutes of activity | XB-9, XB-10, XB-16 |
| A scope that keeps looking | Stock: a muted stepped sweep that stops on the first point above a threshold and plays it | `analysis/scope_stock.md` (top summary) |
| | Ferrite: live sweep, marker, strongest named, Go | ch 21 |
| Transmit, in a safer order | Stock writes the APC (PA drive) before the TX select; Ferrite sets and reads back the path before any drive, 4 ms ramp | `docs/research/tx-vs-stock.md` §1 Verdict; `analysis/tx-gotchas.md` item 11 |
| Calibration left alone | Stock falls back to built-in TX windows when calibration's are blank; Ferrite locks transmit; never writes calibration; checks at power-on | `docs/research/calibration-vs-stock.md` line 117; CAL-4, TX-14; CLAUDE.md rules |

## compare.story: beside nicFW (its wiki)

| Block | nicFW claim (wiki) | Ferrite claim | Source |
| --- | --- | --- | --- |
| Saved as you go | deferred writes, red icon, Shut Down needed | saved within 2 s, power-cut safe | comparison doc "Storage" row (Superior) |
| Key lock | LP-\*, only PTT works | hold #; monitor, squelch, volume still work; PTT lockable | "Key lock" row; ch 7 |
| GPS | lat/long/alt/sats/grid/speed/course/time | accuracy, fix age, sky plot, nearest waypoint; go-to dial with distance, ETA, arrival alarm | "GPS data" and "Navigation" rows; ch 28 |
| Smarter beacons | time and distance, ambiguity, Mic-E, Beacon NOW | timed or smart (paced by speed), Mic-E, beacon after voice | "Beaconing" row; APRS-24 (decision 272 dropped distance), APRS-52 (Mic-E, decision 275) |
| KISS in the background | a modal screen, cable or BT | runs in the background over Bluetooth or the cable | "KISS TNC" row; ch 26 |
| Programming in a browser | RMS (Windows) | Ferrite Console in Chrome or Edge, cable or Bluetooth, live view, backup first | "PC programming" row; Console |
| Where nicFW goes further | SSB/CW on HF, ~40 assignable key functions, editable band plan, DSB and AM transmit, its own power table | (credit) | rows "LW/MW/SW, SSB/CW, BFO", "User keys", "Editable band plan", "Modulation and bandwidth", "AM transmit", "Power levels" |

## unique.story ("Things we haven't seen in stock's menus or in nicFW's wiki")

| Block | Claim | Source | How sure it's unique |
| --- | --- | --- | --- |
| Idles at 19 mA | 24 MHz at rest, full speed on need; ~19 mA vs ~65 mA at full speed always | power bench Oct 22 (memory "power-strategy": 64.7 to 18.6 mA); user guide ch 8 | No stock or nicFW idle figure measured: the comparison is Ferrite against itself |
| APRS that isn't missed | C every other turn, never deaf more than 70 ms | WATCH-17.2, decision 279 | Not checked against nicFW (its APRS page wasn't read) |
| A decoder that holds its own | WA8LMF tracks 1 and 2: 1,011 and 1,007; Dire Wolf 1.8.1 defaults 1,005 and 982; ~1.7 KB RAM | `docs/firmware-design.md` line 753 (Oct 22); Dire Wolf figures as in `aprs.story` (udigimodes scoring) | A measured result, not a comparison with stock or nicFW |
| Messages you can stop | stop, send again, delete stops tries | APRS-13.1, 13.2, 13.4; decision 284 | Not checked against nicFW's APRS page or stock's message handling |
| Nothing lost at power-off | heard list and event log survive a restart; real times or "earlier" | decisions 281, 282 | nicFW's wiki heard list: "50, browse, set target, wipe" (no persistence stated) |
| FX.25 | sent and received; plain receivers still decode | APRS-44; aprs.story | Not in stock's menu list; nicFW's APRS page not read |
| Bluetooth ~12 times faster | home 3.4 s to ~0.3 s; 176 channels ~33 s to ~6 s | decisions 285, 286; `docs/research/ble-speed.md` (black, Oct 24) | Ferrite against its own earlier link |
| Built not to brick | calibration never written; full backup before install; back to stock from the same page | decision 261 (BACKUP-7, BACKUP-9.3); CLAUDE.md rules | |
| Squelch you can see | live signal against the opening level | SQL-12 | nicFW: "noise floor on the squelch screen" is Ferrite's own row; not a firm unique |
| Low Power Mode | one setting; on at 20 % by default, off on the charger at 80 %; status bar shows it | PWR-24 (Auto), user guide ch 8 | |

## Left out (no source, or not true today)

- SSB, USB, LSB and CW on HF: not built (the old preview claimed it; removed).
- Receive audio to the PC: built (decisions 288, 290) but not yet checked on a radio.
- SAME weather alerts: built on the BK4829 (ch 23) but not tried on a live alert.
- Anything about nicFW's APRS messaging, FX.25 or heard-list persistence: its APRS page wasn't in the wiki copy we read.
- Stock idle current: never measured, so no stock power comparison.
- Faster screens than stock: no stock timings measured.
- Screen note: the beacon screen's help line still says "Manual, timed, distance or smart" though decision 272 dropped distance (a stale firmware string, visible in compare block "Smarter beacons").
