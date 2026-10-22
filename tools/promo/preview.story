# Ferrite preview: seven features, recorded from the simulator (the firmware's own screens).
# Blocks are separated by lines of ---. Fields:
#   kind: intro or outro for a title card (no radio), shown `seconds` long
#   title, left, right: caption text (|| starts a new paragraph)
#   layout: center (screen in the middle, left and right captions beside it; the default), right (all captions on
#           the right of the screen) or left (all on the left)
#   hold: seconds the last frame stays before the fade (default 1.5)
#   setup: a script played before recording, not shown; script: recorded (the indented lines below it)
#   music: on any block, an audio file for the whole video (relative to this file), with a licence that allows
#          it (note its source and licence in tools/promo/music/README.md); without one the video is silent
# Scripts use the simulator harness's steps: press K, hold K, knob N, type 123, wait 2s, menu 0 5 1, home, event NAME.

kind: intro
title: Ferrite
left: Firmware for the Radtel RT-950 Pro.
seconds: 4
music: music/chiptune3.wav
---
title: Three slots, one radio
layout: center
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
layout: right
left: Hold 1 to see what's on the air around you, live.
right: The knob moves the marker along the band; the strongest signal is named below.
setup: home
script:
  hold 1
  wait 3s
  knob 3
  wait 2500ms
---
title: A channel on one page
layout: left
left: MENU 8 opens everything about a channel on a single screen.
right: Name, zone, tones, offset, power, squelch, scan and APRS: pick a field, turn the knob.
setup: home; press SLOT
script:
  wait 600ms
  press OK
  wait 500ms
  type 8
  wait 300ms
  press OK
  wait 1500ms
  press DOWN
  wait 600ms
  press RIGHT
  wait 600ms
  knob 2
  wait 1500ms
  press BACK
  wait 300ms
---
title: Squelch you can see
layout: right
left: Set the squelch against what's on the air right now.
right: The live signal bar shows where the audio opens, before you leave the page.
setup: press BACK; home
script:
  wait 500ms
  press OK
  wait 900ms
  press OK
  wait 1800ms
  knob 2
  wait 1500ms
  knob -1
  wait 1500ms
  press BACK
  wait 300ms
---
title: Crossband repeater
layout: left
left: Two bands, one radio: what's heard on A goes out on B, and back.
right: Live: who's passing, the timeout, and the last 30 minutes of activity both ways.
setup: press BACK; home; menu 0 3 5; press HASH; wait 18s
script:
  wait 9s
---
title: Messages, without typing callsigns
layout: center
left: An empty recipient lists your contacts, recent conversations, stations heard and APRS services.
right: Pick one, then write with predictive text on the keypad. || Acks and retries are handled for you.
setup: press BACK; home
script:
  menu 0 5 2
  wait 1200ms
  press HASH
  wait 1500ms
  press DOWN
  wait 500ms
  press RIGHT
  wait 900ms
  press LEFT
  wait 500ms
  press OK
  wait 800ms
  type 4663
  wait 900ms
  press HASH
  wait 2500ms
---
title: HF and SSB on a handheld
layout: right
left: The broadcast receiver doubles as an HF radio: AM, USB, LSB and CW below 30 MHz.
right: Here, FT8 on 40 m in USB, with its own tuning screen.
setup: press BACK; home; menu 0 3 4; wait 2700ms; type 7074; press OK; press STAR; wait 10500ms
script:
  wait 1200ms
  knob 3
  wait 1500ms
  knob -3
  wait 2500ms
---
kind: outro
title: ferritefw.dev
left: Free and Source Included.
seconds: 5
