//! The channel browser (MENU 0 2 1; manual chapter 10, CHAN-10 to CHAN-15): pick a zone or a view (C8),
//! its channels (C9), and a channel's actions (C10: edit, rename, move, renumber, copy to slot, skip, delete).
//! Views are computed, never stored (CHAN-14): All by number, per band (Broadcast is the saved FM stations the
//! FM screen's OK opens, BCR-31), Recent from the event log, Nearby by distance from the GPS fix (CHAN-15: empty
//! without one). Channels are read from the store as rows are
//! drawn; the browser keeps only its position.
//!
//! Deleting asks first and names the channel (CHAN-13, MENU-23).

use super::lists::*;
use super::nav::Dest;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::{Channel, ChannelStore, FLAG_HIDDEN, NO_ZONE};
use crate::data::Op;
use crate::keys::{KeyEvent, Press};
use crate::slot::{chan_label, shift_text, tone_text, MODES};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// The views after the zones: All, five bands, Recent, Nearby.
const VIEWS: [&str; 8] = ["All", "2 m", "70 cm", "Air", "Marine", "Broadcast", "Recent", "Nearby"];
const V_ALL: u8 = 1;
/// The browser's `view` while picking one.
pub const PICKER: u8 = 255;
/// The band views: 2 to this.
const V_BANDS: u8 = 6;
pub(crate) const V_BROADCAST: u8 = 6;
const V_RECENT: u8 = 7;
const V_NEARBY: u8 = 8;
/// The band views' edges (CHAN-14: "2 m, 70 cm, air, marine"; BCR-31: broadcast FM, 76-108 MHz, the bands of every
/// region, BCR-21).
const BAND_VIEWS: [(Freq10, Freq10); 5] =
    [(mhz(144), mhz(148)), (mhz(420), mhz(450)), (mhz(108), mhz(137)), (mhz(156), mhz(163)), (mhz(76), mhz(108) + 1)];
/// Recent: up to ten channels (CHAN-25).
const RECENT_MAX: u16 = crate::chan::RECENT_MAX as u16;

/// The row actions (C10), in order.
const ACTIONS: [(&str, &str); 8] = [
    ("Edit", "Its settings, in place on the active slot"),
    ("Rename", "Up to 16 characters"),
    ("Move", "To another zone, or none"),
    ("Renumber", "Changes its place in its zone"),
    ("Copy to slot", "The active slot goes to this channel"),
    ("Skip", "Left out of every scan"),
    ("Hide", "Out of the lists; still there by number"),
    ("Delete", "Asks first"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    List,
    /// The actions menu, its row.
    Actions(u8),
    /// Picking a zone to move to (NO_ZONE: none).
    Move(u8),
    /// Picking a new number: it is kept in `cur.top` meanwhile (the list isn't shown), so the state stays
    /// eight bytes.
    Renumber,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Browse {
    /// [`PICKER`]: picking a view (C8); 0: a zone's channels; else 1 + an index into VIEWS.
    pub view: u8,
    pub zone: u8,
    pub cur: Cursor,
    pub mode: Mode,
}

impl Browse {
    pub fn new() -> Self {
        Browse { view: PICKER, zone: 0, cur: Cursor::default(), mode: Mode::List }
    }
}

/// A view: how many items, which show, and the channel at an index.
struct ViewOf<'a, S: ChannelStore> {
    store: &'a S,
    view: u8,
    zone: u8,
    /// Hidden channels are listed too (the list's options; decision 63).
    hidden: bool,
}

impl<S: ChannelStore> ViewOf<'_, S> {
    fn n(&self) -> u16 {
        match self.view {
            0 => self.store.zone_len(self.zone),
            V_RECENT => (0..RECENT_MAX).take_while(|&r| self.recent(r).is_some()).count() as u16,
            V_NEARBY if self.store.data().fix().is_none() => 0,
            V_NEARBY => (0..self.store.count()).filter(|&i| self.store.by_number(i).is_some_and(|c| self.store.position(c.num).is_some())).count() as u16,
            _ => self.store.count(),
        }
    }

    fn keep(&self, i: u16) -> bool {
        let band = match self.view {
            2..=V_BANDS => {
                let (lo, hi) = BAND_VIEWS[self.view as usize - 2];
                self.store.by_number(i).is_some_and(|c| c.rx >= lo && c.rx < hi)
            }
            _ => true,
        };
        // A hidden channel is left out of the lists unless they show hidden ones; Recent lists what was used.
        band && (self.hidden || self.view == V_RECENT || !self.at(i).is_some_and(|c| c.flags & FLAG_HIDDEN != 0))
    }

    fn at(&self, i: u16) -> Option<Channel> {
        match self.view {
            0 => self.store.zone_at(self.zone, i),
            V_RECENT => self.recent(i),
            V_NEARBY => self.nearest(i).map(|(c, _)| c),
            _ => self.store.by_number(i),
        }
    }


    /// Recent (CHAN-25): the r-th channel used, newest first, from the store's list; a deleted one is skipped.
    fn recent(&self, r: u16) -> Option<Channel> {
        (0..RECENT_MAX).filter_map(|k| self.store.recent(k)).filter_map(|n| self.store.get(n)).nth(r as usize)
    }


    /// Nearby: the r-th nearest channel with a position, and its distance and bearing (CHAN-15). Each step
    /// finds the next by (distance, number), so nothing is sorted in RAM.
    fn nearest(&self, r: u16) -> Option<(Channel, (u32, u16))> {
        let here = self.store.data().fix()?.pos;
        let mut prev: Option<(u32, u16)> = None;
        let mut found = None;
        for _ in 0..=r {
            found = None;
            for c in (0..self.store.count()).filter_map(|i| self.store.by_number(i)) {
                let Some(p) = self.store.position(c.num) else { continue };
                let db = dist_bearing(here, p);
                let key = (db.0, c.num);
                if prev.is_none_or(|p| key > p) && found.is_none_or(|(f, (fd, _)): (Channel, (u32, u16))| key < (fd, f.num)) {
                    found = Some((c, db));
                }
            }
            let (c, (d, _)) = found?;
            prev = Some((d, c.num));
        }
        found
    }
}

