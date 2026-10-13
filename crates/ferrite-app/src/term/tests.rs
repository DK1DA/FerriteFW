//! The terminal against a small VT100 model: what a terminal would show after the bytes it was sent.

use ufmt::uwrite;
use super::*;
use crate::sample::Sample;
use std::string::String;
use std::vec;
use std::vec::Vec;

/// A VT100 screen: the characters, which cells are reverse video, the cursor; plus every byte, and how much
/// the sink takes at once.
struct Vt {
    cols: usize,
    rows: usize,
    cells: Vec<Vec<u8>>,
    rev: Vec<Vec<bool>>,
    cur: (usize, usize),
    saved: (usize, usize),
    attr_rev: bool,
    raw: Vec<u8>,
    room: usize,
    asked_size: bool,
}

impl Vt {
    fn new(cols: usize, rows: usize) -> Self {
        Vt {
            cols,
            rows,
            cells: vec![vec![b' '; cols]; rows],
            rev: vec![vec![false; cols]; rows],
            cur: (0, 0),
            saved: (0, 0),
            attr_rev: false,
            raw: Vec::new(),
            room: 1 << 20,
            asked_size: false,
        }
    }

    fn line(&self, r: usize) -> String {
        String::from_utf8_lossy(&self.cells[r]).trim_end().into()
    }

    fn text(&self) -> String {
        (0..self.rows).map(|r| self.line(r)).collect::<Vec<_>>().join("\n")
    }

    fn has(&self, s: &str) -> bool {
        self.text().contains(s)
    }

    /// The row holding `s`.
    fn row_of(&self, s: &str) -> Option<usize> {
        (0..self.rows).find(|&r| self.line(r).contains(s))
    }

    fn apply(&mut self, b: &[u8]) {
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c == 0x1B {
                let rest = &b[i + 1..];
                match rest.first() {
                    Some(b'7') => {
                        self.saved = self.cur;
                        i += 2;
                        continue;
                    }
                    Some(b'8') => {
                        self.cur = self.saved;
                        i += 2;
                        continue;
                    }
                    Some(b'[') => {
                        let end = rest.iter().position(|c| c.is_ascii_alphabetic()).expect("a CSI final byte");
                        let params = core::str::from_utf8(&rest[1..end]).unwrap();
                        let n: Vec<usize> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
                        match rest[end] {
                            b'H' => {
                                let r = n.first().copied().unwrap_or(1).max(1);
                                let c = n.get(1).copied().unwrap_or(1).max(1);
                                self.cur = ((r - 1).min(self.rows - 1), (c - 1).min(self.cols - 1));
                            }
                            b'J' => {
                                for r in 0..self.rows {
                                    self.cells[r].fill(b' ');
                                    self.rev[r].fill(false);
                                }
                            }
                            b'K' => {
                                let (r, c) = self.cur;
                                if c < self.cols {
                                    self.cells[r][c..].fill(b' ');
                                    self.rev[r][c..].fill(false);
                                }
                            }
                            b'm' => {
                                for p in &n {
                                    match p {
                                        0 => self.attr_rev = false,
                                        7 => self.attr_rev = true,
                                        _ => {}
                                    }
                                }
                            }
                            b'n' => self.asked_size = true,
                            other => panic!("unexpected CSI {}", other as char),
                        }
                        i += 2 + end;
                        continue;
                    }
                    _ => panic!("unexpected ESC sequence"),
                }
            }
            match c {
                b'\r' => self.cur.1 = 0,
                b'\n' => self.cur.0 = (self.cur.0 + 1).min(self.rows - 1),
                0x20..=0x7E => {
                    let (r, col) = self.cur;
                    if col < self.cols {
                        self.cells[r][col] = c;
                        self.rev[r][col] = self.attr_rev;
                        self.cur.1 += 1;
                    }
                }
                _ => panic!("unexpected byte {c:#x}"),
            }
            i += 1;
        }
    }
}

impl Sink for Vt {
    fn room(&self) -> usize {
        self.room
    }
    fn put(&mut self, b: &[u8]) {
        assert!(b.len() <= self.room, "the sink was overfilled");
        self.room -= b.len();
        self.raw.extend_from_slice(b);
        self.apply(b);
    }
}

/// A store the rig can hand channel writes to.
trait Apply {
    fn apply(&self, w: ChanWrite);
}

impl Apply for Sample {
    fn apply(&self, w: ChanWrite) {
        Sample::apply(self, w);
    }
}

struct Rig<S = Sample> {
    app: App,
    store: S,
    term: Term,
    vt: Vt,
    dev: Device,
    live: LiveAll,
    now: u32,
}

impl Rig<Sample> {
    fn new() -> Self {
        Rig::with(Sample::new())
    }

    /// Started and on the main menu.
    fn main() -> Self {
        let mut r = Rig::new();
        r.send(b"\r\r");
        assert_eq!(r.term.page(), Page::Main);
        r
    }
}

impl<S: ChannelStore + Apply> Rig<S> {
    fn with(store: S) -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        let dev = Device { battery_mv: 7900, clock_min: Some(14 * 60 + 32), ..Default::default() };
        Rig { app, store, term: Term::new(), vt: Vt::new(80, 24), dev, live: LiveAll::default(), now: 1000 }
    }

    fn send(&mut self, b: &[u8]) {
        for &c in b {
            // A whole line waits for the next poll to run it (TERM-13): the cable holds the rest meanwhile.
            while !self.term.ready() {
                self.poll();
            }
            self.term.feed(&mut self.app, c, self.now, &self.store);
            while let Some(w) = self.app.pop_write() {
                self.store.apply(w);
            }
        }
        self.poll();
    }

    fn poll(&mut self) {
        self.now += 100;
        self.term.poll(&mut self.app, &self.store, &self.dev, &self.live, self.now, &mut self.vt);
        while let Some(w) = self.app.pop_write() {
            self.store.apply(w);
        }
        while self.app.pop_cmd().is_some() {}
    }

    /// A command typed after `:`, run.
    fn cmd(&mut self, c: &str) {
        self.send(b":");
        self.send(c.as_bytes());
        self.send(b"\r");
        self.poll();
        self.poll();
    }

    fn bottom(&self) -> String {
        self.vt.line(self.vt.rows - 1)
    }

    /// Everything after the console started (the raw bytes as text).
    fn raw(&self) -> String {
        String::from_utf8_lossy(&self.vt.raw).into()
    }

    /// `:load`, the text line by line (CR LF), then polls until the console is done.
    fn load(&mut self, text: &str) {
        self.cmd("load");
        assert!(matches!(self.term.con, Con::Run(Body::Load { .. })), "{:?}", self.term.con);
        for l in text.lines() {
            self.send(l.as_bytes());
            self.send(b"\r\n");
        }
        for _ in 0..8 {
            self.poll();
        }
        assert!(matches!(self.term.con, Con::Done | Con::Off), "the load ended: {:?}", self.term.con);
    }
}

/// The dump as lines.
fn dump<S: ChannelStore>(app: &App, st: &S, dev: &Device) -> Vec<String> {
    let mut p = cmd::DumpPos::default();
    let mut l = cmd::Line::new();
    let mut v = Vec::new();
    while cmd::dump_line(app, st, dev, &mut p, &mut l) {
        v.push(String::from_utf8_lossy(l.as_bytes()).into());
    }
    v
}

