"""Record the radio's cable log (Cable log on, 0 9 9 3) without sending a byte: any byte from the PC ends the log
until the radio restarts. Usage: cablelog.py SECONDS OUTFILE (appends). Port: FERRITE_PORT or /dev/cu.usbserial-114220."""
import os, sys, time
import serial

s = serial.Serial(os.environ.get("FERRITE_PORT", "/dev/cu.usbserial-114220"), 115200, timeout=0.2)
end, out = time.time() + float(sys.argv[1]), open(sys.argv[2], "ab")
while time.time() < end:
    b = s.read(4096)
    if b:
        out.write(b)
        out.flush()