/// The Nearby view (CHAN-15), opened from the zones page's Nearby row (CHAN-29).
pub(crate) fn nearby_view(store: &impl ChannelStore, hidden: bool) -> Browse {
    opened(store, V_NEARBY, hidden)
}

/// BCR-31: the Broadcast view, the saved FM stations, opened by OK on the FM screen.
pub(crate) fn broadcast_view(store: &impl ChannelStore, hidden: bool) -> Browse {
    opened(store, V_BROADCAST, hidden)
}

fn opened(store: &impl ChannelStore, view: u8, hidden: bool) -> Browse {
    let mut b = Browse { view, ..Browse::new() };
    let v = view_of(store, &b, hidden);
    b.cur = Cursor::first(v.n(), &|i| v.keep(i));
    b
}

fn view_of<'a, S: ChannelStore>(store: &'a S, b: &Browse, hidden: bool) -> ViewOf<'a, S> {
    ViewOf { store, view: b.view, zone: b.zone, hidden }
}

/// The picker's rows: the zones that exist, then the views.
fn picker_n(store: &impl ChannelStore) -> u16 {
    store.zones() as u16 + VIEWS.len() as u16
}

fn picker_keep(store: &impl ChannelStore, i: u16) -> bool {
    i >= store.zones() as u16 || store.zone_name(i as u8).is_some()
}

impl View for Browse {
    /// KEY-19: renumbering steps 10 with ◀▶; otherwise they are the knob.
    fn own_arrows(self) -> bool {
        true
    }

    /// Hold MENU: the list's options (show hidden channels).
    fn options(self, app: &mut App) -> bool {
        app.open_screen(super::opts::Of::Channels.screen());
        true
    }