/// TERM-2: in auto nothing answers until a typed Enter; then the banner (TERM-20) under the top line (TERM-11).
#[test]
fn auto_starts_on_enter_with_the_banner() {
    let mut r = Rig::new();
    r.send(b"hello");
    assert_eq!(r.term.page(), Page::Off);
    assert!(r.vt.raw.is_empty(), "silent until Enter");
    r.send(b"\r");
    assert_eq!(r.term.page(), Page::Splash);
    assert!(r.vt.asked_size, "asks the terminal's size (TERM-15)");
    let top = r.vt.line(0);
    assert!(top.starts_with(" Ferrite 0.1.0") && top.contains("RT-950 Pro") && top.contains("cable 115200"), "{top}");
    assert!(top.ends_with("14:32"), "the clock at the far right: {top}");
    assert!(r.vt.rev[0].iter().all(|&v| v), "the top line is reverse video, full width");
    assert!(r.vt.has("FERRITE 0.1.0 - open firmware for the RT-950 Pro"));
    assert!(r.vt.has("Press Enter for the menu, ? for help"));
    assert!(r.vt.line(23).contains("Enter Menu"), "keys on the bottom line");
    // No ASCII art: nothing but the banner's three lines in the body.
    let body: Vec<String> = (1..23).map(|i| r.vt.line(i)).filter(|l| !l.is_empty()).collect();
    assert_eq!(body.len(), 3, "{body:?}");
}

/// Link and KISS never start the terminal; Terminal starts it on any byte.
#[test]
fn cable_port_setting_decides() {
    let mut r = Rig::new();
    r.app.set.cable_port = 1;
    r.send(b"\r\r");
    assert_eq!(r.term.page(), Page::Off);
    r.app.set.cable_port = 3;
    r.send(b"\r");
    assert_eq!(r.term.page(), Page::Off);
    r.app.set.cable_port = 2;
    r.send(b"x");
    assert_eq!(r.term.page(), Page::Splash);
}

/// The main menu with the radio's state; digits jump; Esc back; Ctrl-C leaves and waits for Enter again.
#[test]
fn main_menu_and_leaving() {
    let mut r = Rig::main();
    assert!(r.vt.has("Ferrite terminal"));
    let n = r.store.count();
    assert!(r.vt.has(&std::format!("1  Channels      {n} used of 1000")), "{}", r.vt.text());
    assert!(r.vt.has("7  About"));
    assert!(r.vt.has("Radio now"));
    assert!(r.vt.has("Battery 7.9 V"));
    let sel = r.vt.row_of("1  Channels").unwrap();
    assert!(r.vt.rev[sel][2], "the selection is reverse video");
    r.send(b"\x1b[B"); // down
    let z = r.vt.row_of("2  Zones").unwrap();
    assert!(r.vt.rev[z][2] && !r.vt.rev[sel][2]);
    r.send(b"7");
    assert_eq!(r.term.page(), Page::About);
    r.send(b"\x1b");
    r.now += 100; // a lone ESC is Esc after a moment
    r.poll();
    assert_eq!(r.term.page(), Page::Main);
    r.send(b"\x03");
    assert_eq!(r.term.page(), Page::Off);
    assert!(r.vt.has("Ferrite terminal closed"));
    r.send(b"\r");
    assert_eq!(r.term.page(), Page::Splash);
}

/// TERM-19: nothing is sent again while nothing changed; Ctrl-L sends it all (TERM-16).
#[test]
fn only_changed_rows_are_sent() {
    let mut r = Rig::main();
    let before = r.vt.raw.len();
    r.now += 2000; // the refresh checks every row
    r.poll();
    assert_eq!(r.vt.raw.len(), before, "unchanged rows are not resent");
    r.dev.clock_min = Some(14 * 60 + 33);
    r.now += 2000;
    r.poll();
    let sent = r.vt.raw.len() - before;
    assert!(sent > 0 && sent < 200, "only the top line: {sent} bytes");
    assert!(r.vt.line(0).ends_with("14:33"));
    r.send(b"\x0c");
    assert!(r.vt.raw.len() - before > 1000, "Ctrl-L redraws everything");
}

/// TERM-4: a full sink holds rows back; later polls finish the page.
#[test]
fn never_overfills_the_sink() {
    let mut r = Rig::new();
    r.vt.room = 300;
    r.send(b"\r");
    for _ in 0..50 {
        r.vt.room = 300;
        r.poll();
    }
    assert!(!r.term.busy());
    assert!(r.vt.has("Press Enter for the menu"));
}

/// TERM-15: a larger terminal is used whole; the reply to the size question sets it.
#[test]
fn larger_terminal_from_the_size_report() {
    let mut r = Rig::new();
    r.vt = Vt::new(120, 40);
    r.send(b"\r");
    r.send(b"\x1b[40;120R");
    assert_eq!(r.term.size(), (120, 40));
    assert!(r.vt.line(0).ends_with("14:32") && r.vt.line(0).len() > 100, "{}", r.vt.line(0));
    assert!(r.vt.line(39).contains("Enter Menu"), "the bottom line on the last row");
    // Smaller than 80 x 24 counts as 80 x 24.
    r.send(b"\x1b[10;40R");
    assert_eq!(r.term.size(), (80, 24));
}

/// Help on ? and PF1, closed by Esc.
#[test]
fn help() {
    let mut r = Rig::main();
    r.send(b"?");
    assert!(r.vt.has("Help") && r.vt.has("Ctrl-L redraws"));
    r.send(b"?");
    assert!(!r.vt.has("Ctrl-L redraws"));
    r.send(b"\x1bOP"); // PF1
    assert!(r.vt.has("Ctrl-L redraws"));
}

/// Channels: the list with paging, then the editor; changes go through the App (TERM-6, TERM-9) and the
/// radio's slot on that channel follows.
#[test]
fn channels_list_and_edit() {
    let mut r = Rig::main();
    r.send(b"1");
    assert_eq!(r.term.page(), Page::Channels);
    assert!(r.vt.has("zone All channels"));
    assert!(r.vt.has("No") && r.vt.has("RX MHz") && r.vt.has("Flags"));
    let first = r.store.by_number(0).unwrap();
    let mut name = String::from_utf8_lossy(first.name()).into_owned();
    name.truncate(16);
    assert!(r.vt.has(&name), "{}", r.vt.text());
    // PgDn moves a page; Home comes back.
    r.send(b"\x1b[6~");
    assert!(r.term.chan_sel > 0);
    r.send(b"\x1b[H");
    assert_eq!(r.term.chan_sel, 0);
    // Edit the first channel: power up one step.
    r.send(b"\r");
    assert_eq!(r.term.page(), Page::Edit);
    assert!(r.vt.has(&std::format!("Channels > {} ", first.num + 1)));
    for _ in 0..8 {
        r.send(b"\x1b[B"); // down to Power (past Offset and TX)
    }
    assert_eq!(FIELDS[r.term.field as usize], F::It(Item::Power));
    r.send(b"\x1b[C");
    let after = r.store.by_num(first.num).unwrap().0;
    if band_of(first.rx) == Some(BAND_125M) {
        assert_eq!(after.cfg.power, first.cfg.power, "1.25 m power can't change (TERM-7)");
        assert!(r.vt.has("fixed at 0.25 W"));
    } else {
        assert_eq!(after.cfg.power, (first.cfg.power + 1) % 3, "saved in the store");
        assert!(r.vt.has("Saved"));
    }
}

/// A typed frequency is checked (CHAN-22, TERM-7); a typed name is saved; Esc cancels typing.
#[test]
fn typed_values() {
    let mut r = Rig::main();
    r.send(b"1\r");
    let num = r.term.edit.unwrap().num;
    // Name: field 0.
    r.send(b"\r");
    for _ in 0..20 {
        r.send(b"\x7f");
    }
    r.send(b"Test name\r");
    assert_eq!(r.store.by_num(num).unwrap().0.name(), b"Test name");
    // RX: field 2.
    r.send(b"\x1b[B\x1b[B");
    r.send(b"\r700\r");
    assert!(r.vt.has("756-1300 MHz"), "refused");
    r.send(b"\r146.52\r");
    assert_eq!(r.store.by_num(num).unwrap().0.rx, 14_652_000);
    assert!(r.vt.has("146.52000 MHz"));
    r.send(b"\r147");
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    assert_eq!(r.store.by_num(num).unwrap().0.rx, 14_652_000, "Esc cancels typing");
    assert_eq!(r.term.page(), Page::Edit);
}

