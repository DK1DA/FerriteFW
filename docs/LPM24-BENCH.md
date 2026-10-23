# Low Power Mode at 24 MHz: bench checks (PWR-24.9)

On black, from `ferrite/` on branch `lpm-24mhz`. Rule: manual PWR-24.9, decision 250 (docs branch `lpm-24mhz`).

## Setup
- Build: `tools/mkbtf.sh tx-bench` (Dry TX on at power-on). For the 120 MHz comparison, main's tx-bench image.
- Flash black (cable): `../.venv/bin/python tools/flash.py PORT target/ferrite-txbench.btf --backup ../backups/blackradio/flash.bin`.
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
