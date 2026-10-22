#!/bin/sh
# Downloads the videos' music (CC0, see README.md) into this folder. The audio isn't committed.
set -e
cd "$(dirname "$0")"
[ -s chiptune3.wav ] || curl -fsSL -o chiptune3.wav https://opengameart.org/sites/default/files/synth_3.wav
[ -s partysector.ogg ] || curl -fsSL -o partysector.ogg https://opengameart.org/sites/default/files/partysectorstopforamoment2_0.ogg
ls -la *.wav *.ogg
