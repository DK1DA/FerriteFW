#!/bin/sh
# PNGs and favicon.ico from the SVGs tools/mklogo.py writes. Needs rsvg-convert and ImageMagick.
set -e
cd "$(dirname "$0")/../assets/logo"
rsvg-convert social-preview.svg -o social-preview.png
for n in 1024 512 256 192 180 128; do rsvg-convert -w $n -h $n ferrite-icon.svg -o icon-$n.png; done
for n in 64 48 32 16; do rsvg-convert -w $n -h $n ferrite-icon-small.svg -o icon-$n.png; done
magick icon-16.png icon-32.png icon-48.png favicon.ico