/// A slot on the edited channel follows at once (TERM-9): the radio's screen shows the change.
#[test]
fn the_radio_follows_an_edit() {
    let mut r = Rig::main();
    // Put slot A on the first channel, as the radio would.
    let first = r.store.by_number(0).unwrap();
    {
        let s = &mut r.app.slots[0];
        (s.ch, s.zone, s.vfo_mode) = (Some(first), first.zone, false);
    }
    while r.app.pop_cmd().is_some() {}
    r.send(b"1\r");
    for _ in 0..9 {
        r.send(b"\x1b[B"); // Squelch
    }
    assert_eq!(FIELDS[r.term.field as usize], F::It(Item::Squelch));
    r.send(b"\x1b[C");
    let next = if first.cfg.squelch >= 9 { crate::slot::SQ_GLOBAL } else { first.cfg.squelch + 1 }; // 9, then Global
    assert_eq!(r.app.slot(0).cfg().squelch, next, "the slot's copy follows");
}

/// Zones: every channel, each zone, no zone; Enter lists that zone's channels and Esc comes back.
#[test]
fn zones() {
    let mut r = Rig::main();
    r.send(b"2");
    assert_eq!(r.term.page(), Page::Zones);
    assert!(r.vt.has("All channels"));
    let z0 = r.store.zone_name(0).unwrap();
    assert!(r.vt.has(core::str::from_utf8(z0.as_bytes()).unwrap()));
    r.send(b"\x1b[B\r");
    assert_eq!(r.term.page(), Page::Channels);
    assert_eq!(r.term.zone, 0);
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    assert_eq!(r.term.page(), Page::Zones);
}

/// Settings: the radio's groups and numbers; a wired setting changes the App (TERM-6); the region and PIN are
/// left to the radio (TERM-7).
#[test]
fn settings_same_numbers_and_validation() {
    let mut r = Rig::main();
    r.send(b"3");
    assert!(r.vt.has("1  Watch and scan") && r.vt.has("9  System"));
    // 0 7 1: theme.
    r.send(b"7");
    assert!(r.vt.has("Settings > 0 7 Display, sound, power"));
    assert!(r.vt.has("Path on the radio: MENU 0 7 1"));
    let theme = r.app.set.theme;
    r.send(b"\x1b[C");
    assert_eq!(r.app.set.theme, (theme + 1) % crate::settings::THEMES.len() as u8, "changed in the App");
    assert!(r.vt.has("* "), "marked as not the default");
    r.send(b"#");
    assert_eq!(r.app.set.theme, theme);
    // 0 9 4: the region stays on the radio.
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    r.send(b"9");
    let region = r.app.set.region;
    for _ in 0..3 {
        r.send(b"\x1b[B");
    }
    r.send(b"\x1b[C");
    assert_eq!(r.app.set.region, region);
    assert!(r.vt.has("Change this one on the radio"));
    // 0 9 2 5: the cable port itself.
    r.send(b"\x1b[A\x1b[A\r");
    assert!(r.vt.has("Cable port"));
    for _ in 0..4 {
        r.send(b"\x1b[B");
    }
    r.send(b"\x1b[C");
    assert_eq!(r.app.set.cable_port, 1);
    r.send(b"#");
    assert_eq!(r.app.set.cable_port, 0);
}

/// Monitor: the event log's decodes one line each, newest at the bottom, and a slot receiving now (TERM-5).
#[test]
fn monitor() {
    let mut r = Rig::main();
    r.live.slots[1].squelch_open = true;
    r.live.slots[1].freq = 44_347_500;
    r.live.slots[1].rssi = -80;
    r.send(b"4");
    assert_eq!(r.term.page(), Page::Monitor);
    assert!(r.vt.has("-- live --"));
    let newest = r.store.data().event(0).unwrap();
    let now_row = r.vt.row_of("B 443.475").expect("the slot receiving now");
    let ev_row = r.vt.row_of(core::str::from_utf8(newest.src.name().as_bytes()).unwrap()).unwrap();
    assert!(ev_row < now_row, "events above the live slots");
    assert!(r.vt.line(now_row).trim_start().starts_with("now"));
}

#[test]
fn frequencies() {
    assert_eq!(parse_mhz(b"146.52"), Some(14_652_000));
    assert_eq!(parse_mhz(b"446"), Some(44_600_000));
    assert_eq!(parse_mhz(b"146.520 00"), Some(14_652_000));
    assert_eq!(parse_mhz(b"146.5205"), Some(14_652_050));
    assert_eq!(parse_mhz(b"abc"), None);
    assert_eq!(parse_mhz(b""), None);
    assert_eq!(mhz5(14_652_000).as_bytes(), b"146.52000");
    assert_eq!(mhz5(44_347_500).as_bytes(), b"443.47500");
}

/// The state is small: no screen buffer (TERM-19); the command line's 255-byte buffer (TERM-12), a debug
/// command's one answer (64 bytes of memory at most, TERM-22: about 100 bytes) and the channel list's tools
/// (TERM-14: marks, a bit per channel, 125 bytes; a sorted page of up to 56 channel numbers, 112 bytes;
/// Oct 6, 2026) on top.
#[test]
fn small() {
    let n = core::mem::size_of::<Term>();
    std::eprintln!("Term is {n} bytes");
    assert!(n <= 512 + 256 + 112 + 304, "Term is {n} bytes");
}

// ---- The command line and dump/load (TERM-12, TERM-13) ----

use crate::chan::{ChanWrite, Name, FLAG_APRS, FLAG_RX_ONLY, FLAG_SKIP};
use crate::data::{Data, Op, RangePreset, ZoneCfg};
use crate::slot::{SlotCfg, CTCSS, DCS, DCS_FLAG, DCS_INV, DEFAULT_CFG};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

/// A blank radio's store: what a load fills in.
#[derive(Default)]
struct Blank {
    chans: RefCell<BTreeMap<u16, Channel>>,
    zones: RefCell<Vec<(Name, ZoneCfg)>>,
    presets: RefCell<Vec<RangePreset>>,
    busy: Cell<bool>,
}

impl Apply for Blank {
    fn apply(&self, w: ChanWrite) {
        let mut c = self.chans.borrow_mut();
        match w {
            ChanWrite::Put { ch, from } => {
                if let Some(f) = from {
                    c.remove(&f);
                }
                c.insert(ch.num, ch);
            }
            ChanWrite::Delete(n) => {
                c.remove(&n);
            }
            ChanWrite::DeleteZone(z) => c.values_mut().filter(|x| x.zone == z).for_each(|x| x.zone = NO_ZONE),
            ChanWrite::EraseAll => c.clear(),
        }
    }
}

impl ChannelStore for Blank {
    fn zones(&self) -> u8 {
        self.zones.borrow().len() as u8
    }
    fn zone_name(&self, zone: u8) -> Option<Name> {
        self.zones.borrow().get(zone as usize).map(|z| z.0)
    }
    fn zone_len(&self, zone: u8) -> u16 {
        self.chans.borrow().values().filter(|c| c.zone == zone).count() as u16
    }
    fn zone_at(&self, zone: u8, i: u16) -> Option<Channel> {
        self.chans.borrow().values().filter(|c| c.zone == zone).nth(i as usize).copied()
    }
    fn count(&self) -> u16 {
        self.chans.borrow().len() as u16
    }
    fn by_num(&self, num: u16) -> Option<(Channel, u16)> {
        let c = *self.chans.borrow().get(&num)?;
        let pos = self.chans.borrow().values().filter(|x| x.zone == c.zone && x.num < num).count() as u16;
        Some((c, pos))
    }
    fn by_number(&self, i: u16) -> Option<Channel> {
        self.chans.borrow().values().nth(i as usize).copied()
    }
    fn data(&self) -> &dyn Data {
        self
    }
}