    fn name(self) -> &'static str {
        "Channel browser"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Channels");
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let mut s = self;
        let v = view_of(store, &self, app.set.show_hidden);
        let ch = if self.view == PICKER { None } else { v.at(self.cur.sel) };
        match (e.key, e.press, self.mode) {
            (Key::Up | Key::Down, Short | Repeat, _) => return self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Left | Key::Right, Short | Repeat, _) => {
                let d = if e.key == Key::Right { 1 } else { -1 };
                return self.knob(app, if self.mode == Mode::Renumber { d * 10 } else { d }, store);
            }
            // C8: open a zone or a view
            (Key::Menu, Short, Mode::List) if self.view == PICKER => {
                let z = store.zones() as u16;
                let i = self.cur.sel;
                (s.view, s.zone) = if i < z { (0, i as u8) } else { (1 + (i - z) as u8, 0) };
                let v = view_of(store, &s, app.set.show_hidden);
                s.cur = Cursor::first(v.n(), &|i| v.keep(i));
            }
            // BCR-31: a saved station plays on the FM screen, as a preset did; its actions are in its zone and All
            (Key::Menu, Short, Mode::List) if self.view == V_BROADCAST => {
                let Some(c) = ch else { return };
                return play(app, c);
            }
            (Key::Menu, Short, Mode::List) if ch.is_some() => s.mode = Mode::Actions(0),
            (Key::Menu, Short, Mode::Actions(a)) => {
                let Some(c) = ch else { return };
                match a {
                    0 => {
                        copy_to_slot(app, store, &c);
                        return app.open(Dest::ChannelEdit);
                    }
                    1 => return super::text::open(app, super::text::TextFor::Channel(c.num), store),
                    2 => s.mode = Mode::Move(c.zone),
                    3 => (s.mode, s.cur.top) = (Mode::Renumber, c.num),
                    4 => {
                        copy_to_slot(app, store, &c);
                        let mut h = Buf::<20>::new();
                        let _ = uwrite!(h, "Slot {}", w::LETTERS[app.active as usize] as char);
                        app.go_home();
                        return app.show_toast(h.as_bytes(), c.name(), false);
                    }
                    5 => {
                        store.data().request(Op::Skip(c.num, !c.skip()));
                        app.show_toast(c.name(), if c.skip() { b"back in scans" } else { b"skipped in scans" }, false);
                        s.mode = Mode::List;
                    }
                    6 => {
                        // Decision 63: hidden from the lists, unless they show hidden ones.
                        let hide = c.flags & FLAG_HIDDEN == 0;
                        store.data().request(Op::Hide(c.num, hide));
                        app.show_toast(c.name(), if hide { b"hidden from the lists" } else { b"shown in the lists" }, false);
                        s.mode = Mode::List;
                    }
                    _ => {
                        // CHAN-13, MENU-23: the shared confirm asks; BACK there returns to Delete.
                        return super::confirm::ask(app, super::confirm::Ask::DeleteChannel(c.num));
                    }
                }
            }
            (Key::Menu, Short, Mode::Move(z)) => {
                let Some(c) = ch else { return };
                if z != c.zone && store.data().request(Op::Move(c.num, z)) {
                    app.show_toast(c.name(), zone_name(store, z).as_bytes(), false);
                }
                s.mode = Mode::List;
            }
            (Key::Menu, Short, Mode::Renumber) => {
                let n = self.cur.top;
                let Some(c) = ch else { return };
                let mut l = Buf::<32>::new();
                chan_label(n, &mut l);
                if n != c.num && !store.data().request(Op::Renumber(c.num, n)) {
                    return app.show_toast(l.as_bytes(), b"is taken", false);
                }
                app.show_toast(c.name(), l.as_bytes(), false);
                s.mode = Mode::List;
                s.cur.top = s.cur.sel;
            }
            (Key::Exit, Short, Mode::List) if self.view != PICKER => {
                // back to the picker, on this view's row
                let row = if self.view == 0 { self.zone as u16 } else { store.zones() as u16 + self.view as u16 - 1 };
                s = Browse { view: PICKER, cur: Cursor { sel: row, top: 0 }.shown(picker_n(store), ROWS1, &|i| picker_keep(store, i)), ..Browse::new() };
            }
            (Key::Exit, Short, Mode::List) => return app.back(),
            (Key::Exit, Short, Mode::Actions(_)) => s.mode = Mode::List,
            // back to the actions, on the one it came from
            (Key::Exit, Short, m) => {
                s.mode = Mode::Actions(match m {
                    Mode::Move(_) => 2,
                    Mode::Renumber => 3,
                    _ => 6,
                });
                s.cur.top = s.cur.sel;
            }
            _ => return,
        }
        app.screen = Screen::Browse(s);
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let mut s = self;
        match self.mode {
            Mode::List if self.view == PICKER => s.cur = self.cur.step(d, picker_n(store), ROWS1, &|i| picker_keep(store, i)),
            Mode::List => {
                let v = view_of(store, &self, app.set.show_hidden);
                s.cur = self.cur.step(d, v.n(), ROWS2, &|i| v.keep(i));
            }
            Mode::Actions(a) => s.mode = Mode::Actions((a as i32 + d).rem_euclid(ACTIONS.len() as i32) as u8),
            Mode::Move(z) => {
                // the zones that exist, then "none"
                let n = store.zones() as i32 + 1;
                let mut i = if z == NO_ZONE { n - 1 } else { z as i32 };
                for _ in 0..n {
                    i = (i + d.signum()).rem_euclid(n);
                    if i == n - 1 || store.zone_name(i as u8).is_some() {
                        break;
                    }
                }
                s.mode = Mode::Move(if i == n - 1 { NO_ZONE } else { i as u8 });
            }
            Mode::Renumber => s.cur.top = (self.cur.top as i32 + d).clamp(0, 999) as u16,
        }
        app.screen = Screen::Browse(s);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let store = cx.store;
        let data = store.data();
        let list = self.mode == Mode::List;
        app.page(lcd, (self.view != PICKER, list));
        if !app.gate((self, data.version(), data.fix().map(|f| f.pos))) {
            return;
        }
        if self.view == PICKER {
            return picker(app, lcd, store, self.cur);
        }
        let view = self.view;
        let v = view_of(store, &self, app.set.show_hidden);
        let n = v.n();
        let ch = v.at(self.cur.sel);
        if !list {
            return actions(app, lcd, store, self, ch);
        }
        let mut name = Buf::<20>::new();
        name.set(if view == 0 { zone_name(store, self.zone) } else { Buf::from(VIEWS[view as usize - 1].as_bytes()) }.as_bytes());
        let shown = if (2..=V_BANDS).contains(&view) || !v.hidden { (0..n).filter(|&i| v.keep(i)).count() } else { n as usize };
        let mut r = Buf::<24>::new();
        match view {
            V_NEARBY => r.set(b"nearest first"),
            V_RECENT => r.set(b"newest first"),
            _ => {
                let _ = uwrite!(r, "{} {}", shown, if shown == 1 { "channel" } else { "channels" });
            }
        }
        app.list_head(lcd, name.as_bytes(), r.as_bytes());
        let rows = self.cur.rows(n, ROWS2, &|i| v.keep(i), |row, i| {
            let (c, db) = if view == V_NEARBY {
                match v.nearest(i) {
                    Some((c, db)) => (Some(c), Some(db)),
                    None => (None, None),
                }
            } else {
                (v.at(i), None)
            };
            if let Some(c) = c {
                let (mut f, mut sub, mut right) = (Buf::<16>::new(), Buf::<48>::new(), Buf::<12>::new());
                mhz_text(c.rx, &mut f);
                let mut l = Buf::<32>::new();
                chan_label(c.num, &mut l);
                sub.push(l.as_bytes());
                if let Some((m, _)) = db {
                    dist_text(m, &mut right);
                    let _ = uwrite!(sub, " \u{b7} {}", Txt(f.as_bytes()));
                } else {
                    right.set(f.as_bytes());
                }
                if view != 0 {
                    sub.push(" \u{b7} ".as_bytes());
                    sub.push(zone_name(store, c.zone).as_bytes());
                }
                let mut t = Buf::<16>::new();
                tone_text(c.cfg.tx_tone, &mut t);
                if !t.is_empty() {
                    let _ = uwrite!(sub, " \u{b7} {}", Txt(t.as_bytes()));
                }
                shift_text(&c.cfg, false, &mut t);
                if !t.is_empty() {
                    let _ = uwrite!(sub, " {}", Txt(t.as_bytes()));
                }
                if c.cfg.mode != 0 {
                    let _ = uwrite!(sub, " \u{b7} {}", MODES[c.cfg.mode as usize % MODES.len()]);
                }
                let r = Row2 {
                    title: c.name(),
                    right: right.as_bytes(),
                    arrow: db.map(|d| Some(d.1)),
                    sub: sub.as_bytes(),
                    sub_right: if c.skip() { b"skip" } else { b"" },
                    selected: i == self.cur.sel,
                    ..Row2::default()
                };
                app.row2(lcd, row, &r);
            }
        });
        app.rows_end(lcd, rows, ROWS2, ROW2_H);
        app.list_cue(lcd, self.cur, n, ROWS2, ROW2_H, rows, &|i| v.keep(i));
        if rows == 0 {
            let (a, b): (&[u8], &[u8]) = match view {
                V_NEARBY if data.fix().is_none() => (b"Needs a GPS fix", b"Nearby sorts by distance from it"),
                V_NEARBY => (b"No channels with a position", b"Set them: ch N pos= or the link"),
                V_RECENT => (b"Nothing recent", b"Channels you tune, use or hear"),
                V_BROADCAST => (b"No stations saved", b"Hold 9 on broadcast saves one"),
                _ => (b"No channels here", b"Hold 9 on a VFO to save one"),
            };
            app.empty(lcd, a, b);
        }
        if app.drawn.hash[R_HELP] != 0 {
            // the box is gone: everything under it again
            app.drawn.hash[R_HELP] = 0;
            app.drawn.hash[R_PAGE] = 0;
        }
    }

    fn hints(self, _: &App) -> Hints {
        match (self.view, self.mode) {
            (PICKER, _) => Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Open"), ("BACK", "Back")]),
            (V_BROADCAST, Mode::List) => Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Listen"), ("BACK", "Views")]),
            (_, Mode::List) => Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Actions"), ("BACK", "Views")]),
            (_, Mode::Actions(_)) => Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Do it"), ("BACK", "List")]),
            (_, Mode::Move(_)) => Hints::new(&[("Knob", "Zone"), ("OK", "Move"), ("BACK", "Cancel")]),
            (_, Mode::Renumber) => Hints::new(&[("Knob", "Number"), (ARROWS_LR, "10"), ("OK", "Set")]),
        }
    }
}

