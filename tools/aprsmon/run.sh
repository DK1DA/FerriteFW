#!/bin/bash
# The APRS monitor: the RTL-SDR on the APRS frequency into Direwolf, every decoded frame into a daily log.
# Settings by environment (aprsmon.service): APRS_FREQ (144.39M), GAIN (40), PPM (0), LOGDIR.
set -o pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
FREQ="${APRS_FREQ:-144.39M}"
LOGDIR="${LOGDIR:-$HOME/aprsmon/logs}"
mkdir -p "$LOGDIR"
exec bash -c "rtl_fm -f $FREQ -M fm -s 22050 -g ${GAIN:-40} -p ${PPM:-0} -E dc - 2>/dev/null \
  | direwolf -c '$DIR/direwolf.conf' -r 22050 -t 0 -l '$LOGDIR' -"