impl Data for Blank {
    fn presets(&self) -> u8 {
        self.presets.borrow().len() as u8
    }
    fn preset(&self, i: u8) -> Option<RangePreset> {
        self.presets.borrow().get(i as usize).copied()
    }
    fn zone_cfg(&self, zone: u8) -> ZoneCfg {
        self.zones.borrow().get(zone as usize).map_or(ZoneCfg { shown: true, ..ZoneCfg::default() }, |z| z.1)
    }
    fn request(&self, op: Op) -> bool {
        match op {
            Op::PutZone(z, name, cfg) => {
                let mut zs = self.zones.borrow_mut();
                match (z as usize).cmp(&zs.len()) {
                    core::cmp::Ordering::Less => zs[z as usize] = (name, cfg),
                    core::cmp::Ordering::Equal if zs.len() < 254 => zs.push((name, cfg)),
                    _ => return false,
                }
                true
            }
            Op::PutPreset(i, p) => {
                let mut ps = self.presets.borrow_mut();
                let n = ps.len();
                if let Some(o) = ps.get_mut(i as usize) {
                    *o = RangePreset { skips: o.skips, ..p };
                } else if i as usize == n && n < 16 {
                    ps.push(RangePreset { skips: 0, ..p });
                } else {
                    return false;
                }
                true
            }
            Op::DeleteChannel(n) => self.chans.borrow_mut().remove(&n).is_some(),
            _ => false,
        }
    }
    fn busy(&self) -> bool {
        self.busy.get()
    }
}

/// TERM-12: `:` opens the command line on the bottom row; set and get use the menu's numbers and values.
#[test]
fn command_line_set_and_get() {
    let mut r = Rig::main();
    r.send(b":set 0 7");
    assert!(r.bottom().starts_with(" :set 0 7_"), "{}", r.bottom());
    r.send(b" 1 night red\r");
    r.poll();
    assert_eq!(r.app.set.theme, 2, "set by the value the radio shows, any case");
    assert!(r.bottom().contains("0 7 1 Theme: Night red"), "{}", r.bottom());
    r.cmd("get 0 7 1");
    assert!(r.bottom().contains("Theme: Night red (not the default)"), "{}", r.bottom());
    // Up brings the last command back; Esc closes the line without running it.
    r.send(b":\x1b[A");
    assert!(r.bottom().contains(":get 0 7 1_"), "{}", r.bottom());
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    assert!(!r.term.cmd);
    // Third-level numbers and values with spaces.
    r.cmd("set 0 3 1 2 200 kHz");
    assert_eq!(r.app.set.fm_step, 2);
    r.cmd("set 0 1 1 off");
    assert_eq!(r.app.watch, 0);
    // Typed values: the pager capcode (DEC-4), the custom APRS path (APRS-26), Popups (DEC-7).
    r.cmd("set 0 4 2 3 1234567");
    assert_eq!(r.app.set.pocsag_cap, 1_234_567);
    r.cmd("set 0 4 2 3 none");
    assert_eq!(r.app.set.pocsag_cap, 0);
    r.cmd("set 0 4 2 3 9999999");
    assert!(r.bottom().contains("a capcode up to 2097151"), "{}", r.bottom());
    r.cmd("set 0 5 6 2 wide2-2");
    assert_eq!(r.app.aprs_cfg().custom_text(), b"WIDE2-2");
    assert_eq!(r.app.aprs_cfg().path, crate::aprs::PATH_CUSTOM);
    r.cmd("set 0 5 6 2 WIDE1-1,WIDE2-1,WIDE3-1");
    assert!(r.bottom().contains("one or two entries"), "{}", r.bottom());
    r.cmd("set 0 5 6 2 None");
    assert!(r.app.aprs_cfg().custom_text().is_empty());
    r.cmd("set 0 4 8 off");
    assert!(!r.app.set.popups);
}

/// Every refusal says what was wrong and how to put it right; nothing changes.
#[test]
fn command_errors() {
    let mut r = Rig::main();
    let theme = r.app.set.theme;
    for (c, says) in [
        ("set 0 7 1 purple", "Theme: one of Dark, Daylight, Night red"),
        ("set 0 7 1", "set needs a value"),
        ("set 7 1 Dark", "a menu number from MENU"),
        ("set 0 1 2 1 Heard", "can't be set yet"),
        ("set 0 9 4 EU", "changes only on the radio"),
        ("set 0 9 3 1 Off", "changes only on the radio"),
        ("frob", "frob: no such command"),
        ("ch 999 power high", "ch 999: no such channel"),
        ("ch 12 power ultra", "power: low, mid, high"),
        ("ch 12 tone 300.0", "300.0 Hz is outside 60.0 to 260.0 Hz"),
        ("ch 12 tone D028N", "a DCS code (D023N, D023I)"),
        ("ch 12 rx 700", "rx: 0.15-64 MHz (HF), 64-630 or 756-1300 MHz"),
        ("ch 12 rx 30", "an HF channel is AM, USB, LSB or CW"),
        ("ch 12 mode usb", "USB, LSB and CW are for HF channels"),
        ("ch 12 zone 9", "zone 9 doesn't exist"),
        ("ch 12 \"Name", "no closing quote"),
        ("ch 1001", "ch: a number from 1 to 1000"),
        ("zone 9 \"Far\"", "the radio has 6, so the next is zone 7"),
        ("clear channels 3", "type: clear channels 21"),
    ] {
        r.cmd(c);
        assert!(r.bottom().contains(says), "{c}: {}", r.bottom());
        assert!(r.term.warn, "{c}: shown as a refusal");
    }
    assert_eq!(r.app.set.theme, theme);
    assert_eq!(r.store.count(), 21);
}

/// `ch`: one field at a time, a whole channel, show, delete; the 1.25 m cap holds (TERM-7).
#[test]
fn command_channels() {
    let mut r = Rig::main();
    r.cmd("ch 12 power high");
    assert_eq!(r.store.by_num(11).unwrap().0.cfg.power, 2);
    r.cmd("ch 12 tone=D023I");
    let c = r.store.by_num(11).unwrap().0;
    assert_eq!((c.cfg.tx_tone, c.cfg.rx_tone), (DCS_FLAG | DCS_INV | 0o023, DCS_FLAG | DCS_INV | 0o023));
    r.cmd("ch 12 name \"Club \\\"north\\\"\"");
    assert_eq!(r.store.by_num(11).unwrap().0.name(), b"Club \"north\"");
    r.cmd("ch 300 \"Test\" 146.6 offset=-0.6 tone=100.0 power=mid zone=1 skip squelch=6");
    let t = r.store.by_num(299).expect("made").0;
    assert_eq!((t.rx, t.cfg.offset, t.cfg.tx_tone, t.cfg.power, t.zone, t.flags & FLAG_SKIP, t.cfg.squelch), (14_660_000, -60_000, 1000, 1, 0, FLAG_SKIP, 6));
    r.cmd("ch 301 \"Two20\" 223.5 power=high");
    assert!(r.bottom().contains("fixed at 0.25 W on 1.25 m"), "{}", r.bottom());
    assert!(r.store.by_num(300).is_none(), "refused whole");
    r.cmd("ch 301 \"Two20\" 223.5");
    r.cmd("ch 301 power mid");
    assert!(r.bottom().contains("fixed at 0.25 W"), "{}", r.bottom());
    // Show: the console, then Enter goes back to the page.
    r.cmd("ch 300");
    assert!(r.raw().contains("ch 300 \"Test\" 146.60000 offset=-0.60000 mode=NFM tone=100.0 power=mid zone=1 squelch=6 skip"), "{}", r.raw());
    r.send(b"\r");
    assert_eq!(r.term.con, Con::Off);
    assert!(r.vt.has("Ferrite terminal"), "the page again");
    r.cmd("ch 300 delete");
    assert!(r.store.by_num(299).is_none());
    // Clearing every channel needs their count typed.
    let n = r.store.count();
    r.cmd(&std::format!("clear channels {n}"));
    assert_eq!(r.store.count(), 0);
}

