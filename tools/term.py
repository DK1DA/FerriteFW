"""Drive the radio's cable terminal: each argument is a keystroke string (Python escapes: '\\r' Enter, '\\x1b' Esc).
Prints what came back, ANSI codes stripped. Send Esc first (the terminal may be on its banner); addresses take no 0x:
    term.py '\\x1b' ':' 'bk 1 37\\r' '\\x1b'
Port: FERRITE_PORT or /dev/cu.usbserial-114220. Use the project's .venv python (pyserial)."""
import os, re, sys, time
import serial

s = serial.Serial(os.environ.get("FERRITE_PORT", "/dev/cu.usbserial-114220"), 115200, timeout=0.2)


def rd(t=1.5):
    end, b = time.time() + t, b""
    while time.time() < end:
        b += s.read(4096)
    return b


for k in sys.argv[1:]:
    k = k.encode().decode("unicode_escape").encode()
    s.write(k)
    out = rd()
    txt = re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b" ", out).decode("latin1")
    print(f"--- sent {k!r}: {len(out)} bytes")
    print("\n".join(l.rstrip() for l in re.split(r"[\r\n]+", txt) if l.strip())[:3000])
