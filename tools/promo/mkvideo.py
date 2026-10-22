#!/usr/bin/env python3
"""A video of the radio from a storyboard: the simulator plays each block's script (the firmware's own screens,
recorded frame by frame), each block gets a 1920 x 1080 card with its captions left and right and the screen in
the middle at 3x, and the blocks are joined with fades.

    python3 tools/promo/mkvideo.py [STORY] [-o OUT.mp4] [--only N]   (from ferrite/; default tools/promo/preview.story)

Needs ffmpeg and Google Chrome (headless, to draw the cards). The storyboard format is described at the top of
preview.story. Work files go to target/promo/; --only N records just block N (to tune a script quickly).
"""
import argparse
import html
import os
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]  # ferrite/
WORK = ROOT / "target" / "promo"
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
FPS = 50  # the harness steps 20 ms a frame
# 2x: an even scale keeps every pixel's colour inside the video's 2 x 2 chroma blocks, so edges stay crisp (3x
# smeared them), and leaves room around the screen.
SCALE = 2
SW, SH = 240 * SCALE, 320 * SCALE
PAD = 36  # the bezel around the screen
FADE = 0.4
# Each radio block ends on its last frame for this long before the fade, so the viewer can read it (`hold:` per block).
HOLD = 1.5
# Where the screen goes, by the block's layout: captions on both sides (center), all on the right, or all on the left.
SCREEN_AT = {"center": ((1920 - SW) // 2, 190), "right": (330, 190), "left": (1920 - 330 - SW, 190)}


def parse(path):
    blocks = []
    for raw in path.read_text().split("\n---\n"):
        b, key = {}, None
        for line in raw.splitlines():
            if line.startswith("#") and key != "script":
                continue
            if key == "script" and (line.startswith("  ") or not line.strip()):
                b["script"] = b.get("script", "") + line.strip() + "\n"
                continue
            if ":" in line and not line.startswith(" "):
                key, val = line.split(":", 1)
                key = key.strip()
                b[key] = val.strip()
        if b:
            blocks.append(b)
    return blocks


def paras(text):
    return "".join(f"<p>{html.escape(t.strip())}</p>" for t in text.split("||") if t.strip())


def card(b, n, total, out):
    logo = (HERE / "logo.svg").read_text()
    kind = b.get("kind", "")
    if kind in ("intro", "outro"):
        body = f'<div class="center">{logo.replace("<svg", "<svg class=\"logo\"", 1)}<h1>{html.escape(b.get("title", ""))}</h1>{paras(b.get("left", ""))}</div>'
    else:
        lay = b.get("layout", "center")
        x, y = SCREEN_AT[lay]
        title = f'<h1>{html.escape(b.get("title", ""))}</h1>'
        bezel = f'<div class="bezel" style="left:{x - PAD}px;top:{y - PAD}px;width:{SW + 2 * PAD}px;height:{SH + 2 * PAD}px"></div>'
        if lay == "center":
            cols = (f'<div class="col al" style="left:110px;width:{x - PAD - 190}px">{title}{paras(b.get("left", ""))}</div>'
                    f'<div class="col" style="left:{x + SW + PAD + 80}px;width:{1920 - x - SW - PAD - 190}px">{paras(b.get("right", ""))}</div>')
        elif lay == "right":
            l = x + SW + PAD + 120
            cols = f'<div class="col" style="left:{l}px;width:{1920 - l - 160}px">{title}{paras(b.get("left", ""))}{paras(b.get("right", ""))}</div>'
        else:
            w = x - PAD - 120 - 160
            cols = f'<div class="col al" style="left:160px;width:{w}px">{title}{paras(b.get("left", ""))}{paras(b.get("right", ""))}</div>'
        body = bezel + cols + '<div class="n">RT-950 PRO · SIMULATOR, THE FIRMWARE\'S OWN SCREENS</div>'
    page = (HERE / "card.html").read_text().replace("{{FONTS}}", "file://" + str(ROOT / "fonts")).replace("{{BODY}}", body)
    src = out.with_suffix(".html")
    src.write_text(page)
    # Its own throwaway profile (a running Chrome's can hang a headless one), and a timeout with retries.
    for attempt in range(3):
        try:
            subprocess.run([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars", "--no-first-run",
                            f"--user-data-dir={WORK / 'chrome'}", "--window-size=1920,1080",
                            f"--screenshot={out}", f"file://{src}"], check=True, capture_output=True, timeout=60)
            return
        except subprocess.TimeoutExpired:
            print(f"  card {out.name}: Chrome timed out, trying again ({attempt + 1})")
    sys.exit(f"Chrome couldn't draw {out.name}")


# Keys shown under the screen as they're pressed (Alex, Oct 21, 2026: it makes the videos tutorials) (a block's `keys: off` hides them): the harness's key names as on
# the radio's keycaps.
KEY_NAMES = {"OK": "MENU", "MENU": "MENU", "BACK": "BACK", "UP": "\u25b2", "DOWN": "\u25bc", "LEFT": "\u25c0",
             "RIGHT": "\u25b6", "SLOT": "SLOT", "VM": "V/M", "STAR": "*", "HASH": "#", "SIDE1": "SIDE1",
             "SIDE2": "SIDE2", "PTT": "PTT", "PTTB": "PTT B"}
GROUP_S = 0.8   # presses closer than this share one chip
LINGER_S = 1.2  # a chip stays this long after its last press


def key_events(path):
    """[(start_s, end_s, [(text, is_key)])] from a recording's NN.keys: presses close together share a chip;
    keys held together (down ... up) are one chip ("hold SIDE1 + SIDE2") from the first down to the last up."""
    if not path.exists():
        return []
    chips, cur, held, held_from = [], None, [], None

    def flush():
        nonlocal cur
        if cur:
            chips.append((cur[0], cur[1] + LINGER_S, cur[2]))
            cur = None

    def add(t, tokens):
        nonlocal cur
        if cur and t - cur[1] <= GROUP_S:
            cur[1] = t
            cur[2] += [(" ", False)] + tokens
        else:
            flush()
            cur = [t, t, tokens]

    for line in path.read_text().splitlines():
        f, step = line.split("\t", 1)
        t = int(f) / FPS
        cmd, _, arg = step.partition(" ")
        arg = arg.strip()
        name = KEY_NAMES.get(arg, arg)
        if cmd == "press":
            add(t, [(name, True)])
        elif cmd == "hold":
            add(t, [("hold", False), (" ", False), (name, True)])
        elif cmd == "type":
            add(t, [x for c in arg for x in ((c, True), (" ", False))][:-1])
        elif cmd == "menu":
            add(t, [("MENU", True)] + [x for c in arg.split() for x in ((" ", False), (c, True))])
        elif cmd == "knob":
            n = int(arg)
            add(t, [("knob", False), (" ", False), ((f"+{n}" if n > 0 else f"\u2212{-n}"), True)])
        elif cmd == "down":
            flush()
            if not held:
                held_from = t
            held.append(name)
        elif cmd == "up" and name in held:
            held.remove(name)
            if not held:
                toks, names = [("hold", False), (" ", False)], held_names
                for k, nm in enumerate(names):
                    toks += ([(" + ", False)] if k else []) + [(nm, True)]
                chips.append((held_from, t + LINGER_S, toks))
        if cmd == "down":
            held_names = list(held)
    flush()
    chips.sort()
    # One chip at a time: each ends where the next begins.
    return [(a, min(b, chips[k + 1][0]) if k + 1 < len(chips) else b, x) for k, (a, b, x) in enumerate(chips)]


def chip_png(tokens, out):
    """A row of keycaps (dark text on light caps) and words, on a dark rounded bar."""
    from PIL import Image, ImageDraw, ImageFont
    f = ImageFont.truetype(str(ROOT / "fonts" / "JetBrainsMono-Bold.ttf"), 30)
    pad, cap_pad, h = 18, 10, 64
    draw0 = ImageDraw.Draw(Image.new("RGBA", (10, 10)))
    widths = [draw0.textlength(t, font=f) + (2 * cap_pad if k else 0) for t, k in tokens]
    w = int(sum(widths) + 2 * pad)
    im = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    d.rounded_rectangle((0, 0, w - 1, h - 1), 14, fill=(22, 26, 32, 235), outline=(70, 78, 90, 255), width=2)
    x = pad
    for (t, k), wd in zip(tokens, widths):
        if k:
            d.rounded_rectangle((x, 12, x + wd, h - 12), 7, fill=(236, 233, 225, 255))
            d.text((x + cap_pad, h / 2), t, font=f, fill=(20, 22, 26, 255), anchor="lm")
        else:
            d.text((x, h / 2), t, font=f, fill=(201, 205, 211, 255), anchor="lm")
        x += wd
    im.save(out)
    return w, h


def ffmpeg(*args):
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *args], check=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("story", nargs="?", default=str(HERE / "preview.story"))
    ap.add_argument("-o", "--out", default=str(WORK / "preview.mp4"))
    ap.add_argument("--only", type=int, help="record and render only this block (0-based)")
    a = ap.parse_args()
    if not shutil.which("ffmpeg") or not os.path.exists(CHROME):
        sys.exit("needs ffmpeg and Google Chrome")
    blocks = parse(Path(a.story))
    WORK.mkdir(parents=True, exist_ok=True)
    shows = [i for i, b in enumerate(blocks) if b.get("kind") not in ("intro", "outro")]
    picked = [a.only] if a.only is not None else list(range(len(blocks)))

    # The recorder: one radio, every picked radio block's setup and script in turn.
    rec = [i for i in picked if i in shows]
    if rec:
        subprocess.run(["cargo", "build", "-q", "--release", "-p", "ferrite-sim", "--bin", "ferrite-sim-record"], cwd=ROOT, check=True)
        args = [str(WORK / "frames")]
        for i in rec:
            for part in ("setup", "script"):
                f = WORK / f"{i:02}.{part}"
                f.write_text(blocks[i].get(part, "").replace(";", "\n"))
                args.append(str(f))
        subprocess.run([str(ROOT / "target" / "release" / "ferrite-sim-record"), *args], cwd=ROOT, check=True)

    clips = []
    for i in picked:
        b = blocks[i]
        png = WORK / f"card{i:02}.png"
        n = shows.index(i) + 1 if i in shows else 0
        card(b, n, len(shows), png)
        clip = WORK / f"clip{i:02}.mp4"
        if i in shows:
            raw = WORK / "frames" / f"{rec.index(i):02}.rgb"
            x, y = SCREEN_AT[b.get("layout", "center")]
            hold = float(b.get("hold", HOLD))
            secs = raw.stat().st_size / (240 * 320 * 3) / FPS + hold
            # The keys pressed, as chips under the screen (`keys: off` hides them).
            chips = [] if b.get("keys", "on") == "off" else key_events(WORK / "frames" / f"{rec.index(i):02}.keys")
            inputs, chain = [], f"[1]tpad=stop_mode=clone:stop_duration={hold},scale={240 * SCALE}:{320 * SCALE}:flags=neighbor[s];[0][s]overlay={x}:{y}:shortest=1[v0]"
            for c, (t0, t1, toks) in enumerate(chips):
                cp = WORK / f"chip{i:02}_{c:02}.png"
                cw, _ = chip_png(toks, cp)
                inputs += ["-loop", "1", "-framerate", str(FPS), "-i", str(cp)]
                cx = x + SW // 2 - cw // 2
                chain += f";[v{c}][{c + 2}]overlay={cx}:{y + SH + PAD + 26}:shortest=1:enable='between(t,{t0:.2f},{t1:.2f})'[v{c + 1}]"
            chain += f";[v{len(chips)}]fade=t=in:st=0:d={FADE},fade=t=out:st={secs - FADE:.2f}:d={FADE},format=yuv420p"
            ffmpeg("-loop", "1", "-framerate", str(FPS), "-i", str(png),
                   "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", "240x320", "-framerate", str(FPS), "-i", str(raw),
                   *inputs, "-filter_complex", chain,
                   "-r", str(FPS), "-c:v", "libx264", "-crf", "12", "-tune", "animation", "-preset", "slow", str(clip))
        else:
            secs = float(b.get("seconds", "3"))
            ffmpeg("-loop", "1", "-framerate", str(FPS), "-i", str(png), "-t", str(secs),
                   "-vf", f"fade=t=in:st=0:d={FADE},fade=t=out:st={secs - FADE:.2f}:d={FADE},format=yuv420p",
                   "-r", str(FPS), "-c:v", "libx264", "-crf", "12", "-tune", "animation", "-preset", "slow", str(clip))
        clips.append(clip)
        print(f"block {i}: {b.get('title', '')} ({secs:.1f} s)")

    lst = WORK / "clips.txt"
    lst.write_text("".join(f"file '{c}'\n" for c in clips))
    out = Path(a.out) if a.only is None else WORK / f"block{a.only:02}.mp4"
    silent = WORK / "silent.mp4"
    ffmpeg("-f", "concat", "-safe", "0", "-i", str(lst), "-c", "copy", str(silent))
    # Music: a `music:` file from the storyboard (relative to it), licensed for this use; none: a silent video.
    length = float(subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", str(silent)],
                                  capture_output=True, text=True, check=True).stdout)
    music = next((b["music"] for b in blocks if b.get("music")), "")
    if not music:
        shutil.copy(silent, out)
        print(f"wrote {out} (no music)")
        return
    if not Path(music).is_absolute():
        music = str(Path(a.story).resolve().parent / music)
    ffmpeg("-i", str(silent), "-stream_loop", "-1", "-i", music, "-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k",
           "-af", f"afade=t=in:d=0.5,afade=t=out:st={length - 2:.2f}:d=2", "-t", f"{length:.2f}", "-movflags", "+faststart", str(out))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