/// Help and the changed settings on the console.
#[test]
fn command_lists() {
    let mut r = Rig::main();
    r.cmd("help");
    assert!(r.raw().contains("Commands: type : then one of these"));
    assert!(r.raw().contains("Enter or Esc returns to the menu."));
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    assert_eq!(r.term.con, Con::Off);
    r.cmd("set 0 7 1 Daylight");
    r.cmd("get");
    assert!(r.raw().contains("set 0 7 1 Daylight"), "{}", r.raw());
}

/// TERM-13: the dump starts with its version, has every section, and ends with `end`.
#[test]
fn dump_format() {
    let r = Rig::main();
    let d = dump(&r.app, &r.store, &r.dev);
    assert_eq!(d[0], "# Ferrite dump v1, firmware 0.1.0");
    for l in [
        "set 0 7 1 Dark                    # Theme",
        "set 0 6 6 24 h, GPS               # Clock",
        "zone 2 \"Repeaters\" scan=on shown=on distance=off resume=radio dwell=radio pri1=12",
        "preset 3 \"Airband\" 118.00000 137.00000 step=25kHz mode=AM resume=carrier",
        "ch 12 \"Club repeater\" 443.47500 offset=+5.00000 mode=NFM txtone=100.0 rxtone=off power=low zone=2",
        "ch 63 \"FRS 1\" 462.56250 offset=simplex mode=NFM tone=off power=low",
    ] {
        assert!(d.iter().any(|x| x == l), "missing: {l}");
    }
    assert_eq!(d.last().map(String::as_str), Some("end"));
    assert!(d.iter().all(|l| l.len() < 200));
    // The console streams it, the same lines.
    let mut r = Rig::main();
    r.cmd("dump");
    for _ in 0..4 {
        r.poll();
    }
    let raw = r.raw();
    for l in &d {
        assert!(raw.contains(&std::format!("{l}\r\n")), "streamed: {l}");
    }
}

/// TERM-13: dump, load on a blank radio, dump again: the same text.
#[test]
fn dump_load_round_trip() {
    let a = Rig::main();
    let text = dump(&a.app, &a.store, &a.dev).join("\n");
    let mut b = Rig::with(Blank::default());
    b.app.set.theme = 1; // a setting the dump puts back
    b.send(b"\r\r");
    b.load(&text);
    assert!(b.raw().contains(" 0 refused."), "{}", b.raw());
    assert_eq!(dump(&b.app, &b.store, &b.dev), dump(&a.app, &a.store, &a.dev));
}

/// A full radio: 1000 channels with every field in use, 30 zones, 16 presets; round trip through the terminal.
#[test]
fn dump_load_1000_channels() {
    let full = Blank::default();
    for z in 0..30u8 {
        let mut n = Name::new();
        let _ = uwrite!(n, "Zone \"{}\\", z);
        let cfg = ZoneCfg { scan_all: z % 2 == 0, shown: z % 3 != 0, by_distance: z % 5 == 0, resume: z % 13, dwell: if z % 4 == 0 { 255 } else { z }, pri: [Some(z as u16 * 3), if z % 2 == 1 { Some(999) } else { None }] };
        assert!(full.request(Op::PutZone(z, n, cfg)));
    }
    for i in 0..16u8 {
        let p = RangePreset { name: Buf::from(&b"Range 1234567"[..12 - (i as usize % 3)]), start: mhz(140) + i as u32 * 1_000_000, end: mhz(150) + i as u32 * 1_000_000, step: i % 14, mode: i % 3, narrow: i % 2 == 0, skips: 0, resume: i % 13 };
        assert!(full.request(Op::PutPreset(i, p)));
    }
    let bases = [mhz(144), mhz(430), mhz(156), mhz(118), mhz(222)];
    for n in 0..1000u16 {
        let k = n as u32;
        let rx = bases[n as usize % 5] + (k * 1250) % 300_000;
        let tone = |m: u32| match m % 4 {
            0 => 0,
            1 => CTCSS[(m % 50) as usize],
            2 => DCS_FLAG | DCS[(m % 104) as usize],
            _ => DCS_FLAG | DCS_INV | DCS[(m * 7 % 104) as usize],
        };
        let off = [0, 60_000, -60_000, 500_000, -500_000, 160_000][n as usize % 6];
        let cfg = SlotCfg {
            mode: (n % 3) as u8,
            narrow: n % 2 == 0,
            rx_tone: tone(k / 3),
            tx_tone: tone(k / 2),
            offset: off,
            power: if bases[n as usize % 5] == mhz(222) { 0 } else { (n % 3) as u8 },
            step: (n % 14) as u8,
            squelch: (n % 10) as u8,
            busy: (n % 3) as u8,
            reverse: n % 5 == 0,
            // every 7th a split (odd: RX on one band, TX on another)
            split: n % 7 == 3,
        };
        let cfg = if cfg.split { SlotCfg { offset: mhz(146) as i32 + (n as i32 % 40) * 1250, power: 0, ..cfg } } else { cfg };
        let mut c = Channel { num: n, zone: if n % 31 == 30 { NO_ZONE } else { (n % 31) as u8 }, rx, name: [0; 16], cfg, flags: (n % 8) as u8 & (FLAG_SKIP | FLAG_APRS | FLAG_RX_ONLY), tones: (n % 7) as u8 | ((n % 3) as u8) << 3 };
        let mut name = Buf::<16>::new();
        let _ = uwrite!(name, "{}{}", if n % 9 == 0 { "q\"\\ " } else { "Ch " }, n);
        c.set_name(name.as_bytes());
        full.apply(ChanWrite::Put { ch: c, from: None });
    }
    let a = Rig::with(full);
    a.app.set.theme = 2;
    a.app.set.home_ch = Some(500);
    let d = dump(&a.app, &a.store, &a.dev);
    assert_eq!(d.iter().filter(|l| l.starts_with("ch ")).count(), 1000);
    let mut b = Rig::with(Blank::default());
    b.send(b"\r\r");
    b.vt.room = usize::MAX / 2;
    b.load(&d.join("\n"));
    assert!(b.raw().contains(" 0 refused."), "{}", &b.raw()[b.raw().len().saturating_sub(400)..]);
    assert_eq!(dump(&b.app, &b.store, &b.dev), d);
    assert_eq!(DEFAULT_CFG.squelch, 4);
}

/// TERM-13: a bad line is named and skipped whole; the rest goes in; region and the 1.25 m cap hold (TERM-7).
#[test]
fn load_reports_bad_lines() {
    let mut r = Rig::main();
    let long = "a".repeat(300);
    let text = std::format!(
        "# a note\n\nset 0 7 1 Daylight\nset 0 9 4 EU\nch 5 \"X\" 146.5 tone=999\nch 6 \"Six\" 223.5 power=high\nch 7 \"Seven\" 146.55 bogus=1\nzone 9 \"Far\"\ndump\n{long}\nch 8 \"Eight\" 146.56 power=high\nend\nch 9 \"after the end\" 146.57"
    );
    r.load(&text);
    let raw = r.raw();
    for l in [
        "line 4: Region changes only on the radio",
        "line 5: tone: 999.0 Hz is outside 60.0 to 260.0 Hz",
        "line 6: power: fixed at 0.25 W on 1.25 m",
        "line 7: bogus: no such field",
        "line 8: zone 9: zones are made in order",
        "line 9: dump can't be in a load",
        "line 10: too long",
        "Done: 11 lines read, 2 applied, 7 refused.",
    ] {
        assert!(raw.contains(l), "{l}\n{raw}");
    }
    assert_eq!(r.app.set.theme, 1);
    assert!(r.store.by_num(4).is_none() && r.store.by_num(5).is_none() && r.store.by_num(6).is_none());
    assert_eq!(r.store.by_num(7).unwrap().0.cfg.power, 2);
    assert!(r.store.by_num(8).is_none(), "nothing after end");
    assert_eq!(r.app.set.region, crate::tests::set().region);
}

