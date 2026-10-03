//! Zones (MENU 0 2 3; manual chapters 10 and 14, CHAN-6, CHAN-11, CHAN-12; C11) and one zone's settings
//! (A12): name, in "scan all", shown in ◀▶ on home, sort by distance, resume, dwell, smart scan, priority 1
//! and 2. The knob or ◀▶ change a setting at once (MENU-16, MENU-19); # resets it (MENU-18). # on the list
//! makes a new zone, * deletes one after asking (CHAN-12: its channels go to no zone and stay in All).

use super::lists::*;
use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::{Op, ZoneCfg};
use crate::keys::{KeyEvent, Press};
use crate::slot::chan_label;
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Zones {
    pub cur: Cursor,
    /// 0 the list, 1 the list asking before a delete, 2 + row: the zone editor (A12). One byte keeps the
    /// state six bytes.
    pub mode: u8,
}

impl Zones {
    pub fn new() -> Self {
        Zones { cur: Cursor::default(), mode: 0 }
    }

    /// The editor's row, when the editor is up.
    pub fn edit(self) -> Option<u8> {
        self.mode.checked_sub(2)
    }

    pub fn confirm(self) -> bool {
        self.mode == 1
    }
}

/// The editor's rows: label and help (layout as data; values from [`value`]).
const ROWS: [(&str, &str); 9] = [
    ("Name", "Up to 16 characters"),
    ("Scan all", "In the set \"zones in scan all\" scans"),
    ("Shown on home", "Home's left and right stop on it (CHAN-11)"),
    ("Sort by distance", "Nearest channel first, with a GPS fix"),
    ("Resume", "After a hit; the radio's rule unless set"),
    ("Dwell", "Wait before resuming"),
    ("Smart scan", "Checks busy channels more often"),
    ("Priority 1", "Checked during scans (SCAN-12)"),
    ("Priority 2", "Checked during scans (SCAN-12)"),
];

/// The defaults, for the dot (MENU-12) and # (MENU-18).
const DEFAULT: ZoneCfg = ZoneCfg { scan_all: false, shown: true, by_distance: false, resume: 0, dwell: 255, smart: false, pri: [None, None] };

fn keep(store: &impl ChannelStore) -> impl Fn(u16) -> bool + '_ {
    |i| store.zone_name(i as u8).is_some()
}

fn value(store: &impl ChannelStore, z: u8, c: &ZoneCfg, row: usize, b: &mut Buf<24>) {
    let on = |v: bool| if v { "On" } else { "Off" };
    b.clear();
    let _ = match row {
        0 => {
            b.set(store.zone_name(z).unwrap_or(b""));
            Ok(())
        }
        1 => uwrite!(b, "{}", on(c.scan_all)),
        2 => uwrite!(b, "{}", on(c.shown)),
        3 => uwrite!(b, "{}", on(c.by_distance)),
        4 => match c.resume {
            0 => uwrite!(b, "Radio's"),
            11 => uwrite!(b, "Carrier drop"),
            12 => uwrite!(b, "Stop on hit"),
            s => uwrite!(b, "After {} s", s),
        },
        5 => match c.dwell {
            255 => uwrite!(b, "Radio's"),
            s => uwrite!(b, "{} s", s),
        },
        6 => uwrite!(b, "{}", on(c.smart)),
        _ => match c.pri[row - 7].and_then(|n| (0..store.zone_len(z)).filter_map(|i| store.zone_at(z, i)).find(|ch| ch.num == n)) {
            Some(ch) => {
                let mut l = Buf::<32>::new();
                chan_label(ch.num, &mut l);
                let _ = uwrite!(b, "{} ", Txt(l.as_bytes()));
                b.push(ch.name());
                Ok(())
            }
            None => uwrite!(b, "None"),
        },
    };
}

/// A setting stepped by d.
fn change(store: &impl ChannelStore, z: u8, c: &mut ZoneCfg, row: usize, d: i32) {
    match row {
        1 => c.scan_all = !c.scan_all,
        2 => c.shown = !c.shown,
        3 => c.by_distance = !c.by_distance,
        4 => c.resume = (c.resume as i32 + d).rem_euclid(13) as u8,
        5 => {
            // the radio's, then 0-5 s
            let i = (if c.dwell == 255 { 0 } else { c.dwell as i32 + 1 } + d).rem_euclid(7);
            c.dwell = if i == 0 { 255 } else { i as u8 - 1 };
        }
        6 => c.smart = !c.smart,
        7 | 8 => {
            // None, then the zone's channels in order
            let n = store.zone_len(z) as i32;
            let at = c.pri[row - 7].and_then(|num| (0..store.zone_len(z)).position(|i| store.zone_at(z, i).is_some_and(|ch| ch.num == num)));
            let i = (at.map_or(-1, |p| p as i32) + d + 1).rem_euclid(n + 1) - 1;
            c.pri[row - 7] = if i < 0 { None } else { store.zone_at(z, i as u16).map(|ch| ch.num) };
        }
        _ => {}
    }
}

