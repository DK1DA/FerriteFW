//! TERM-47: the terminal without its pages (decision 342), a plain command line: the build without the `vt100`
//! feature, or one with it whose template part is missing (UPD-21; `NO_VT` here).
#![cfg_attr(not(feature = "line-cmds"), allow(unused))] // helpers for the tests gated on the line commands

use super::*;
use crate::sample::Sample;
use std::string::String;
use std::vec::Vec;

struct Out(Vec<u8>);

impl Sink for Out {
    fn room(&self) -> usize {
        1 << 20
    }
    fn put(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
}

struct Rig {
    app: App,
    store: Sample,
    term: Term,
    out: Out,
    dev: Device,
    now: u32,
}

impl Rig {
    fn new() -> Self {
        super::NO_VT.with(|v| v.set(true));
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        Rig { app, store: Sample::new(), term: Term::new(), out: Out(Vec::new()), dev: Device::default(), now: 1000 }
    }

    /// Bytes typed, then polls; what went out since.
    fn send(&mut self, b: &[u8]) -> String {
        let at = self.out.0.len();
        for &c in b {
            while !self.term.ready() {
                self.poll();
            }
            self.term.feed(&mut self.app, c, self.now, &self.store, &self.dev, &Default::default());
            while let Some(w) = self.app.pop_write() {
                self.store.apply(w);
            }
        }
        for _ in 0..4 {
            self.poll();
        }
        String::from_utf8_lossy(&self.out.0[at..]).into()
    }

    fn poll(&mut self) {
        self.now += 100;
        self.term.poll(&mut self.app, &self.store, &self.dev, &LiveAll::default(), self.now, &mut self.out);
        while let Some(w) = self.app.pop_write() {
            self.store.apply(w);
        }
        while self.app.pop_cmd().is_some() {}
    }
}

/// TERM-47: Enter starts it with a banner and a prompt; a line runs as a command and its answer follows; a leading `:`
/// is taken; lists stream, then the prompt comes back; Ctrl-C leaves.
#[cfg(feature = "line-cmds")]
#[test]
fn command_line_only() {
    let mut r = Rig::new();
    let s = r.send(b"\r");
    assert!(s.contains("terminal: type a command") && s.ends_with("> "), "{s:?}");
    assert!(!s.contains("\x1b[2J"), "no full-screen page");
    let s = r.send(b"set 0 7 1 Daylight\r");
    assert!(s.contains("  0 7 1 Theme: Daylight\r\n"), "{s:?}");
    let s = r.send(b":frob\r");
    assert!(s.contains("frob: no such command; help lists them"), "{s:?}");
    let s = r.send(b"help\r");
    assert!(s.contains("Commands: type : then one of these") && s.ends_with("> "), "{s:?}");
    let s = r.send(b"ch 1x\x7f2\r");
    assert!(s.contains("ch 12 \""), "Backspace edits the line: {s:?}");
    let s = r.send(b"\x03");
    assert!(s.contains("Ferrite terminal closed"), "{s:?}");
    assert!(!r.term.on());
}

/// TERM-12, SAT-3, TERM-35 (coverage of the command line's edges): `sat` with nothing held, `sat del` and its usage,
/// `sat add` taking a TLE with an alpha-5 catalogue number and refusing a CSV line that isn't OMM; `get` with every
/// setting at its default; `quick` with too much; `knob` past 20 and while keyed; `slot` naming a scan.
#[cfg(feature = "line-cmds")]
#[test]
fn command_line_edges() {
    let mut r = Rig::new();
    r.send(b"\r");
    for (line, want) in [
        ("sat del 5", "No satellite at 5"),
        ("sat del", "sat del N"),
        ("sat frob", "sat lists them"),
        ("quick 1 hello there extra", "quick"),
        ("knob 30", "knob"),
    ] {
        let s = r.send(std::format!("{line}\r").as_bytes());
        assert!(s.contains(want), "{line}: {s:?}");
    }
    // SAT-3: a CSV line that isn't OMM is refused with its reason; a TLE with an alpha-5 catalogue number (A0001 is
    // 100001) parses and goes to the store (the RAM sample has none: busy, paste again)
    let s = r.send(b"sat add\r");
    assert!(s.contains("Paste satellites"), "{s:?}");
    let s = r.send(b"foo,bar,baz\r");
    assert!(s.contains("not OMM CSV"), "{s:?}");
    let s = r.send(b"ALPHA TEST\r1 A0001U 02058C   26304.82911449  .00000904  00000+0  12403-3 0  9994\r2 A0001  64.5527  66.1378 0068979 237.1936 122.2515 14.83296838284726\r");
    assert!(s.contains("busy: paste it again") && !s.contains("catalogue number"), "{s:?}");
    let s = r.send(b"end\r");
    assert!(s.contains("Done: 4 lines read, 0 applied, 2 refused."), "{s:?}");
    // TERM-12: get lists the settings away from their defaults, and says when there are none
    let s = r.send(b"get\r");
    assert!(s.contains("All settings are at their defaults."), "{s:?}");
    r.send(b"set 0 7 7 2 Off\r");
    let s = r.send(b"get\r");
    assert!(s.contains("set 0 7 7 2 Off"), "{s:?}");
    // LINK-19: a remote knob step while keyed is refused with the reason
    r.app.radio.keyed = Some(0);
    let s = r.send(b"knob +2\r");
    assert!(s.contains("knob: transmitting"), "{s:?}");
    r.app.radio.keyed = None;
    // TERM-12: a channel's name as name=WORD
    let s = r.send(b"ch 1 name=Club\r");
    assert!(s.contains("Channel 1 saved"), "{s:?}");
    // TERM-35: slot names a scan
    r.app.scan_mask = 1;
    let s = r.send(b"slot A\r");
    assert!(s.contains(" scanning"), "{s:?}");
}