/// A load waits for the store before each line (TERM-13): the cable is held, nothing is lost.
#[test]
fn load_waits_for_the_store() {
    let mut r = Rig::with(Blank::default());
    r.send(b"\r\r");
    r.cmd("load");
    r.store.busy.set(true);
    r.send(b"ch 1 \"One\" 146.52\r");
    for _ in 0..5 {
        r.poll();
    }
    assert!(!r.term.ready(), "the line waits");
    assert_eq!(r.store.count(), 0);
    r.store.busy.set(false);
    r.poll();
    assert!(r.term.ready());
    assert_eq!(r.store.count(), 1);
    r.send(b"\x04");
    r.poll();
    assert_eq!(r.term.con, Con::Done);
    assert!(r.raw().contains("Done: 1 lines read, 1 applied, 0 refused."));
}


/// Split from the command line and the dump: offset=split with tx= (either order), the TX band's power cap,
/// squelch=global, and a ch line that loads back the same.
#[test]
fn command_split_and_global() {
    let mut r = Rig::main();
    r.cmd("ch 310 \"Cross\" 446.0 tx=146.52 offset=split squelch=global");
    let c = r.store.by_num(309).expect("made").0;
    assert!(c.cfg.split);
    assert_eq!((c.cfg.offset, c.cfg.squelch), (14_652_000, crate::slot::SQ_GLOBAL));
    r.cmd("ch 311 \"Cross2\" 446.0 offset=split tx=146.52");
    assert!(r.store.by_num(310).expect("either order").0.cfg.split);
    r.cmd("ch 312 \"NoTx\" 446.0 offset=split");
    assert!(r.bottom().contains("needs its TX frequency"), "{}", r.bottom());
    assert!(r.store.by_num(311).is_none());
    // TX on 1.25 m: power is capped by the TX band even with RX on 70 cm.
    r.cmd("ch 313 \"Odd\" 446.0 tx=223.5 power=high");
    assert!(r.bottom().contains("fixed at 0.25 W"), "{}", r.bottom());
    // The ch line, and back.
    r.cmd("ch 310");
    assert!(r.raw().contains("ch 310 \"Cross\" 446.00000 offset=split tx=146.52000 mode=NFM"), "{}", r.raw());
    assert!(r.raw().contains("squelch=global"), "{}", r.raw());
    r.send(b"\r");
    r.cmd("ch 310 offset=+5");
    let c = r.store.by_num(309).unwrap().0;
    assert!(!c.cfg.split && c.cfg.offset == 500_000, "a shift ends the split");
}

/// The channel editor: Offset stepped onto Split moves to the TX row to type the frequency.
#[test]
fn editor_split() {
    let mut r = Rig::main();
    let first = r.store.by_number(0).unwrap();
    r.send(b"1");
    r.send(b"\x1b[H");
    r.send(b"\r");
    let off = FIELDS.iter().position(|f| *f == F::It(Item::Offset)).unwrap();
    for _ in 0..off {
        r.send(b"\x1b[B");
    }
    // Simplex -> Split is one step left (the 4th choice wraps).
    r.send(b"\x1b[D");
    assert_eq!(FIELDS[r.term.field as usize], F::Tx, "typing the TX now");
    r.send(b"144.39\r");
    r.poll();
    let c = r.store.by_num(first.num).unwrap().0;
    assert!(c.cfg.split, "{:?}", c.cfg);
    assert_eq!(c.cfg.offset, 14_439_000);
    assert!(r.vt.has("Split 144.390") && r.vt.has("144.39000 MHz, split"), "{}", r.vt.text());
}

/// TX-37 from the cable: a channel line whose split TX needs the other relay state is refused, typed or loaded.
#[test]
fn split_across_the_relay_refused() {
    let mut r = Rig::main();
    r.cmd("ch 320 \"Cross\" 446.0 offset=split tx=146.52");
    assert!(r.store.by_num(319).expect("allowed: same relay state").0.cfg.split);
    r.cmd("ch 321 \"Six\" 146.52 offset=split tx=50.125");
    assert!(r.bottom().contains("Split TX needs the HF relay"), "{}", r.bottom());
    assert!(r.store.by_num(320).is_none());
    r.load("ch 322 \"Ok\" 146.55\nch 323 \"Bad\" 146.52 offset=split tx=50.125\nend");
    let raw = r.raw();
    assert!(raw.contains("line 2: Split TX needs the HF relay: not allowed with this RX frequency"), "{raw}");
    assert!(r.store.by_num(321).is_some() && r.store.by_num(322).is_none());
}

// ---- Debug commands (TERM-21 to TERM-24) ----

use ferrite_core::msg::{DebugAns, DebugReq, DEBUG_MEM};

impl Rig<Sample> {
    /// Polls, answering each debug request with `f`; every request seen, in order.
    fn serve(&mut self, polls: usize, mut f: impl FnMut(DebugReq) -> DebugAns) -> Vec<DebugReq> {
        let mut seen = Vec::new();
        for _ in 0..polls {
            if let Some(r) = self.term.debug_req() {
                seen.push(r);
                self.term.debug_answer(f(r));
            }
            self.poll();
        }
        seen
    }
}

/// A made-up register file: each register reads as its number twice (REG_30 = 0x3030).
fn regs(r: DebugReq) -> DebugAns {
    match r {
        DebugReq::BkRead { chip, reg } => DebugAns::Reg { chip, reg, val: reg as u16 * 0x101 },
        DebugReq::BkWrite { chip, reg, val } => DebugAns::Reg { chip, reg, val },
        DebugReq::Mem { addr, n, .. } => {
            let mut data = [0u8; DEBUG_MEM];
            for (i, b) in data.iter_mut().enumerate() {
                *b = (addr as usize + i) as u8;
            }
            DebugAns::Mem { addr, data, n }
        }
        DebugReq::Si { reply, .. } => {
            let mut data = [0u8; 16];
            data[0] = 0x80;
            DebugAns::Si { data, n: reply }
        }
        // A module that answers its version.
        DebugReq::Bt { .. } => {
            let mut data = [0u8; 48];
            data[..8].copy_from_slice(b"V1.0\r\nOK");
            DebugAns::Bt { data, n: 8 }
        }
    }
}

/// TERM-27: `bt` sends only queries; the answer prints as text (control bytes as dots); settings are refused
/// before anything is sent.
#[test]
fn debug_bt_queries_only() {
    let mut r = Rig::main();
    r.cmd("bt AT+VERS?");
    let mut text = [0u8; 16];
    text[..8].copy_from_slice(b"AT+VERS?");
    assert_eq!(r.serve(4, regs), vec![DebugReq::Bt { text, n: 8 }]);
    assert!(r.raw().contains("bt AT+VERS? -> V1.0..OK"), "{}", r.raw());
    r.send(b"\r");
    for bad in ["bt AT+BAUD=9600", "bt AT+RST", "bt AT+NAME=Ferrite"] {
        r.cmd(bad);
        assert!(r.bottom().contains("refused"), "{bad}: {}", r.bottom());
        assert_eq!(r.term.debug_req(), None, "{bad}: nothing sent");
    }
}

