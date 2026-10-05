//! The terminal against a small VT100 model: what a terminal would show after the bytes it was sent.

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

struct Rig {
    app: App,
    store: Sample,
    term: Term,
    vt: Vt,
    dev: Device,
    live: LiveAll,
    now: u32,
}

impl Rig {
    fn new() -> Self {
        let mut app = App::new(crate::tests::set(), crate::tests::alert());
        while app.pop_cmd().is_some() {}
        let dev = Device { battery_mv: 7900, clock_min: Some(14 * 60 + 32), ..Default::default() };
        Rig { app, store: Sample::new(), term: Term::new(), vt: Vt::new(80, 24), dev, live: LiveAll::default(), now: 1000 }
    }

    fn send(&mut self, b: &[u8]) {
        for &c in b {
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
    }

    /// Started and on the main menu.
    fn main() -> Self {
        let mut r = Rig::new();
        r.send(b"\r\r");
        assert_eq!(r.term.page(), Page::Main);
        r
    }
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
    for _ in 0..7 {
        r.send(b"\x1b[B"); // down to Power
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
    r.send(b"\r30\r");
    assert!(r.vt.has("64 to 1000 MHz"), "refused");
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
    for _ in 0..8 {
        r.send(b"\x1b[B"); // Squelch
    }
    assert_eq!(FIELDS[r.term.field as usize], F::It(Item::Squelch));
    r.send(b"\x1b[C");
    assert_eq!(r.app.slot(0).cfg().squelch, (first.cfg.squelch + 1) % 10, "the slot's copy follows");
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

/// Monitor: the event log, newest at the bottom, and a slot receiving now.
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

/// The state is small: no screen buffer (TERM-19).
#[test]
fn small() {
    let n = core::mem::size_of::<Term>();
    std::eprintln!("Term is {n} bytes");
    assert!(n <= 512, "Term is {n} bytes");
}
