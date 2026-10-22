# Ferrite APRS, recorded from the simulator (the firmware's own screens). Format: see preview.story.
# The decoder numbers are docs/showcase/aprs-decoder.md's WA8LMF rows (Oct 22, 2026: the radio's receive path at 24 kHz, decision 240): copy them from there when they change,
# never estimate them.

kind: intro
title: APRS on Ferrite
left: Position, messages and a decoder that holds its own, on the radio itself.
seconds: 4
music: music/chiptune3.wav
---
title: APRS on slot C
layout: right
left: MENU 0 5 3 puts APRS mode on slot C.
right: A and B stay yours: listen, scan and talk as usual while C watches the APRS frequency.
setup: home
script:
  wait 1000ms
  menu 0 5 3
  wait 900ms
  knob 1
  wait 900ms
  home
  wait 3000ms
---
title: Who's around
layout: left
left: OK on the APRS card opens the heard list: callsign, distance, bearing and how they were heard.
right: Open a station, hold MENU, and save its position as a waypoint.
setup: wait 300ms
script:
  wait 800ms
  press SLOT
  wait 300ms
  press SLOT
  wait 900ms
  press OK
  wait 700ms
  press OK
  wait 2200ms
  press OK
  wait 2200ms
  hold OK
  wait 1500ms
  press DOWN
  wait 400ms
  press DOWN
  wait 700ms
  press OK
  wait 2500ms
  home
  wait 600ms
---
title: Beacons
layout: right
left: Beacon now from the APRS card, or automatically.
right: Timed, by distance or smart beaconing (0 5 5 1). || Beacons go out at their own power, Low by default.
setup: home
script:
  wait 800ms
  hold OK
  wait 1500ms
  press OK
  wait 2500ms
  menu 0 5 5 1
  wait 1200ms
  knob 1
  wait 700ms
  knob 1
  wait 700ms
  knob 1
  wait 1500ms
---
title: Messages
layout: left
left: An inbox, retries and acks, and an address book: saved contacts, recent and heard stations.
right: Quick messages and predictive text on the keypad.
setup: home
script:
  wait 800ms
  menu 0 5 2
  wait 1500ms
  press HASH
  wait 1500ms
  knob 1
  wait 600ms
  knob 1
  wait 900ms
  press OK
  wait 1500ms
  knob 1
  wait 600ms
  knob 1
  wait 1500ms
  press OK
  wait 2500ms
---
title: FX.25
layout: right
left: Forward error correction around each AX.25 frame, sent and received (0 5 7 8).
right: Plain AX.25 receivers still decode it; FX.25 receivers repair damaged bytes. || 16 check bytes by default.
setup: home
script:
  wait 800ms
  menu 0 5 7 8
  wait 1500ms
  knob 1
  wait 900ms
  knob -1
  wait 2500ms
---
title: And more
layout: left
left: A digipeater, and a KISS TNC over Bluetooth or the cable for phone and PC apps.
right: It answers station queries, and sends a beacon after you talk if you want one.
setup: home
script:
  wait 800ms
  menu 0 5
  wait 1200ms
  press DOWN
  wait 1000ms
  press DOWN
  wait 1000ms
  press DOWN
  wait 1000ms
  press DOWN
  wait 1000ms
  press DOWN
  wait 1000ms
  press DOWN
  wait 1000ms
  press DOWN
  wait 1000ms
  wait 1500ms
---
kind: intro
title: 1,011 and 1,007 packets
left: The WA8LMF TNC Test CD, tracks 1 and 2, through the radio's own receive path, in about 1.7 KB of RAM. The KPC-3+ hardware TNC's published counts: 989 and 925.
seconds: 7
---
kind: outro
title: ferritefw.dev
left: Free and Source Included.
seconds: 5