fn zone_name(store: &impl ChannelStore, z: u8) -> Buf<16> {
    store.zone_label(z, b"No zone", b"No zone")
}

/// BCR-31: a saved station into the broadcast receiver (the broadcast slot's while one plays, BCR-25), then the
/// FM screen it was picked from (replaced, not opened again: one BACK leaves FM), or quick settings on a broadcast
/// slot (BCR-28); from elsewhere, FM opens over it.
fn play(app: &mut App, c: Channel) {
    let f = (c.rx / 1000) as u16;
    app.fm_set(f.clamp(super::fm::LO, super::fm::HI)); // the reconciler retunes (MODE-2)
    app.back();
    match app.screen {
        Screen::Fm(f) => app.screen = Screen::Fm(super::fm::Fm { menu: false, ..f }),
        Screen::Quick(_) => {}
        _ => app.open(Dest::FmBroadcast),
    }
    app.show_toast(c.name(), b"", false);
}

/// The active slot onto the channel, at its place in its zone.
fn copy_to_slot(app: &mut App, store: &impl ChannelStore, c: &Channel) {
    if let Some(pos) = (0..store.zone_len(c.zone)).find(|&i| store.zone_at(c.zone, i).is_some_and(|o| o.num == c.num)) {
        app.set_channel(app.active, c.zone, pos, store);
    }
}

