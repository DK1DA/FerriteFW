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
SCALE = 3
SCREEN_AT = (600, 60)  # the 720 x 960 screen's top-left on the card
FADE = 0.4


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
        body = (
            f'<div class="col left"><div class="kick">Ferrite · {n} of {total}</div><h1>{html.escape(b.get("title", ""))}</h1>{paras(b.get("left", ""))}</div>'
            f'<div class="bezel"></div><div class="col right">{paras(b.get("right", ""))}</div>'
            f'<div class="n">RT-950 PRO · SIMULATOR, THE FIRMWARE\'S OWN SCREENS · DRAFT</div>'
        )
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
            secs = raw.stat().st_size / (240 * 320 * 3) / FPS
            ffmpeg("-loop", "1", "-framerate", str(FPS), "-i", str(png),
                   "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", "240x320", "-framerate", str(FPS), "-i", str(raw),
                   "-filter_complex",
                   f"[1]scale={240 * SCALE}:{320 * SCALE}:flags=neighbor[s];[0][s]overlay={SCREEN_AT[0]}:{SCREEN_AT[1]}:shortest=1,"
                   f"fade=t=in:st=0:d={FADE},fade=t=out:st={secs - FADE:.2f}:d={FADE},format=yuv420p",
                   "-r", str(FPS), "-c:v", "libx264", "-crf", "16", "-preset", "slow", str(clip))
        else:
            secs = float(b.get("seconds", "3"))
            ffmpeg("-loop", "1", "-framerate", str(FPS), "-i", str(png), "-t", str(secs),
                   "-vf", f"fade=t=in:st=0:d={FADE},fade=t=out:st={secs - FADE:.2f}:d={FADE},format=yuv420p",
                   "-r", str(FPS), "-c:v", "libx264", "-crf", "16", "-preset", "slow", str(clip))
        clips.append(clip)
        print(f"block {i}: {b.get('title', '')} ({secs:.1f} s)")

    lst = WORK / "clips.txt"
    lst.write_text("".join(f"file '{c}'\n" for c in clips))
    out = Path(a.out) if a.only is None else WORK / f"block{a.only:02}.mp4"
    ffmpeg("-f", "concat", "-safe", "0", "-i", str(lst), "-c", "copy", "-movflags", "+faststart", str(out))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
