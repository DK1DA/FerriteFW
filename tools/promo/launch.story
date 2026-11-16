# Ferrite 0.9.0 launch video (about 90 s): the main features, no keycaps. Format: see the top of preview.story.
# Sources for the captions: tools/promo/CLAIMS.md (preview.story's rows) and manual chapter 28a (satellites).

kind: intro
title: Ferrite
left: Free firmware for the Radtel RT-950 Pro, written from scratch.
seconds: 4
music: music/chiptune3.wav
---
title: Three slots, one radio
layout: center
keys: off
hold: 1.2
left: Three receivers on one screen: listen on one while the others watch or scan.
right: Each slot keeps its own frequency, mode, tones and power. || SLOT switches between them; the knob tunes.
setup: home
script:
  wait 1000ms
  press SLOT
  wait 1000ms
  press SLOT
  wait 1000ms
  press SLOT
  wait 600ms
  knob 4
  wait 1200ms
---
title: Band scope
layout: right
keys: off
hold: 1.2
left: See what's on the air around you, live.
right: The knob moves the marker along the band; the strongest signal is named below.
setup: home
script:
  hold 1
  wait 2500ms
  knob 3
  wait 2000ms
---
title: A channel on one page
layout: left
keys: off
hold: 1
left: Everything about a channel on a single screen.
right: Name, zone, tones, offset, power, squelch, scan and APRS: pick a field, turn the knob.
setup: home; press SLOT
script:
  wait 400ms
  press OK
  wait 400ms
  type 8
  wait 300ms
  press OK
  wait 1300ms
  press DOWN
  wait 350ms
  press DOWN
  wait 350ms
  press DOWN
  wait 350ms
  press RIGHT
  wait 400ms
  press RIGHT
  wait 600ms
  knob 2
  wait 1300ms
---
title: Squelch you can see
layout: right
keys: off
hold: 1
left: Set the squelch against what's on the air right now.
right: The live signal bar shows where the audio opens, before you leave the page.
setup: press BACK; press BACK; home
script:
  wait 400ms
  press OK
  wait 800ms
  press OK
  wait 1600ms
  knob 2
  wait 1300ms
  knob -1
  wait 1300ms
---
title: Crossband repeater
layout: left
keys: off
hold: 1
left: Two bands, one radio: what's heard on A goes out on B, and back.
right: Live: who's passing, the timeout, and the last 30 minutes of activity both ways.
setup: press BACK; home; menu 0 3 4; press HASH; wait 18s
script:
  wait 6s
---
title: Messages, without typing callsigns
layout: center
keys: off
hold: 1.5
left: An empty recipient lists your contacts, recent conversations, stations heard and APRS services.
right: Pick one, then write with predictive text on the keypad. || Acks and retries are handled for you.
setup: press BACK; home
script:
  menu 0 5 2
  wait 1000ms
  press HASH
  wait 1300ms
  press DOWN
  wait 500ms
  press RIGHT
  wait 800ms
  press LEFT
  wait 500ms
  press OK
  wait 700ms
  type 4663
  wait 800ms
  press HASH
  wait 2000ms
---
title: Everything it heard
layout: right
keys: off
hold: 1.2
left: The event log keeps every decode and alert, newest first, through a restart.
right: Filter by source; open one for its details, or tune straight to it. || The heard list comes back after a restart too.
setup: press BACK; home
script:
  wait 600ms
  menu 0 4 1
  wait 1800ms
  press RIGHT
  wait 1000ms
  press LEFT
  wait 800ms
  press OK
  wait 2000ms
---
title: Shortwave on a handheld
layout: right
keys: off
hold: 1.2
left: The broadcast receiver doubles as an HF receiver: AM on long wave, medium wave and shortwave.
right: Its own tuning screen, with the signal and the station's band at a glance.
setup: press BACK; home; menu 0 3 3; wait 2700ms; type 9580; press OK; wait 3000ms
script:
  wait 1000ms
  knob 3
  wait 1300ms
  knob -3
  wait 1800ms
---
title: Satellites
layout: left
keys: off
hold: 1.5
left: The radio predicts the FM satellite passes from its position and the orbits loaded from Ferrite Console.
right: The pass page plots its track across the sky and corrects Doppler on both the uplink and the downlink. || A pointing page shows where to aim.
setup: press BACK; home; menu 0 3 6; wait 8s
script:
  wait 1500ms
  press OK
  wait 3000ms
  press 0
  wait 3000ms
---
kind: intro
title: Ferrite Console
left: Program the radio and watch it live from a browser, over the cable or Bluetooth. Channels, settings, APRS messages, the heard list on a map, and a backup before anything is installed.
seconds: 6
---
kind: outro
title: ferritefw.dev
left: Free. The source is public.
seconds: 5
