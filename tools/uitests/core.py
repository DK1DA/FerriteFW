"""The on-radio UI test framework (tools/uitest.py runs it): tests, the test context, the screen readback.

A test is a function taking the context `t`, tagged with the manual rule IDs it checks:

    from .core import test

    @test("MENU-1", "MENU-13")
    def quick_settings_rows(t):
        t.home()
        u = t.keys("MENU")              # keys, then the screen read back (a U)
        t.eq(u.id, "Quick", "MENU on home opens quick settings")
        t.check(len(u.rows) == 10, "nine rows and Advanced", u.rows)

`where="radio"` marks a test the simulator can't model (it is skipped there); `slow=True` one that waits on a timer
(run with --slow). Safe by construction: the context has no PTT and the link's remote keys never transmit
(LINK-19); a setting a test changes is put back by the runner after it (from the snapshot taken at the start), and
`t.set()` only takes settings outside UNSAFE.
"""
import re
import time

REGISTRY = []

# Settings a test never changes: they could transmit (VOX, beacon, digipeat, CW ID), take the cable away from the
# link (cable port, KISS, cable log, terminal colour) or the radio off (auto power off), or need the radio's owner.
UNSAFE = {"0 8 2 1", "0 5 5 1", "0 5 5 3", "0 5 8 1", "0 8 7 2", "0 9 2 4", "0 5 9 1", "0 9 2 6", "0 9 2 5",
          "0 7 8", "0 9 2 1", "0 9 3 1", "0 9 3 2"}


class Fail(Exception):
    pass


class Skip(Exception):
    pass


class Test:
    def __init__(self, fn, rules, where, slow, needs, smoke, xfail):
        self.fn, self.rules, self.where, self.slow, self.needs, self.smoke = fn, list(rules), where, slow, needs, smoke
        self.xfail = xfail
        self.name = fn.__name__
        self.module = fn.__module__.rsplit(".", 1)[-1]
        self.doc = (fn.__doc__ or "").strip().split("\n")[0]


def test(*rules, where="both", slow=False, needs=None, smoke=False, xfail=None):
    """Register a test: the rule IDs it checks; where it runs (both, radio, sim); slow (waits on timers);
    needs: the build flavours it needs (("tx", "tx-dry") for a transmit build's screens), else any; smoke: in
    the short set bench.py runs beside the SDR suite; xfail: a known deviation from the rule (the reason): a
    failure is reported as xfail (not a run failure, recorded ✗), a pass as xpass (fixed: drop the mark)."""
    def deco(fn):
        REGISTRY.append(Test(fn, rules, where, slow, needs, smoke, xfail))
        return fn
    return deco


class U:
    """One readback of the screen (ferrite_link.parse_ui's dict) with short names."""

    def __init__(self, d):
        self.d = d
        sc = d.get("screen", {})
        self.id, self.name, self.title = sc.get("id", ""), sc.get("name", ""), sc.get("title", "")
        self.rows, self.sel, self.hints = d["rows"], d["sel"], d["hints"]
        self.toast, self.popup, self.qmenu = d["toast"], d["popup"], d["qmenu"]
        self.bar, self.state = d.get("bar", {}), d.get("state", {})
        self.cards = {c["slot"]: c for c in d["cards"]}
        self.lines = d["lines"]

    def __getitem__(self, k):
        return self.d.get(k)

    def row(self, name):
        for r in self.rows:
            if r["name"] == name:
                return r
        return None

    def num(self, n):
        for r in self.rows:
            if r["num"] == n:
                return r
        return None

    def names(self):
        return [r["name"] for r in self.rows]

    @property
    def active(self):
        return self.cards.get(self.state.get("active", "A"), {})

    def hint_keys(self):
        return [k for k, _ in self.hints]

    def hint(self, key):
        for k, v in self.hints:
            if k == key:
                return v
        return None

    def text(self):
        return "\n".join(self.lines)


def mhz_of(s):
    return float(s) if s not in (None, "") else None


GET = re.compile(r"^(\d(?: \d)*) ([^:]+?): (.*?)( \(not the default\))?\s*$")


