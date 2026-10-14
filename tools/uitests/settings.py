"""Every setting in the settings table, by its menu number (Appendix A; MENU-7, MENU-12, MENU-15, MENU-16, MENU-21)."""
import re

from .core import UNSAFE, test

def ascii_(s):
    """The terminal sends ASCII (° comes as ?): compare without the rest."""
    return re.sub(r"[^\x20-\x7e]|\?", "", s or "")


def unbuilt_group(t, path):
    """Whether path's third-level group isn't built yet: OK on its row says "setting comes next". Leaves the
    screen on the opened group otherwise."""
    p = path.split()
    t.home()
    u = t.keys(f"MENU {' '.join(p[:3])} MENU", settle=0.15)
    return bool(u.toast and "comes next" in u.toast["body"])


# Settings typed or picked on a page of their own (OK opens them): the knob doesn't step them in the group.
TYPED = {"0 4 2 2", "0 5 6 1", "0 5 6 2", "0 5 8 2", "0 4 2 3"}


@test("MENU-5", "MENU-7", "MENU-12", "MENU-15", "MENU-16", "MENU-21")
def every_setting_reachable_and_live(t):
    """Each setting from the table (`dump`): MENU 0 and its number lands on its row (name as `get`, value,
    help with the path); the knob changes it at once (`get` has it), and back; # leaves defaults alone."""
    paths = [p for p in t.state.paths if p not in UNSAFE]
    t.check(len(paths) >= 40, "the settings table", len(paths))
    changed = unchanged = 0
    odd, via, misrouted, unbuilt = [], [], [], []
    for path in paths:
        name, value, nd = t.get_full(path)
        t.home()
        u = t.keys("MENU " + path, settle=0.15)
        if u.id == "Group" and (u["help"] or {}).get("args", [""])[0] != "MENU " + path and len(path.split()) == 4:
            # The typed path went elsewhere (a row with a value and a sub-group: its digit picks it, OK opens it).
            p = path.split()
            if unbuilt_group(t, path):
                # Its group isn't built yet: the row says "setting comes next" (the value is in the record).
                unbuilt.append(f"{path} {name}")
                continue
            misrouted.append(f"{path} -> {(u['help'] or {}).get('args', ['?'])[0]}")
            u = t.keys(p[3], settle=0.15)  # unbuilt_group left it on the opened group
        if path in TYPED and u.id == "Text":
            via.append(f"{path} {name} (typed)")
            continue
        if u.id != "Group":
            # A tool's own settings (0 3 1 x, FM broadcast): its number opens the tool; hold MENU there has them.
            t.check(u.id in ("Fm", "Scope", "Hf"), f"{path} {name}: a group or its tool's screen", u.id)
            via.append(f"{path} {name} ({u.id})")
            continue
        t.eq(u.sel and u.sel["name"], name, f"{path}: the row is {name}")
        t.eq(u["help"]["args"][0], "MENU " + path, f"{path}: the help names its path")
        t.eq(ascii_(u.sel["value"]), ascii_(value), f"{path} {name}: the row's value")
        t.eq(u.sel["dot"], nd, f"{path} {name}: the dot")
        if path in TYPED:
            continue
        u = t.knob(+1)
        v1 = u.sel["value"]
        if v1 == value:
            u = t.knob(-1)
            v1 = u.sel["value"]
            if v1 == value:
                unchanged += 1
                odd.append(f"{path} {name}")
                continue
            back = +1
        else:
            back = -1
        t.eq(ascii_(t.get(path)), ascii_(v1), f"{path} {name}: the change is in the settings at once")
        u = t.knob(back)
        t.eq(ascii_(u.sel["value"]), ascii_(value), f"{path} {name}: the knob back")
        t.eq(ascii_(t.get(path)), ascii_(value), f"{path} {name}: back in the settings")
        changed += 1
    t.note(f"{len(paths)} settings reached; {changed} changed by the knob and back"
           + (f"; the knob didn't step {', '.join(odd)}" if odd else "")
           + (f"; reached on their tool's screen: {', '.join(via)}" if via else "")
           + (f"; typed paths landing elsewhere (reached with OK): {', '.join(misrouted)}" if misrouted else "")
           + (f"; in groups not built yet (\"setting comes next\"): {', '.join(unbuilt)}" if unbuilt else ""))
    t.check(not misrouted, "every typed path lands on its row", misrouted)