/// TERM-23: a register read and a write print their value and bits; the read-back is shown.
#[test]
fn debug_bk_read_and_write() {
    let mut r = Rig::main();
    r.cmd("bk 0 30");
    assert_eq!(r.serve(4, regs), vec![DebugReq::BkRead { chip: 0, reg: 0x30 }]);
    assert!(r.raw().contains("bk0 REG_30 = 0x3030   0011 0000 0011 0000"), "{}", r.raw());
    r.send(b"\r");
    r.cmd("bk 1 0x47 6142");
    assert_eq!(r.serve(4, regs), vec![DebugReq::BkWrite { chip: 1, reg: 0x47, val: 0x6142 }]);
    assert!(r.raw().contains("bk1 REG_47 <- 0x6142, reads back 0x6142"), "{}", r.raw());
}

/// TERM-23: transmit and PA bits, and the GPIO registers, are refused before anything is asked.
#[test]
fn debug_bk_refuses_tx_and_gpio() {
    let mut r = Rig::main();
    r.cmd("bk 0 30 BFF9");
    assert!(r.bottom().contains("refused: REG_30 Tx DSP and PA Gain"), "{}", r.bottom());
    assert_eq!(r.term.debug_req(), None);
    r.cmd("bk 1 33 0000");
    assert!(r.bottom().contains("REG_33 and REG_34"), "{}", r.bottom());
    r.cmd("bk 0 30 BFF1");
    assert_eq!(r.serve(2, regs), vec![DebugReq::BkWrite { chip: 0, reg: 0x30, val: 0xBFF1 }], "receive bits are fine");
    r.send(b"\r");
    r.cmd("bk 2 30");
    assert!(r.bottom().contains("bk CHIP REG"), "{}", r.bottom());
    r.cmd("bk 0 80");
    assert!(r.bottom().contains("bk CHIP REG"), "{}", r.bottom());
}

/// TERM-23: `bk N dump` reads all 128 registers, one request at a time, eight to a line.
#[test]
fn debug_bk_dump() {
    let mut r = Rig::main();
    r.cmd("bk 1 dump");
    let seen = r.serve(200, regs);
    assert_eq!(seen.len(), 128);
    assert_eq!(seen[127], DebugReq::BkRead { chip: 1, reg: 0x7F });
    let raw = r.raw();
    assert!(raw.contains("bk1 00: 0000 0101 0202 0303 0404 0505 0606 0707"), "{raw}");
    assert!(raw.contains("bk1 78: 7878 7979 7A7A 7B7B 7C7C 7D7D 7E7E 7F7F"), "{raw}");
    assert!(raw.contains("Enter or Esc returns"), "{raw}");
}

/// TERM-22: memory in 64-byte requests, 16 bytes a line with their characters; words for peripherals.
#[test]
fn debug_mem() {
    let mut r = Rig::main();
    r.cmd("mem 20000000 100");
    let seen = r.serve(20, regs);
    assert_eq!(seen, vec![DebugReq::Mem { addr: 0x2000_0000, n: 64, words: false }, DebugReq::Mem { addr: 0x2000_0040, n: 36, words: false }]);
    let raw = r.raw();
    assert!(raw.contains("20000000  00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F  |................|"), "{raw}");
    assert!(raw.contains("20000060  60 61 62 63"), "{raw}");
    assert!(!raw.contains("20000070"), "100 bytes end at 0x63: {raw}");
    r.send(b"\r");
    r.cmd("mem32 40010808 2");
    assert_eq!(r.serve(6, regs), vec![DebugReq::Mem { addr: 0x4001_0808, n: 8, words: true }]);
    assert!(r.raw().contains("40010808  0B0A0908 0F0E0D0C"), "{}", r.raw());
    r.send(b"\r");
    r.cmd("mem32 40010809");
    assert!(r.bottom().contains("word-aligned"), "{}", r.bottom());
}

/// TERM-22: `gpio` reads each port's input and output registers, A to E.
#[test]
fn debug_gpio() {
    let mut r = Rig::main();
    r.cmd("gpio");
    let seen = r.serve(20, regs);
    assert_eq!(seen.len(), 5);
    assert_eq!(seen[4], DebugReq::Mem { addr: 0x4001_1808, n: 8, words: true });
    assert!(r.raw().contains("PA  in 0000 1001 0000 1000   out 0000 1101 0000 1100"), "{}", r.raw());
    assert!(r.raw().contains("  PE  in "), "{}", r.raw());
}

/// TERM-24: SI4732 commands and properties go as typed; the reply starts with the status byte.
#[test]
fn debug_si() {
    let mut r = Rig::main();
    r.cmd("si prop 4001");
    let seen = r.serve(4, regs);
    assert!(matches!(seen[0], DebugReq::Si { cmd: [0x13, 0, 0x40, 0x01, ..], n: 4, reply: 4 }), "{seen:?}");
    assert!(r.raw().contains("si 13 00 40 01 -> 80 00 00 00"), "{}", r.raw());
    r.send(b"\r");
    r.cmd("si 10 00 reply 9");
    assert!(matches!(r.serve(4, regs)[0], DebugReq::Si { cmd: [0x10, 0, ..], n: 2, reply: 9 }));
}

/// TERM-21: no answer in 2 s ends the command; Esc stops one, and a late answer is dropped.
#[test]
fn debug_no_answer_and_stop() {
    let mut r = Rig::main();
    r.cmd("bk 0 30");
    assert!(r.term.debug_req().is_some());
    for _ in 0..25 {
        r.poll();
    }
    assert!(r.raw().contains("no answer"), "{}", r.raw());
    r.send(b"\r");
    r.cmd("bk 0 30");
    assert!(r.term.debug_req().is_some());
    r.send(b"\x1b");
    for _ in 0..3 {
        r.poll();
    }
    r.term.debug_answer(DebugAns::Reg { chip: 0, reg: 0x30, val: 1 });
    r.poll();
    assert!(!r.raw().contains("REG_30 ="), "stopped: {}", r.raw());
    assert!(matches!(r.term.con, Con::Done | Con::Off), "{:?}", r.term.con);
}

// ---- The channel list's tools (TERM-14), the radio's editor (TERM-17), colour (TERM-18) ----

impl Rig<Sample> {
    /// The channel list, every channel.
    fn list() -> Self {
        let mut r = Rig::main();
        r.send(b"1");
        assert_eq!(r.term.page(), Page::Channels);
        r
    }

    /// Polls until a change to several channels is done.
    fn settle(&mut self) {
        for _ in 0..200 {
            if self.term.bulk.is_none() {
                break;
            }
            self.poll();
        }
        assert!(self.term.bulk.is_none(), "the change finished");
        self.poll();
    }

    fn nums(&self) -> Vec<u16> {
        (0..crate::chan::MAX_CHANNELS).filter(|&n| self.store.get(n).is_some()).collect()
    }
}

/// TERM-14: S sorts by the next column, the header says which; the page follows the order.
#[test]
fn list_sorts_by_column() {
    let mut r = Rig::list();
    assert!(r.vt.has("sort: number"), "{}", r.vt.text());
    r.send(b"s");
    assert!(r.vt.has("sort: name"), "{}", r.vt.text());
    let mut names: Vec<Vec<u8>> = r.nums().iter().map(|&n| r.store.get(n).unwrap().name().to_ascii_lowercase()).collect();
    names.sort();
    let first = r.term.selected(&r.store).unwrap();
    assert_eq!(first.name().to_ascii_lowercase(), names[0], "the first row is the first name");
    // Down then End then Home move through the sorted list.
    r.send(b"\x1b[B");
    assert_eq!(r.term.selected(&r.store).unwrap().name().to_ascii_lowercase(), names[1]);
    r.send(b"\x1b[F");
    assert_eq!(r.term.selected(&r.store).unwrap().name().to_ascii_lowercase(), *names.last().unwrap());
    r.send(b"\x1b[H");
    assert_eq!(r.term.selected(&r.store).unwrap().name().to_ascii_lowercase(), names[0]);
    // RX: by frequency.
    r.send(b"s");
    assert!(r.vt.has("sort: RX"));
    let low = r.nums().iter().map(|&n| r.store.get(n).unwrap().rx).min().unwrap();
    assert_eq!(r.term.selected(&r.store).unwrap().rx, low);
}

