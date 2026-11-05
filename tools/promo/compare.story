# Ferrite compared: with the radio's stock firmware, and with what nicFW's wiki describes.
# Every caption's source is in tools/promo/CLAIMS.md (stock from our own analysis; nicFW only from its public wiki,
# docs/research/nicfw-feature-comparison.md). Our side is recorded from the simulator (the firmware's own screens).
# Format: see the top of preview.story.

kind: intro
title: Ferrite and the alternatives
left: What Ferrite does differently from the RT-950 Pro's stock firmware, and what it adds beside nicFW.
seconds: 5
music: music/chiptune3.wav
---
kind: intro
title: Against stock
left: The RT-950 Pro's own firmware, from our analysis of version 0.27 and 0.29.
seconds: 3
---
title: A repeater that times out
layout: left
left: Stock: a crossband repeat has no timeout, so a stuck carrier keeps the transmitter keyed.
right: Ferrite: a timeout on every pass, a duty limit, and the last 30 minutes of activity on screen.
setup: home; menu 0 3 4; press HASH; wait 18s
script:
  wait 8s
---
title: A scope that keeps looking
layout: right
left: Stock: the scope sweeps until the first signal above a threshold, then stops and plays it.
right: Ferrite: a live sweep of the band. The knob moves a marker, the strongest signal is named, and Go tunes to it.
setup: press BACK; home
script:
  hold 1
  wait 3s
  knob 3
  wait 2500ms
---
kind: intro
title: Transmit, in a safer order
left: Stock: the power amplifier's drive is set before the transmit path is selected. || Ferrite: the path is set and read back before any drive, then the drive ramps up over 4 ms.
seconds: 6
---
kind: intro
title: Calibration left alone
left: Stock: with its transmit limits missing from calibration, it falls back to built-in ones. || Ferrite: it never writes calibration, checks it at power-on, and won't transmit without valid limits.
seconds: 6
---
kind: intro
title: Beside nicFW
left: nicFW is a well-loved free firmware for this radio. From its public wiki, here is what Ferrite adds, and where nicFW goes further.
seconds: 5
---
kind: intro
title: Saved as you go
left: nicFW's wiki: settings are written later, with a red icon, and a Shut Down is needed. || Ferrite: changes are saved within 2 seconds and survive a pulled battery.
seconds: 6
---
title: A key lock that still listens
layout: left
left: nicFW's wiki: with the keys locked, only PTT works.
right: Ferrite: hold # to lock. Monitor, squelch and volume still work, and PTT can be locked too.
setup: press BACK; home
script:
  wait 800ms
  hold HASH
  wait 2000ms
  press 5
  wait 1500ms
  hold HASH
  wait 1500ms
---
title: GPS you can navigate by
layout: right
left: nicFW's wiki: position, altitude, satellites, grid, speed, course and time.
right: Ferrite adds accuracy, fix age, a sky plot of the satellites and the nearest waypoint, and a go-to dial with distance, ETA and an arrival alarm.
setup: home
script:
  wait 800ms
  menu 0 6 2
  wait 2500ms
  press RIGHT
  wait 3000ms
---
title: Smarter beacons
layout: left
left: nicFW's wiki: beacons by time or distance, Mic-E and Beacon now.
right: Ferrite: timed or smart beaconing paced by speed, Mic-E, and a beacon after you talk if you want one.
setup: press BACK; home
script:
  wait 800ms
  menu 0 5 6
  wait 1500ms
  press OK
  wait 1200ms
  knob 1
  wait 900ms
  knob 1
  wait 1500ms
---
kind: intro
title: KISS in the background
left: nicFW's wiki: KISS is a screen of its own. || Ferrite: KISS runs in the background over Bluetooth or the cable, while you keep using the radio.
seconds: 6
---
kind: intro
title: Programming in a browser
left: nicFW's wiki: RMS, a Windows program. || Ferrite Console: Chrome or Edge on any computer, over the cable or Bluetooth, with a live view of the radio and a backup before anything is installed.
seconds: 6
---
kind: intro
title: Where nicFW goes further
left: SSB and CW on HF today, keys you can assign to about 40 functions, an editable band plan, DSB and AM transmit, and its own power table. Credit where it's due.
seconds: 6
---
kind: outro
title: ferritefw.dev
left: Free firmware, source public.
seconds: 5
