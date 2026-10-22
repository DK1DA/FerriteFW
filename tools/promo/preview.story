# Ferrite preview: five features, recorded from the simulator (the firmware's own screens).
# Blocks are separated by lines of ---. Fields: title, left, right (caption text; || starts a new paragraph),
# setup (a script played before recording, not shown) and script (recorded; the indented lines below it).
# Scripts use the simulator harness's steps: press K, hold K, knob N, type 123, wait 2s, menu 0 5 1, home, event NAME.
# The first block with kind: intro and the last with kind: outro are title cards (no radio), `seconds` long.

kind: intro
title: Ferrite
left: Firmware for the Radtel RT-950 Pro.
seconds: 3.5
---
title: Three slots, one radio
left: Three receivers on one screen: listen on one while the others watch or scan.
right: Each slot keeps its own frequency, mode, tones and power. || SLOT switches between them; the knob tunes. || Where a slot can't transmit, it says so.
setup: home
script:
  wait 1500ms
  press SLOT
  wait 1200ms
  press SLOT
  wait 1200ms
  press SLOT
  wait 800ms
  knob 4
  wait 1500ms
---
title: Band scope
left: Hold 1 to see what's on the air around you.
right: Live bars across the band. || The knob moves along it.
setup: home
script:
  hold 1
  wait 3s
  knob 3
  wait 2500ms
  press BACK
  wait 500ms
---
title: Scanning that tells you more
left: SIDE2 scans the slot's zone or band.
right: It keeps the last hits: when, for how long, and how strong. || On a channel scan, the channel's name.
setup: home
script:
  wait 600ms
  press SIDE2
  wait 6s
  press SIDE2
  wait 800ms
---
title: APRS, built in
left: Stations heard, newest first, with distance and bearing.
right: Open one for its position, course, path and the last message it sent you.
setup: home
script:
  menu 0 5 1
  wait 2s
  press DOWN
  wait 600ms
  press UP
  wait 400ms
  press OK
  wait 3500ms
---
title: Messages, without typing callsigns
left: An empty recipient lists your contacts, recent conversations, stations heard and APRS services.
right: Pick one, then write with predictive text on the keypad. || Acks and retries are handled for you.
setup: home
script:
  menu 0 5 2
  wait 1200ms
  press HASH
  wait 1500ms
  press DOWN
  wait 500ms
  press RIGHT
  wait 900ms
  press RIGHT
  wait 900ms
  press LEFT
  wait 500ms
  press LEFT
  wait 500ms
  press OK
  wait 800ms
  type 4663
  wait 900ms
  press HASH
  wait 2500ms
---
kind: outro
title: ferritefw.dev
left: Free for any noncommercial use. Draft preview, from the simulator.
seconds: 4