/// C8: the zones and views, with how many channels each holds.
fn picker(app: &mut App, lcd: &mut impl Panel, store: &impl ChannelStore, cur: Cursor) {
    let mut l = Buf::<24>::new();
    let _ = uwrite!(l, "{} channels", store.count());
    app.list_head(lcd, l.as_bytes(), b"zones and views");
    let z = store.zones() as u16;
    let rows = cur.rows(picker_n(store), ROWS1, &|i| picker_keep(store, i), |row, i| {
        let mut v = Buf::<12>::new();
        let name: Buf<16> = if i < z {
            // a zone as the zones page shows it: chips and count, dimmed when empty (CHAN-27, CHAN-30)
            let r = super::zones::ZoneRow::of(app, store, i as u8);
            let sel = i == cur.sel;
            if app.dirty(R_ROW + row, hash(&(2u8, &r, sel))) {
                super::zones::zone_row(lcd, LIST_Y + row as i32 * ROW1_H, ROW1_H, &r, sel, app.cue());
            }
            return;
        } else {
            let k = (i - z) as u8 + 1;
            let vw = ViewOf { store, view: k, zone: 0, hidden: true };
            match k {
                V_ALL => {
                    let _ = uwrite!(v, "{}", store.count());
                }
                V_RECENT | V_NEARBY => v.set(b">"),
                _ => {
                    let _ = uwrite!(v, "{}", (0..vw.n()).filter(|&i| vw.keep(i)).count());
                }
            }
            Buf::from(VIEWS[k as usize - 1].as_bytes())
        };
        app.row1(lcd, row, name.as_bytes(), v.as_bytes(), false, i == cur.sel, false);
    });
    app.rows_end(lcd, rows, ROWS1, ROW1_H);
    app.list_cue(lcd, cur, picker_n(store), ROWS1, ROW1_H, rows, &|i| picker_keep(store, i));
    let help: &[u8] = if cur.sel < z { b"A zone's channels, by number" } else {
        match (cur.sel - z) as u8 + 1 {
            V_ALL => b"Every channel, zoned or not",
            V_RECENT => b"Channels heard lately, from the event log",
            V_NEARBY => b"Channels with a position, nearest first",
            V_BROADCAST => b"Saved FM stations: the FM screen's OK",
            _ => b"Every channel on this band",
        }
    };
    app.help_box(lcd, 300 - 38, 34, help, b"MENU 0 2 1");
}

