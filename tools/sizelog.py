#!/usr/bin/env python3
"""Size check after every build (Alex, Oct 27, 2026: the flash audit found the image growing ~11 KB a day in small steps).

    python3 tools/sizelog.py ferrite-tx        # tools/mkbtf.sh runs it with the image's name after each build

Prints the image, fast and slow flash against the last recorded build of the same name, and warns when the image
grew more than WARN_KB since then. On main with a clean tree it records the build in tools/size-history.tsv (one row
per commit and image), so every merge leaves its sizes behind and the build report can show the trend. Commit that
file with the merge. Sizes come from the release ELF as buildreport.py reads them.
"""
import datetime, os, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HISTORY = os.path.join(ROOT, "tools/size-history.tsv")
HEAD = "date\tcommit\timage\tbytes\tfast\tslow"
WARN_KB = 4

sys.path.insert(0, os.path.join(ROOT, "tools"))
from buildreport import elf_sizes, kb, run  # noqa: E402


def rows():
    if not os.path.exists(HISTORY):
        return []
    lines = open(HISTORY).read().splitlines()[1:]
    return [dict(zip(HEAD.split("\t"), line.split("\t"))) for line in lines if line.strip()]


def main():
    name = sys.argv[1]
    m = elf_sizes()
    commit = run("git", "rev-parse", "--short", "HEAD").stdout.strip()
    branch = run("git", "rev-parse", "--abbrev-ref", "HEAD").stdout.strip()
    dirty = bool(run("git", "status", "--porcelain", "--untracked-files=no", "--", ".", ":!tools/size-history.tsv").stdout.strip())
    last = next((r for r in reversed(rows()) if r["image"] == name and r["commit"] != commit), None)
    line = f"size: {name} {kb(m['image'])} (fast {kb(m['fast'])}, slow {kb(m['slow'])})"
    if last:
        d = m["image"] - int(last["bytes"])
        df, ds = m["fast"] - int(last["fast"]), m["slow"] - int(last["slow"])
        line += f", {d / 1024:+.1f} KB since {last['commit']} (fast {df / 1024:+.1f}, slow {ds / 1024:+.1f})"
        print(line)
        if d > WARN_KB * 1024:
            print(f"SIZE WARNING: {name} grew {d / 1024:.1f} KB since {last['commit']}. Find why before merging: "
                  "tools/sizemap.py --json new.json, then --diff against a map of the older build.")
    else:
        print(line + ", no earlier record")
    if branch == "main" and not dirty:
        old = [r for r in rows() if not (r["image"] == name and r["commit"] == commit)]
        old.append(dict(date=datetime.date.today().isoformat(), commit=commit, image=name, bytes=str(m["image"]),
                        fast=str(m["fast"]), slow=str(m["slow"])))
        with open(HISTORY, "w") as f:
            f.write(HEAD + "\n" + "".join("\t".join(r[k] for k in HEAD.split("\t")) + "\n" for r in old))


if __name__ == "__main__":
    main()