/// TERM-14: / finds part of a name or a frequency, from the row after the selection; Enter alone finds the
/// same again; no match says so.
#[test]
fn list_finds() {
    let mut r = Rig::list();
    let target = r.store.get(*r.nums().last().unwrap()).unwrap();
    let part = std::str::from_utf8(&target.name()[..3]).unwrap().to_lowercase();
    r.send(b"/");
    assert!(r.vt.has("Find (name or MHz"), "{}", r.vt.text());
    r.send(part.as_bytes());
    r.send(b"\r");
    assert!(r.term.selected(&r.store).unwrap().name().to_ascii_lowercase().windows(3).any(|w| w == part.as_bytes()));
    r.send(b"/");
    r.send(b"zzqq\r");
    assert!(r.bottom().contains("No channel here matches \"zzqq\""), "{}", r.bottom());
    // A frequency's start.
    let f = mhz5(target.rx);
    r.send(b"/");
    r.send(&f.as_bytes()[..7]);
    r.send(b"\r");
    assert!(mhz5(r.term.selected(&r.store).unwrap().rx).as_bytes().starts_with(&f.as_bytes()[..7]));
}

/// TERM-14: Space marks (and moves down), the header counts them; D asks with the count, n keeps, y deletes
/// them one write at a time.
#[test]
fn list_marks_and_deletes() {
    let mut r = Rig::list();
    let before = r.nums();
    r.send(b"  ");
    assert!(r.vt.has("2 marked"), "{}", r.vt.text());
    assert!(r.term.marked(before[0]) && r.term.marked(before[1]));
    assert_eq!(r.term.selected(&r.store).unwrap().num, before[2], "Space moves down");
    r.send(b"d");
    assert!(r.vt.has("Delete 2 marked channels? y deletes"), "{}", r.vt.text());
    r.send(b"n");
    assert!(r.bottom().contains("Nothing deleted"));
    assert_eq!(r.nums(), before);
    r.send(b"d");
    r.send(b"y");
    r.settle();
    assert_eq!(r.nums(), before[2..].to_vec(), "the two marked ones are gone");
    assert!(r.bottom().contains("2 channels deleted"), "{}", r.bottom());
    assert_eq!(r.term.marks_n(), 0);
}

/// TERM-14: M moves the selected channel to a zone (by number or name, or none); C copies it to the first
/// free number.
#[test]
fn list_moves_and_copies() {
    let mut r = Rig::list();
    let c = r.term.selected(&r.store).unwrap();
    r.send(b"m");
    r.send(b"none\r");
    r.settle();
    assert_eq!(r.store.get(c.num).unwrap().zone, NO_ZONE);
    let z1 = r.store.zone_name(0).unwrap();
    r.send(b"m");
    r.send(z1.as_bytes());
    r.send(b"\r");
    r.settle();
    assert_eq!(r.store.get(c.num).unwrap().zone, 0, "by name");
    let free = r.store.free_num(0).unwrap();
    r.send(b"c\r");
    r.settle();
    let copy = r.store.get(free).expect("copied to the first free number");
    assert_eq!((copy.rx, copy.name()), (c.rx, c.name()));
    assert!(r.bottom().contains("1 channel copied"), "{}", r.bottom());
}

/// TERM-14: R renumbers the marked channels from a number, in their order, even over their own old numbers;
/// a number held by a channel that isn't moving is refused.
#[test]
fn list_renumbers() {
    let mut r = Rig::list();
    let before = r.nums();
    let (a, b) = (before[0], before[1]);
    let (ca, cb) = (r.store.get(a).unwrap(), r.store.get(b).unwrap());
    // Onto a channel that isn't moving: refused.
    r.send(b" ");
    r.send(b"r");
    r.send(std::format!("{}\r", before[2] + 1).as_bytes());
    assert!(r.bottom().contains("is taken by a channel that isn't moving"), "{}", r.bottom());
    // Both, to the first free numbers after the last channel.
    r.send(b" ");
    let to = before.last().unwrap() + 1;
    r.send(b"r");
    r.send(std::format!("{}\r", to + 1).as_bytes());
    r.settle();
    assert_eq!(r.store.get(to).unwrap().name(), ca.name());
    assert_eq!(r.store.get(to + 1).unwrap().name(), cb.name());
    assert!(r.store.get(a).is_none() && r.store.get(b).is_none());
    // Shift the two up by one: each lands where the other was (the order makes it safe).
    r.send(b"\x1b[F");
    r.send(b"\x1b[A ");
    r.send(b" ");
    assert_eq!(r.term.marks_n(), 2);
    r.send(b"r");
    r.send(std::format!("{}\r", to + 2).as_bytes());
    r.settle();
    assert_eq!(r.store.get(to + 1).unwrap().name(), ca.name());
    assert_eq!(r.store.get(to + 2).unwrap().name(), cb.name());
}

/// TERM-14: N makes a channel at the first free number and opens its editor.
#[test]
fn list_new_channel() {
    let mut r = Rig::list();
    let free = r.store.free_num(0).unwrap();
    let n = r.store.count();
    r.send(b"n");
    assert_eq!(r.term.page(), Page::Edit);
    assert_eq!(r.store.count(), n + 1);
    assert_eq!(r.term.edit.unwrap().num, free);
    assert!(r.bottom().contains("New channel") || r.vt.has("New channel"), "{}", r.vt.text());
}

/// TERM-17: a channel open in the radio's editor isn't changed from the terminal (the editor, `ch`, a bulk
/// change); a setting highlighted in the radio's menu isn't either.
#[test]
fn radio_editor_wins() {
    let mut r = Rig::list();
    let c = r.term.selected(&r.store).unwrap();
    let (z, pos) = (0..r.store.zones()).chain([NO_ZONE]).find_map(|z| (0..r.store.zone_len(z)).find(|&i| r.store.zone_at(z, i).unwrap().num == c.num).map(|i| (z, i))).unwrap();
    r.app.set_channel(r.app.active, z, pos, &r.store);
    r.app.screen = crate::Screen::ChEdit(crate::screens::chedit::ChEdit { cell: 0 });
    r.send(b"\r");
    for _ in 0..8 {
        r.send(b"\x1b[B");
    }
    r.send(b"\x1b[C");
    assert_eq!(r.store.get(c.num).unwrap(), c, "not written");
    assert!(r.vt.has("Open in the radio's editor"), "{}", r.vt.text());
    r.send(b"\x1b");
    r.cmd(&std::format!("ch {} power high", c.num + 1));
    assert!(r.bottom().contains("open in the radio's editor"), "{}", r.bottom());
    r.send(b"d");
    r.send(b"y");
    assert!(r.bottom().contains("open in the radio's editor"), "{}", r.bottom());
    assert!(r.store.get(c.num).is_some());
    // A setting: 0 7 1 highlighted in the radio's menu.
    r.app.screen = crate::Screen::Group(crate::screens::groups::Group { g: 7, sub: 0, row: 0 });
    r.cmd("set 0 7 1 Daylight");
    assert!(r.bottom().contains("open on the radio's menu"), "{}", r.bottom());
}

/// TERM-18: colour is off by default (reverse video and bold only); 0 9 2 6 turns it on.
#[test]
fn colour_is_optional() {
    let mut r = Rig::main();
    assert!(!r.raw().contains("\x1b[0;97;44m"));
    r.cmd("set 0 9 2 6 On");
    assert!(r.app.set.term_colour);
    r.send(b"\x0c");
    r.poll();
    assert!(r.raw().contains("\x1b[0;97;44m"), "the top line in white on blue");
    assert!(r.raw().contains("\x1b[1;36m"), "headings in cyan");
}