/// C10: the actions for one channel, or picking a zone or number for it.
fn actions(app: &mut App, lcd: &mut impl Panel, store: &impl ChannelStore, b: Browse, ch: Option<Channel>) {
    let Some(c) = ch else {
        return app.empty(lcd, b"No channel", b"");
    };
    let mut l = Buf::<32>::new();
    chan_label(c.num, &mut l);
    app.list_head(lcd, c.name(), l.as_bytes());
    for (i, (name, _)) in ACTIONS.iter().enumerate() {
        let mut v = Buf::<20>::new();
        match i {
            2 => v.set(zone_name(store, c.zone).as_bytes()),
            5 => v.set(if c.skip() { b"On" } else { b"Off" }),
            _ => {}
        }
        if i == 3 {
            let mut t = Buf::<32>::new();
            chan_label(c.num, &mut t);
            v.set(t.as_bytes());
        }
        let (sel, val) = match b.mode {
            Mode::Actions(a) => (a as usize == i, v),
            Mode::Move(z) if i == 2 => (true, Buf::from(zone_name(store, z).as_bytes())),
            Mode::Renumber if i == 3 => {
                let mut t = Buf::<32>::new();
                chan_label(b.cur.top, &mut t);
                (true, Buf::from(t.as_bytes()))
            }
            _ => (false, v),
        };
        app.row1(lcd, i, name.as_bytes(), val.as_bytes(), false, sel, false);
    }
    let help: &[u8] = match b.mode {
        Mode::Actions(a) => ACTIONS[a as usize].1.as_bytes(),
        Mode::Move(_) => b"Knob picks the zone; OK moves it",
        _ => b"Knob steps 1, left and right 10; OK sets it",
    };
    app.help_box(lcd, 300 - 38, 34, help, b"");
}

/// After the shared confirm deleted a channel: the list again, its cursor on a channel that still exists.
pub(crate) fn after_delete(b: Browse, store: &impl ChannelStore, hidden: bool) -> Browse {
    let v = view_of(store, &b, hidden);
    let cur = Cursor { sel: b.cur.sel.min(v.n().saturating_sub(1)), ..b.cur }.shown(v.n(), ROWS2, &|i| v.keep(i));
    Browse { mode: Mode::List, cur, ..b }
}
