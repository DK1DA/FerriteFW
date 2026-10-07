#!/usr/bin/env python3
"""Real APRS messages for the predictive-text dictionary: capture them read-only from APRS-IS, keep the ones
people typed, count their words into assets/words/aprs_counts.txt (word<TAB>count; no message text, no callsigns).

    python3 tools/aprs_corpus.py capture msgs.log 7200            # seconds; message packets (t/m)
    python3 tools/aprs_corpus.py capture status.log 7200 --status # status packets (t/s): free text people set
    python3 tools/aprs_corpus.py count msgs.log [status.log ...] --scowl scowl-pre.txt [--show 40]

Capture logs in as FERRITE with passcode -1: receive only, nothing can be sent. Logs hold callsigns and message
text: keep them out of the repository; only the counts go in. Counting keeps what people typed: it drops acks
and rejects, telemetry definitions, messages to oneself, bots and services (OTA, NTS, QSL and weather bots),
messages that aren't mostly English or are mostly numbers, any text already counted (from anyone: software
banners repeat across stations), and more than 6 messages from one sender; for gateway messages
(SMS, email, ANSRVR groups) the address or group in front is dropped and the words kept. Callsigns aren't
counted. Each run adds to the counts already in the file.
"""

import collections, os, re, socket, sys, time

BOT_DEST = re.compile(r"^(NWS|WX|SKY|WXBOT|REPEAT|WHO-IS|FIND|ID|QST|DX|OTA|RGSTRY|NTSGTE|MPAD|WXSVR|SATSVR|TIME|QRZ|LOC)", re.I)
# Gateways people type through: the address or group in front is stripped, the words kept.
GATEWAY = re.compile(r"^(SMSGTE|EMAIL(-\d+)?|ANSRVR|SMS)$", re.I)
BOT_SRC = re.compile(r"^(OTA|RGSTRY|NTSGTE|ANSRVR|SMSGTE|EMAIL\d*|WXBOT|WXSVR|MPAD|SATSVR|WHO-IS|REPEAT|FIND|BLN|APRSPH|SPC|NWS)", re.I)
MACHINE = re.compile(r"^(ack|rej)\w*$|^(PARM|UNIT|EQNS|BITS)\.|^\?|https?://|T#\d|Rain1h|\d(hPa|mm|km/h|m/s)|\b[THWP]=\d", re.I)

def messages(path):
    for line in open(path, encoding="latin1"):
        try:
            _, pkt = line.rstrip("\n").split("\t", 1)
        except ValueError:
            continue
        m = re.match(r"^([^>]+)>([^,:]+)[^:]*::([^:]{9}):(.*)$", pkt)
        if not m:
            continue
        src, tocall, dest, text = m.group(1), m.group(2), m.group(3).strip(), m.group(4)
        text = re.sub(r"\{[\w}]*$", "", text).strip()          # message number
        if GATEWAY.match(dest):
            # "@5551234567 text", "user@example.com text", "CQ GROUP text", "J GROUP": the address goes
            text = re.sub(r"^(@\S+|\S+@\S+|CQ\s+\S+|[JKU]\s+\S+$)\s*", "", text, flags=re.I)
        yield src, tocall, dest, text

def human(src, tocall, dest, text):
    if not text or MACHINE.search(text) or BOT_DEST.match(dest) or BOT_SRC.match(src):
        return False
    if src.split("-")[0] == dest.split("-")[0]:                # to itself: telemetry, notes
        return False
    return mostly_words(text)


def mostly_words(text):
    """Letters outnumber digits: not a reading, a list of numbers or a timestamp."""
    return bool(re.search(r"[A-Za-z]{2}", text)) and sum(c.isalpha() for c in text) > 2 * sum(c.isdigit() for c in text)

CALL = re.compile(r"^[a-z]{1,2}\d{1,2}[a-z]{1,4}(-\d{1,2})?$|^\d[a-z]\d[a-z]{1,3}$")
WORDS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets", "words")


def lexicon(scowl):
    """Words that count as English: our lists, plus SCOWL's (any level) when given."""
    lex = set()
    for f in ("core.txt", "ham.txt"):
        lex |= {l.split("#")[0].strip() for l in open(os.path.join(WORDS, f)) if l.split("#")[0].strip()}
    if scowl:
        for line in open(scowl, encoding="utf-8"):
            m = re.match(r"^.*?:\s+([a-z]+)\b", line)
            if m:
                lex.add(m.group(1))
    return lex
