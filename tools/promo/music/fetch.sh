#!/bin/sh
# Downloads the videos' music (CC0, see README.md) into this folder. The audio isn't committed.
set -e
cd "$(dirname "$0")"
[ -s chiptune3.wav ] || curl -fsSL -o chiptune3.wav https://opengameart.org/sites/default/files/synth_3.wav
[ -s partysector.ogg ] || curl -fsSL -o partysector.ogg https://opengameart.org/sites/default/files/partysectorstopforamoment2_0.ogg
[ -s arplands.ogg ] || curl -fsSL -o arplands.ogg https://opengameart.org/sites/default/files/the_arplands_0.ogg
[ -s dreamalive.ogg ] || curl -fsSL -o dreamalive.ogg "https://opengameart.org/sites/default/files/Keep%20your%20dream%20alive%21%20seamless.ogg"
[ -s concentration.wav ] || curl -fsSL -o concentration.wav https://opengameart.org/sites/default/files/game_0.wav
[ -s bardstale.wav ] || curl -fsSL -o bardstale.wav https://opengameart.org/sites/default/files/CHIPTUNE_Loop_The_Bards_Tale.wav
ls -la *.wav *.ogg
