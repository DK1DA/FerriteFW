# Low Power Mode at 24 MHz: bench checks (PWR-24.9)

On black, from `ferrite/` on branch `lpm-24mhz`. Rule: manual PWR-24.9, decision 250 (docs branch `lpm-24mhz`).

## Setup
- Build: `tools/mkbtf.sh tx-bench` (Dry TX on at power-on). For the 120 MHz comparison, main's tx-bench image.
- Flash black (cable): `../.venv/bin/python tools/flash.py PORT target/ferrite-txbench.btf --backup BACKUP.bin`.
- Power from the DPS-150P at 7.60 V: `../.venv/bin/python tools/dps150.py PORT set 7.6 2`, `on`.
- Cable log on: `:set` 0 9 9 7 on (System > Advanced > Cable log), or `tools/cablelog.py SECONDS FILE` alongside.
- The speed now: `mem32 40021004` (CRM_CFG): bits 18-21 = 1 (x3, 24 MHz) or 13 (x15, 120 MHz); bits 8-13 = 0 at 24 MHz (APBs /1), 0x24 at 120.

## Checks
1. Enter: `:set 0 7 7 2 on`. After about 1 s the cable log says `clock: 24 MHz (interrupts held N us)`. Note N (expect well under 1000).
2. Leave: `:set 0 7 7 2 off`, and SIDE1+SIDE2 held 1 s: `clock: 120 MHz ...` at once, the normal home redrawn at full speed.
3. Current, 60 s averages (`dps150.py PORT log 70 --csv F`, drop the first 10 s), screen dark (0 7 4 2 lowest / timeout) and lit:
   - in Low Power Mode on this build (24 MHz) against main's build (120 MHz). Expect about 20 mA less.
   - out of the mode on this build: the same as main's (no change at 120 MHz).
4. Enter/leave 50 times (script `:set 0 7 7 2 on` / `off`, 3 s apart). Every switch logged; no `*** FAULT`; the link still answers `ui` after each.
5. Time keeping: note the status bar clock against a phone before and after the 50 switches, and after 10 min in the mode. No drift beyond the RTC's usual (the tick stays within 0.5 ms per the design).
6. At 24 MHz (in the mode), each works:
   - display: the Low Power home, then MENU 0 (menus draw; note `draw N ms` lines in the cable log for big redraws);
   - keys and the knob (no missed presses), SIDE1+2 leave;
   - a key beep, and a message sound (0 7 5 5 preview, or an APRS message from clear before entering);
   - backlight and keypad light levels unchanged in look (25 % cap; no flicker, no whine);
   - the cable terminal: `ui`, `slot`, `get` over `tools/ferrite_link.py PORT ui`, and `tools/term.py` at the default and at the fastest link rate (921600).
7. Dry TX key-up in the mode (145.495, SDR well away or unplugged is not needed with Dry TX on): `tools/ferrite_link.py PORT key --seconds 3`. TX screen and timer as usual, CTCSS/roger as set, the radio back to receive; `mem32 40021004` still 24 MHz during and after.
8. Battery reading in the mode matches the DPS (PWR-22) as at 120 MHz.
9. Auto: `:set 0 7 7 3` 40 %, supply to 7.1 V: enters, `clock: 24 MHz`; 8.2 V: leaves, `clock: 120 MHz`.
10. A `clock-24` image (`tools/mkbtf.sh tx-bench clock-24`): no `clock:` lines at all (nothing to switch); note its current (APBs now /1 at 24 MHz, measured before at /2).

## What only the radio shows
- The PLL relock time (N above), and whether a cable byte is ever lost at a switch (link retries in `ferrite_link.py`).
- Redraw speed at 24 MHz: menus and full redraws are about 5x slower in CPU time; the Low Power home's 1 Hz updates are small.
- Beep and message-sound pitch the same at both speeds (TMR6 re-timed).

# The governor: dynamic clocking out of the mode (PWR-26, decision 252)

Same setup (black, `tx-bench`, the cable, the DPS at 7.60 V), Low Power Mode Off, 0 9 9 9 MCU clock Dynamic. The readback's `bar` line has `mcu=` (MHz now), `switches=`, `held_us=` (the longest interrupts-off) and `why=` (bits: 1 input, 2 decode, 4 scan, 8 mode, 16 tx, 32 alert, 64 link, 128 audio). A readback is itself link traffic, but its answer is built before that turn's governor runs: after 11 s or more with nothing on the cable (a link frame keeps the session live 10 s), one `ferrite_link.py PORT ui` reads the state's own clock (`why` without 64); readbacks closer together read 120 with 64. System's Clock row on the screen and the cable log's `clock:` lines (`tools/cablelog.py`, cable log on) show it too.

## Which state reads which clock
| State | Expect | Reason bit |
| --- | --- | --- |
| Home, untouched 4 s, no link traffic | 24 | 0 |
| Any key or the knob, 3 s + 1 s hold | 120 | 1 |
| Screen gone dark (the key's 3 s cut short) | 24 after 1 s | 0 |
| APRS mode 0 5 3 On; KISS on; an APRS-flagged channel active | 120 | 2 |
| POCSAG on, a slot on the pager frequency | 120 | 2 |
| SAME on, a slot on 162.400-162.550 | 120 | 2 |
| SAME on, no slot on a weather channel | 24 | 0 |
| SIDE2 scan running (frequency and channel) | 120 | 4 |
| Band scope, tone search, counter, FM broadcast, HF mode, crossband | 120 | 8 |
| Keyed (Dry TX), the Transmit screen | 120 | 16 |
| The alert screen | 120 | 32 |
| `ferrite_link.py PORT ui` every 2 s (the second on reads it); then stop: 24 after 5 s + 1 s (10 s + 1 s after a link frame) | 120 | 64 |
| A beep or message sound playing | no switch until it ends | 128 |
| Plain receive, watch on, a signal heard | 24 | 0 |
| 0 9 9 9 Fixed, any state, Low Power Mode too | 120, `switches=` frozen | – |

## Checks
1. Each row above: the readback (or System's row for the rest states), and the cable log line `clock: N MHz (interrupts held U us, reasons)`. Note the largest U against `held_us=`.
2. Soak: an hour of normal use (keys and menus every few minutes, a scan, two `uitest.py` runs, a Dry TX key-up every 10 min, an APRS mode spell with clear beaconing). Note `switches=` at the end (expect hundreds); no `*** FAULT`, no stuck key or knob step, the status bar clock against a phone's within a second.
3. The watch's hops: watch on with A and B on one chip (`chips.turns_on_a_shared_chip`, the bench's watch test). Count hops per second from the cable log at 24 MHz (untouched) and at 120 (keep touching the knob, or 0 9 9 9 Fixed). Expect the same within 10 %; if 24 is slower, a watch reason goes in PWR-26.1.
4. Bluetooth link at both speeds: `ferrite_link.py ble:RADIO-ID ui` after 15 s idle (raised by the request) and back to back; `uitest.py` over Bluetooth. No timeouts; count retries (a byte lost at a switch-up).
5. The cable's bursts: a full `uitest.py` run on the radio after 15 s idle: none of the 2 s timeouts seen at a fixed 24 MHz.
6. Audio (PWR-26.4): APRS mode on, clear sending a beacon every 30 s: every frame decoded; the log shows the 120 MHz switch before the capture and none while it runs. A Dry TX beacon decodes on the SDR.
7. Current on the DPS (`dps150.py PORT log 70 --csv F`, drop the first 10 s), screen dark, no link traffic: Dynamic against Fixed (expect about 20 mA less, as in the mode); screen lit untouched; and with keys every 2 s (expect Fixed's).