PER_SENDER = 6

def english(text, lex):
    ws = [w for w in re.findall(r"[a-z']+", text.lower()) if not CALL.match(w)]
    return len(ws) > 0 and sum(w.strip("'") in lex for w in ws) / len(ws) >= 0.7



def capture(out, secs, status):
    end, n = time.time() + secs, 0
    login = b"user FERRITE pass -1 vers ferrite-corpus 0.1 filter " + (b"t/s" if status else b"t/m") + b"\r\n"
    with open(out, "a", encoding="latin1") as f:
        while time.time() < end:
            try:
                s = socket.create_connection(("rotate.aprs2.net", 14580), timeout=15)
                s.sendall(login)
                s.settimeout(60)
                buf = b""
                while time.time() < end:
                    d = s.recv(8192)
                    if not d:
                        break
                    buf += d
                    *lines, buf = buf.split(b"\n")
                    for l in lines:
                        l = l.rstrip(b"\r")
                        if l and not l.startswith(b"#"):
                            f.write(f"{int(time.time())}\t{l.decode('latin1')}\n")
                            n += 1
                    f.flush()
            except OSError as e:
                print("reconnect:", e, flush=True)
                time.sleep(10)
    print("captured", n, "packets", flush=True)


def statuses(path):
    for line in open(path, encoding="latin1"):
        try:
            _, pkt = line.rstrip("\n").split("\t", 1)
        except ValueError:
            continue
        m = re.match(r"^([^>]+)>([^,:]+)[^:]*:>(.*)$", pkt)
        if m:
            text = re.sub(r"^\d{6}[zh/]", "", m.group(3)).strip()   # a timestamp in front
            yield m.group(1), m.group(2), "", text


def count(paths, scowl, show):
    lex = lexicon(scowl)
    seen, kept, total = set(), [], 0
    per = collections.Counter()
    for path in paths:
        src_iter = statuses(path) if "status" in os.path.basename(path) else messages(path)
        for src, tocall, dest, text in src_iter:
            total += 1
            if dest and not human(src, tocall, dest, text):
                continue
            if not dest and (not text or MACHINE.search(text) or not mostly_words(text)):
                continue
            key = re.sub(r"[\d.]+", "#", text.lower())    # one count per distinct text, from anyone
            if key in seen or per[src] >= PER_SENDER or not english(text, lex):
                continue
            seen.add(key)
            per[src] += 1
            kept.append(text)
    words = collections.Counter()
    for t in kept:
        for w in re.findall(r"[a-z0-9']+", t.lower()):
            w = w.strip("'")
            if w and not CALL.match(w) and (not w.isdigit() or w in ("73", "88")):
                words[w] += 1
    out = os.path.join(WORDS, "aprs_counts.txt")
    old = collections.Counter()
    if os.path.exists(out):
        for l in open(out):
            l = l.split("#")[0].strip()
            if l:
                w, c = l.split("\t")
                old[w] += int(c)
    old.update(words)
    with open(out, "w") as f:
        f.write("# Word counts from real APRS messages and statuses (tools/aprs_corpus.py). No text, no callsigns.\n")
        for w, c in old.most_common():
            f.write(f"{w}\t{c}\n")
    print(f"{total} packets, {len(kept)} typed by people, {len(words)} words this run; {len(old)} in {out}")
    for t in kept[:show]:
        print("  ", t)


def main():
    a = sys.argv[1:]
    if len(a) >= 3 and a[0] == "capture":
        return capture(a[1], int(a[2]), "--status" in a)
    if len(a) >= 2 and a[0] == "count":
        if "--scowl" not in a:
            sys.exit("count needs --scowl PATH (SCOWL's data/scowl-pre.txt): the English check uses it")
        scowl = a[a.index("--scowl") + 1]
        show = int(a[a.index("--show") + 1]) if "--show" in a else 0
        paths = [p for i, p in enumerate(a[1:], 1) if not p.startswith("--") and a[i - 1] not in ("--scowl", "--show")]
        return count(paths, scowl, show)
    sys.exit(__doc__)


if __name__ == "__main__":
    main()