class T:
    """The context a test gets: the radio, key presses, the readback, checks with evidence, settings."""

    def __init__(self, radio, sim, build, log=print):
        self.r, self.sim, self.build, self.log = radio, sim, build, log
        self.checks, self.notes, self.last = [], [], None
        self.evidence = []  # the readbacks a check looked at (their lines), newest last

    # -- keys and readback --
    def ui(self, settle=0.12):
        if settle:
            time.sleep(settle)
        self.last = U(self.r.ui())
        return self.last

    def keys(self, seq, settle=0.12):
        self.r.keys(seq)
        return self.ui(settle)

    def press(self, name, long=False, settle=0.12):
        self.r.press(name, long=long)
        return self.ui(settle)

    def knob(self, n, settle=0.12):
        self.r.knob(n)
        return self.ui(settle)

    def wait(self, pred, timeout=3.0, what="the screen", every=0.15):
        end = time.time() + timeout
        while True:
            u = self.ui(every)
            if pred(u):
                return u
            if time.time() > end:
                self.fail(f"{what}: not within {timeout:g} s")

    def home(self):
        """Home, unlocked, no quick menu: hold BACK (HOME-2), then BACK x4 if that wasn't enough."""
        u = self.ui(0)
        if u.bar.get("lock"):
            self.r.press("HASH", long=True)
        for _ in range(2):
            u = self.press("BACK", long=True)
            if u.id == "Home" and not u.qmenu:
                return u
        for _ in range(4):
            u = self.press("BACK")
        if u.id != "Home":
            self.fail(f"can't get home (on {u.id})")
        return u

    def unmute(self):
        """The active slot unmuted (HOME-15's quick mute isn't in `slot`): True if it was muted (put it back with
        STAR on home)."""
        u = self.ui(0)
        if "muted" in str(u.active.get("state", "")).lower():
            self.press("STAR")
            return True
        return False

    def stop_scans(self, keep=()):
        """Stop the scans running on the slots (but those in `keep`), so their audio and their scan stops leave
        the other cards alone (the simulator's sample scans C). Returns the slots stopped: give them to
        `restart_scans` at the end (the runner's restore doesn't compare "scanning")."""
        stopped = []
        active = self.ui(0).state.get("active", "A")
        for L in "ABC":
            if L in keep or "scanning" not in self.slot(L):
                continue
            self.cmd(f"slot {L} active")
            self.home()
            self.press("SIDE2", settle=0.3)
            self.home()
            if "scanning" not in self.slot(L):
                stopped.append(L)
        self.cmd(f"slot {active} active")
        self.home()
        return stopped

    def restart_scans(self, slots):
        """SIDE2 on each slot `stop_scans` stopped, then the active slot as it was."""
        if not slots:
            return
        active = self.ui(0).state.get("active", "A")
        for L in slots:
            self.cmd(f"slot {L} active")
            self.home()
            self.press("SIDE2", settle=0.3)
            self.home()
        self.cmd(f"slot {active} active")
        self.home()

    def put_back(self, stopped):
        """The slots as at the run's start (the runner's snapshot), then the scans `stop_scans` stopped."""
        notes = []
        self.home()
        self.state.restore_slots(notes)
        self.restart_scans(stopped)
        for n in notes:
            self.note(n)

    def screen(self, want, u=None):
        u = u or self.last or self.ui()
        self.eq(u.id, want, "the screen")
        return u

    # -- checks --
    def check(self, cond, what, got=None):
        self.checks.append(dict(what=what, ok=bool(cond), got=None if got is None else str(got)[:300]))
        if self.last is not None and (not self.evidence or self.evidence[-1] is not self.last.lines):
            self.evidence.append(self.last.lines)
            del self.evidence[:-6]
        if not cond:
            raise Fail(f"{what}" + (f": got {str(got)[:300]}" if got is not None else ""))

    def eq(self, got, want, what):
        self.check(got == want, f"{what} is {want!r}", got)

    def has(self, hay, needle, what):
        self.check(needle in (hay or ""), f"{what} has {needle!r}", hay)

    def fail(self, what):
        self.check(False, what)

    def note(self, s):
        self.notes.append(s)

    # -- terminal commands and settings --
    def cmd(self, line):
        return self.r.cmd(line)

    def cmd_err(self, line):
        """(ok, text): the output, or the radio's refusal reason."""
        from ferrite_link import RadioError
        try:
            return True, self.r.cmd(line)
        except RadioError as e:
            return False, str(e)

    def get(self, path):
        out = self.cmd(f"get {path}").strip()
        m = GET.match(out)
        if not m:
            self.fail(f"get {path}: can't read {out!r}")
        return m.group(3)

    def get_full(self, path):
        """(name, value, non-default)."""
        m = GET.match(self.cmd(f"get {path}").strip())
        return m.group(2), m.group(3), bool(m.group(4))

    def set(self, path, value):
        if path in UNSAFE:
            raise Fail(f"{path} is never changed by a test")
        ok, out = self.cmd_err(f"set {path} {value}")
        if not ok:
            self.fail(f"set {path} {value}: {out}")

    def slot(self, letter):
        return self.cmd(f"slot {letter}").strip()

    def need(self, cond, why):
        """A precondition the radio's state doesn't meet: the test is skipped, not failed."""
        if not cond:
            raise Skip(why)
