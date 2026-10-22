# Ferrite Low Power Mode, recorded from the simulator (the firmware's own screens). Format: see preview.story.
# The numbers are the power bench's (docs/dev/power.html, run lpm-120MHz, Oct 21, 2026): copy them from there when
# they change, never estimate them.

kind: intro
title: Low Power Mode
left: When the battery matters more than features.
seconds: 4
music: music/chiptune3.wav
---
title: One switch
layout: right
left: MENU 0 7 7 2 turns it on.
right: The radio drops to one VFO and an unmistakable screen. || Scan, watch, APRS, GPS and Bluetooth go quiet.
setup: home
script:
  menu 0 7 7
  wait 1500ms
  press DOWN
  wait 900ms
  knob 1
  wait 3500ms
---
title: Or by itself
layout: left
left: Set it to Auto and it comes on below 10, 20, 30 or 40 % battery.
right: It goes off again by itself on the charger.
setup: down SIDE1; down SIDE2; wait 1300ms; up SIDE2; up SIDE1; wait 600ms; menu 0 7 7; press DOWN; knob 1; press BACK; press BACK; press BACK; home; wait 600ms
script:
  wait 1500ms
  event battery
  wait 4000ms
---
title: Still a radio
layout: right
left: Tune, change channels and edit the VFO as usual.
right: Receive and transmit on the one VFO, at any power.
setup: wait 5500ms
script:
  wait 800ms
  knob 3
  wait 1200ms
  knob -2
  wait 1500ms
---
title: Out with one hold
layout: left
left: Hold SIDE1 and SIDE2 together.
right: Everything comes back exactly as it was.
setup: wait 300ms
script:
  wait 800ms
  down SIDE1
  down SIDE2
  wait 1400ms
  up SIDE2
  up SIDE1
  wait 2500ms
---
kind: intro
title: 41 % less current in typical use
left: About 23 hours becomes about 39 on the stock battery pack.
seconds: 6
---
kind: outro
title: ferritefw.dev
left: Free and Source Included.
seconds: 5
