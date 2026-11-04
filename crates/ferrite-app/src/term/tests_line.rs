//! The terminal without its pages (decision 342): a plain command line.

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
            self.term.feed(&mut self.app, c, self.now, &self.store);
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

/// Enter starts it with a banner and a prompt; a line runs as a command and its answer follows; a leading `:`
/// is taken; lists stream, then the prompt comes back; Ctrl-C leaves.
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