impl View for Zones {
    fn name(self) -> &'static str {
        if self.edit().is_some() { "Zone editor" } else { "Zones" }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(if self.edit().is_some() { b"Zone" } else { b"Zones" });
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let mut s = self;
        let z = self.cur.sel as u8;
        let d = store.data();
        match (e.key, e.press, self.edit()) {
            (Key::Up | Key::Down, Short | Repeat, _) if !self.confirm() => {
                let dir = if e.key == Key::Up { -1 } else { 1 };
                match self.edit() {
                    Some(r) => s.mode = 2 + (r as i32 + dir).rem_euclid(ROWS.len() as i32) as u8,
                    None => s.cur = self.cur.step(dir, store.zones() as u16, ROWS1, &keep(store)),
                }
            }
            (Key::Left | Key::Right, Short | Repeat, Some(_)) => return self.knob(app, if e.key == Key::Right { 1 } else { -1 }, store),
            (Key::Menu, Short, None) if self.confirm() => {
                let name = Buf::<16>::from(store.zone_name(z).unwrap_or(b""));
                if d.request(Op::DeleteZone(z)) {
                    app.show_toast(b"Zone deleted", name.as_bytes(), false);
                }
                s.mode = 0;
                s.cur = Cursor::first(store.zones() as u16, &keep(store));
            }
            (Key::Menu, Short, None) if store.zone_name(z).is_some() => s.mode = 2,
            (Key::Menu, Short, Some(0)) => return app.open(Dest::Text), // rename: text entry (A7)
            (Key::Hash, Short, None) => {
                if d.request(Op::NewZone) {
                    let n = store.zones() as u16;
                    s.cur = Cursor { sel: n - 1, top: self.cur.top }.shown(n, ROWS1, &keep(store));
                    app.show_toast(b"New zone", store.zone_name((n - 1) as u8).unwrap_or(b""), false);
                } else {
                    app.show_toast(b"No room", b"255 zones at most", false);
                }
            }
            (Key::Star, Short, None) if store.zone_name(z).is_some() => s.mode = 1,
            (Key::Hash, Short, Some(r)) => {
                // MENU-18: back to the default
                let mut c = d.zone_cfg(z);
                match r {
                    1 => c.scan_all = DEFAULT.scan_all,
                    2 => c.shown = DEFAULT.shown,
                    3 => c.by_distance = DEFAULT.by_distance,
                    4 => c.resume = DEFAULT.resume,
                    5 => c.dwell = DEFAULT.dwell,
                    6 => c.smart = DEFAULT.smart,
                    7 | 8 => c.pri[r as usize - 7] = None,
                    _ => {}
                }
                d.request(Op::Zone(z, c));
            }
            (Key::Exit, Short, _) if self.confirm() => s.mode = 0,
            (Key::Exit, Short, Some(_)) => s.mode = 0,
            (Key::Exit, Short, None) => return app.back(),
            _ => return,
        }
        app.screen = Screen::Zones(s);
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        match self.edit() {
            Some(r) if r > 0 => {
                let z = self.cur.sel as u8;
                let mut c = store.data().zone_cfg(z);
                change(store, z, &mut c, r as usize, d);
                store.data().request(Op::Zone(z, c));
            }
            Some(_) => {}
            None if !self.confirm() => app.screen = Screen::Zones(Zones { cur: self.cur.step(d, store.zones() as u16, ROWS1, &keep(store)), ..self }),
            None => {}
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let store = cx.store;
        let data = store.data();
        app.page(lcd, (self.edit().is_some(), self.confirm()));
        if !app.gate((self, data.version())) {
            return;
        }
        let z = self.cur.sel as u8;
        if let Some(row) = self.edit() {
            let c = data.zone_cfg(z);
            let mut r = Buf::<16>::new();
            let _ = uwrite!(r, "{} channels", store.zone_len(z));
            app.list_head(lcd, store.zone_name(z).unwrap_or(b""), r.as_bytes());
            for (i, (label, _)) in ROWS.iter().enumerate() {
                let mut v = Buf::<24>::new();
                value(store, z, &c, i, &mut v);
                let mut dv = Buf::<24>::new();
                value(store, z, &DEFAULT, i, &mut dv);
                app.row1(lcd, i, label.as_bytes(), v.as_bytes(), i > 0 && v != dv, i as u8 == row, false);
            }
            let mut p = Buf::<16>::new();
            let _ = uwrite!(p, "MENU 0 2 3");
            app.help_box(lcd, 300 - 38, 34, ROWS[row as usize].1.as_bytes(), p.as_bytes());
            return;
        }
        let n = store.zones() as u16;
        let shown = (0..n).filter(|&i| keep(store)(i)).count();
        let mut l = Buf::<16>::new();
        let _ = uwrite!(l, "{} zones", shown);
        app.list_head(lcd, l.as_bytes(), b"");
        let rows = self.cur.rows(n, ROWS1, &keep(store), |row, i| {
            let c = data.zone_cfg(i as u8);
            let mut v = Buf::<16>::new();
            let _ = uwrite!(v, "{}{}{} ch", if c.scan_all { "scan \u{b7} " } else { "" }, if c.shown { "" } else { "hidden \u{b7} " }, store.zone_len(i as u8));
            app.row1(lcd, row, store.zone_name(i as u8).unwrap_or(b""), v.as_bytes(), false, i == self.cur.sel, store.zone_len(i as u8) == 0);
        });
        app.rows_end(lcd, rows, ROWS1, ROW1_H);
        if self.confirm() {
            let mut q = Buf::<32>::new();
            q.push(b"Delete zone ");
            q.push(store.zone_name(z).unwrap_or(b""));
            q.push(b"?");
            let mut m = Buf::<40>::new();
            let _ = uwrite!(m, "{} channels go to no zone", store.zone_len(z));
            app.confirm_box(lcd, q.as_bytes(), m.as_bytes());
        } else {
            app.help_box(lcd, 300 - 38, 34, b"OK edits its scan options and priorities", b"MENU 0 2 3");
        }
    }

    fn hints(self, _: &App) -> Hints {
        match (self.edit(), self.confirm()) {
            (_, true) => Hints::new(&[("OK", "Delete"), ("BACK", "Keep")]),
            (Some(_), _) => Hints::new(&[(ARROWS_UD, "Move"), ("Knob", "Change"), ("#", "Reset")]),
            (None, _) => Hints::new(&[("OK", "Edit"), ("#", "New"), ("*", "Delete"), ("BACK", "Back")]),
        }
    }
}
