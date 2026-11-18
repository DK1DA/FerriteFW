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
use crate::chan::{Channel, ChannelStore, FLAG_HIDDEN, NO_ZONE, ZoneId};
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
use super::zones::NearList;
/// The band views' edges (CHAN-14: "2 m, 70 cm, air, marine"; BCR-31: broadcast FM, 76-108 MHz, the bands of every
/// region, BCR-21).
const BAND_VIEWS: [(Freq10, Freq10); 5] =
    [(mhz(144), mhz(148)), (mhz(420), mhz(450)), (mhz(108), mhz(137)), (mhz(156), mhz(163)), (mhz(76), mhz(108) + 1)];
/// Recent: up to ten channels (CHAN-25).
const RECENT_MAX: u16 = crate::chan::RECENT_MAX as u16;

/// The row actions (C10), in order.
const ACTIONS: [(&str, crate::text::Txt); 8] = [
    ("Edit", txt!("Its settings, in place on the active slot")),
    ("Rename", txt!("Up to 16 characters")),
    ("Move", txt!("To another zone, or none")),
    ("Renumber", txt!("Changes its place in its zone")),
    ("Copy to slot", txt!("The active slot goes to this channel")),
    ("Skip", txt!("Left out of every scan")),
    ("Hide", txt!("Out of the lists; still there by number")),
    ("Delete", txt!("Asks first")),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    List,
    /// The actions menu, its row.
    Actions(u8),
    /// Picking a zone to move to: it is kept in `cur.top` meanwhile (NO_ZONE: none), as [`Mode::Renumber`]'s
    /// number is, so the state stays small with 2-byte zone numbers (decision 333).
    Move,
    /// Picking a new number: it is kept in `cur.top` meanwhile (the list isn't shown), so the state stays
    /// eight bytes.
    Renumber,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Browse {
    /// [`PICKER`]: picking a view (C8); 0: a zone's channels; else 1 + an index into VIEWS.
    pub view: u8,
    pub zone: ZoneId,
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
    zone: ZoneId,
    /// Hidden channels are listed too (the list's options; decision 63).
    hidden: bool,
    /// Nearby's rows (the App's, found when the fix or the records changed).
    near: NearList,
}

impl<S: ChannelStore> ViewOf<'_, S> {
    fn n(&self) -> u16 {
        match self.view {
            0 => self.store.zone_len(self.zone),
            V_RECENT => (0..RECENT_MAX).take_while(|&r| self.recent(r).is_some()).count() as u16,
            V_NEARBY if self.store.data().fix().is_none() => 0,
            V_NEARBY => self.near.n as u16,
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


    /// Nearby: the r-th nearest listed channel with a position, and its distance and bearing (CHAN-15), from the
    /// list found once ([`NearList`]): one record read a row.
    fn nearest(&self, r: u16) -> Option<(Channel, (u32, u16))> {
        if r >= self.near.n as u16 {
            return None;
        }
        let here = self.store.data().fix()?.pos;
        let (c, p) = self.store.located(self.near.nums[r as usize])?;
        Some((c, dist_bearing(here, p)))
    }
}

/// The Nearby view (CHAN-15), opened from the zones page's Nearby row (CHAN-29).
pub(crate) fn nearby_view(store: &impl ChannelStore, app: &mut App) -> Browse {
    let mut b = Browse { view: V_NEARBY, ..Browse::new() };
    let v = view_of(store, &b, app);
    b.cur = Cursor::first(v.n(), &|i| v.keep(i));
    b
}

/// BCR-31: the Broadcast view, the saved FM stations, opened by OK on the FM screen.
pub(crate) fn broadcast_view(store: &impl ChannelStore, hidden: bool) -> Browse {
    opened(store, V_BROADCAST, hidden)
}

fn opened(store: &impl ChannelStore, view: u8, hidden: bool) -> Browse {
    let mut b = Browse { view, ..Browse::new() };
    let v = plain_view(store, &b, hidden);
    b.cur = Cursor::first(v.n(), &|i| v.keep(i));
    b
}

/// The view `b` shows; Nearby's rows found again first when the fix or the records changed.
fn view_of<'a, S: ChannelStore>(store: &'a S, b: &Browse, app: &mut App) -> ViewOf<'a, S> {
    let hidden = app.set.show_hidden;
    let near = if b.view == V_NEARBY { app.near_list.fresh(store, hidden) } else { NearList::NONE };
    ViewOf { store, view: b.view, zone: b.zone, hidden, near }
}

/// A view other than Nearby, without the App.
fn plain_view<'a, S: ChannelStore>(store: &'a S, b: &Browse, hidden: bool) -> ViewOf<'a, S> {
    debug_assert!(b.view != V_NEARBY);
    ViewOf { store, view: b.view, zone: b.zone, hidden, near: NearList::NONE }
}

/// The picker's rows: the zones that exist, then the views.
fn picker_n(store: &impl ChannelStore) -> u16 {
    store.zones() + VIEWS.len() as u16
}

fn picker_keep(store: &impl ChannelStore, i: u16) -> bool {
    i >= store.zones() || store.zone_name(i).is_some()
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
        let v = view_of(store, &self, app);
        let ch = if self.view == PICKER { None } else { v.at(self.cur.sel) };
        match (e.key, e.press, self.mode) {
            (Key::Up | Key::Down, Short | Repeat, _) => return self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Left | Key::Right, Short | Repeat, _) => {
                let d = if e.key == Key::Right { 1 } else { -1 };
                return self.knob(app, if self.mode == Mode::Renumber { d * 10 } else { d }, store);
            }
            // C8: open a zone or a view
            (Key::Menu, Short, Mode::List) if self.view == PICKER => {
                let z = store.zones();
                let i = self.cur.sel;
                (s.view, s.zone) = if i < z { (0, i) } else { (1 + (i - z) as u8, 0) };
                let v = view_of(store, &s, app);
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
                    2 => (s.mode, s.cur.top) = (Mode::Move, c.zone),
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
            (Key::Menu, Short, Mode::Move) => {
                let z = self.cur.top;
                let Some(c) = ch else { return };
                if z != c.zone && !store.zone_room(z, c.num) {
                    app.show_toast(b"Zone full", b"256 channels at most", false); // CHAN-35
                } else if z != c.zone && store.data().request(Op::Move(c.num, z)) {
                    app.show_toast(c.name(), zone_name(store, z).as_bytes(), false);
                }
                s.mode = Mode::List;
                s.cur.top = s.cur.sel;
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
                let row = if self.view == 0 { self.zone } else { store.zones() + self.view as u16 - 1 };
                s = Browse { view: PICKER, cur: Cursor { sel: row, top: 0 }.shown(picker_n(store), ROWS1, &|i| picker_keep(store, i)), ..Browse::new() };
            }
            (Key::Exit, Short, Mode::List) => return app.back(),
            (Key::Exit, Short, Mode::Actions(_)) => s.mode = Mode::List,
            // back to the actions, on the one it came from
            (Key::Exit, Short, m) => {
                s.mode = Mode::Actions(match m {
                    Mode::Move => 2,
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
                let v = view_of(store, &self, app);
                // an empty view: nothing to move to (each turn searched every record twice, sim/tests/limits.rs)
                if shown(app, &v) == 0 {
                    return;
                }
                s.cur = self.cur.step(d, v.n(), ROWS2, &|i| v.keep(i));
            }
            Mode::Actions(a) => s.mode = Mode::Actions((a as i32 + d).rem_euclid(ACTIONS.len() as i32) as u8),
            Mode::Move => {
                let z = self.cur.top;
                // the zones that exist, then "none"
                let n = store.zones() as i32 + 1;
                let mut i = if z == NO_ZONE { n - 1 } else { z as i32 };
                for _ in 0..n {
                    i = (i + d.signum()).rem_euclid(n);
                    if i == n - 1 || store.zone_name(i as ZoneId).is_some() {
                        break;
                    }
                }
                s.cur.top = if i == n - 1 { NO_ZONE } else { i as ZoneId };
            }
            Mode::Renumber => s.cur.top = (self.cur.top as i32 + d).clamp(0, crate::chan::MAX_CHANNELS as i32 - 1) as u16,
        }
        app.screen = Screen::Browse(s);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let store = cx.store;
        let data = store.data();
        let list = self.mode == Mode::List;
        app.page(lcd, (self.view != PICKER, list));
        // the fix only moves Nearby's rows: in another view a new fix redrew the list, and a full radio's count with it
        if !app.gate((self, data.version(), data.fix().map(|f| f.pos).filter(|_| self.view == V_NEARBY))) {
            return;
        }
        if self.view == PICKER {
            return picker(app, lcd, store, self.cur);
        }
        let view = self.view;
        let v = view_of(store, &self, app);
        let n = v.n();
        let ch = v.at(self.cur.sel);
        if !list {
            return actions(app, lcd, store, self, ch);
        }
        let mut name = Buf::<20>::new();
        name.set(if view == 0 { zone_name(store, self.zone) } else { Buf::from(VIEWS[view as usize - 1].as_bytes()) }.as_bytes());
        let shown = shown(app, &v) as usize;
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
            (_, Mode::Move) => Hints::new(&[("Knob", "Zone"), ("OK", "Move"), ("BACK", "Cancel")]),
            (_, Mode::Renumber) => Hints::new(&[("Knob", "Number"), (ARROWS_LR, "10"), ("OK", "Set")]),
        }
    }
}

fn zone_name(store: &impl ChannelStore, z: ZoneId) -> Buf<16> {
    store.zone_label(z, b"No zone", b"No zone")
}

/// BCR-31 (decision 352): a saved station plays on the broadcast slot while one plays (BCR-25); otherwise the
/// active slot's VFO goes there, which makes it the broadcast slot (BCR-21). Then back where it was picked from.
fn play(app: &mut App, c: Channel) {
    let (lo, hi) = app.fm_range();
    let f = ((c.rx / 1000) as u16).clamp(lo, hi);
    if app.radio.bcast.is_some() {
        app.fm_set(f); // the reconciler retunes (MODE-2)
    } else if app.aprs_slot(app.active) {
        return app.show_toast(b"APRS slot", b"pick A or B", false);
    } else {
        super::entry::tune_vfo(app, app.active, f as Freq10 * 1000);
    }
    app.back();
    app.show_toast(c.name(), b"", false);
}

/// The active slot onto the channel, at its place in its zone.
fn copy_to_slot(app: &mut App, store: &impl ChannelStore, c: &Channel) {
    if let Some(pos) = (0..store.zone_len(c.zone)).find(|&i| store.zone_at(c.zone, i).is_some_and(|o| o.num == c.num)) {
        app.set_channel(app.active, c.zone, pos, store);
    }
}

/// How many channels the view shows: a pass over its records where it filters, kept until the view or the records
/// change (sim/tests/limits.rs: a full radio's 2 m view counted 3986 channels on every redraw).
fn shown<S: ChannelStore>(app: &mut App, v: &ViewOf<'_, S>) -> u16 {
    let n = v.n();
    if !((2..=V_BANDS).contains(&v.view) || !v.hidden) {
        return n;
    }
    let key = Mix::new().h(&v.view).h(&v.zone).h(&v.store.data().version()).h(&v.hidden).get() | 1;
    if app.view_count.0 != key {
        app.view_count = (key, (0..n).filter(|&i| v.keep(i)).count() as u16);
    }
    app.view_count.1
}

/// The band views' channel counts (hidden ones too), in one pass over the records, kept until they change: the
/// picker counted each band view with a pass of its own on every redraw, 1.6 s a move on a full radio
/// (sim/tests/limits.rs).
fn band_counts(app: &mut App, store: &impl ChannelStore) -> [u16; BAND_VIEWS.len()] {
    let key = store.data().version() | 1 << 31;
    if app.band_counts.0 != key {
        let mut n = [0u16; BAND_VIEWS.len()];
        for c in (0..store.count()).filter_map(|i| store.by_number(i)) {
            for (k, &(lo, hi)) in BAND_VIEWS.iter().enumerate() {
                if c.rx >= lo && c.rx < hi {
                    n[k] += 1;
                }
            }
        }
        app.band_counts = (key, n);
    }
    app.band_counts.1
}

/// C8: the zones and views, with how many channels each holds.
fn picker(app: &mut App, lcd: &mut impl Panel, store: &impl ChannelStore, cur: Cursor) {
    let mut l = Buf::<24>::new();
    let _ = uwrite!(l, "{} channels", store.count());
    app.list_head(lcd, l.as_bytes(), b"zones and views");
    let z = store.zones();
    let rows = cur.rows(picker_n(store), ROWS1, &|i| picker_keep(store, i), |row, i| {
        let mut v = Buf::<12>::new();
        let name: Buf<16> = if i < z {
            // a zone as the zones page shows it: chips and count, dimmed when empty (CHAN-27, CHAN-30)
            let r = super::zones::ZoneRow::of(app, store, i);
            let sel = i == cur.sel;
            if app.dirty(R_ROW + row, Mix::new().h(&2u8).any(&r).h(&sel).get()) {
                super::zones::zone_row(lcd, LIST_Y + row as i32 * ROW1_H, ROW1_H, &r, sel, app.cue());
            }
            return;
        } else {
            let k = (i - z) as u8 + 1;
            match k {
                V_ALL => {
                    let _ = uwrite!(v, "{}", store.count());
                }
                V_RECENT | V_NEARBY => v.set(b">"),
                _ => {
                    let _ = uwrite!(v, "{}", band_counts(app, store)[k as usize - 2]);
                }
            }
            Buf::from(VIEWS[k as usize - 1].as_bytes())
        };
        app.row1(lcd, row, name.as_bytes(), v.as_bytes(), false, i == cur.sel, false);
    });
    app.rows_end(lcd, rows, ROWS1, ROW1_H);
    app.list_cue(lcd, cur, picker_n(store), ROWS1, ROW1_H, rows, &|i| picker_keep(store, i));
    let help = if cur.sel < z { txt!("A zone's channels, by number") } else {
        match (cur.sel - z) as u8 + 1 {
            V_ALL => txt!("Every channel, zoned or not"),
            V_RECENT => txt!("Channels heard lately, from the event log"),
            V_NEARBY => txt!("Channels with a position, nearest first"),
            V_BROADCAST => txt!("Saved FM stations: the FM screen's OK"),
            _ => txt!("Every channel on this band"),
        }
    };
    app.help_txt(lcd, 300 - 38, 34, help, b"MENU 0 2 1");
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
            Mode::Move if i == 2 => (true, Buf::from(zone_name(store, b.cur.top).as_bytes())),
            Mode::Renumber if i == 3 => {
                let mut t = Buf::<32>::new();
                chan_label(b.cur.top, &mut t);
                (true, Buf::from(t.as_bytes()))
            }
            _ => (false, v),
        };
        app.row1(lcd, i, name.as_bytes(), val.as_bytes(), false, sel, false);
    }
    let help = match b.mode {
        Mode::Actions(a) => ACTIONS[a as usize].1,
        Mode::Move => txt!("Knob picks the zone; OK moves it"),
        _ => txt!("Knob steps 1, left and right 10; OK sets it"),
    };
    app.help_txt(lcd, 300 - 38, 34, help, b"");
}

/// After the shared confirm deleted a channel: the list again, its cursor on a channel that still exists.
pub(crate) fn after_delete(b: Browse, store: &impl ChannelStore, app: &mut App) -> Browse {
    let v = view_of(store, &b, app);
    let cur = Cursor { sel: b.cur.sel.min(v.n().saturating_sub(1)), ..b.cur }.shown(v.n(), ROWS2, &|i| v.keep(i));
    Browse { mode: Mode::List, cur, ..b }
}

#[cfg(test)]
mod nearby_cost {
    use super::*;
    use crate::chan::{Channel, Name, ZoneId};
    use crate::data::{Data, Fix, Pos};
    use core::cell::Cell;
    use std::boxed::Box;
    use super::super::zones::NEAR_MAX;

    /// 400 channels, each with a position, and a count of the position lookups.
    struct Many {
        fix: Option<Fix>,
        looks: Cell<u32>,
    }
    const N: u16 = 400;
    impl Data for Many {
        fn fix(&self) -> Option<Fix> {
            self.fix
        }
    }
    impl ChannelStore for Many {
        fn zones(&self) -> ZoneId {
            1
        }
        fn zone_name(&self, _: ZoneId) -> Option<Name> {
            None
        }
        fn zone_len(&self, _: ZoneId) -> u16 {
            N
        }
        fn zone_at(&self, _: ZoneId, i: u16) -> Option<Channel> {
            self.get(i)
        }
        fn count(&self) -> u16 {
            N
        }
        fn get(&self, num: u16) -> Option<Channel> {
            (num < N).then_some(Channel { num, zone: 0, rx: 14_500_000, name: [b'x'; 16], cfg: crate::slot::DEFAULT_CFG, flags: 0, tones: 0 })
        }
        fn by_number(&self, i: u16) -> Option<Channel> {
            self.get(i)
        }
        fn position(&self, num: u16) -> Option<Pos> {
            self.looks.set(self.looks.get() + 1);
            // spread out, in no order of distance: number k is (k * 37 mod 400) steps away
            (num < N).then(|| Pos { lat: 450_000_000 + ((num as i32 * 37) % N as i32) * 10_000, lon: -757_000_000 })
        }
        fn data(&self) -> &dyn Data {
            self
        }
    }

    /// CHAN-15: a few hundred channels with positions hung the radio on opening Nearby (users' reports, Nov 17,
    /// 2026: the watchdog restarted it). The list is found in one pass and kept; a row is one lookup; and the
    /// order is still nearest first.
    #[test]
    fn nearby_found_once_not_per_row() {
        let here = Pos { lat: 450_000_000, lon: -757_000_000 };
        let s = Many { fix: Some(Fix { pos: here, ..crate::sample::Sample::sample_fix().unwrap() }), looks: Cell::new(0) };
        let set = Box::leak(Box::new(crate::settings::Settings::DEFAULT));
        let mut app = App::new(set, Box::leak(Box::new(crate::screens::alert::AlertInfo::NONE)));
        let b = nearby_view(&s, &mut app);
        let v = view_of(&s, &b, &mut app);
        assert_eq!(v.n(), NEAR_MAX as u16);
        let mut last = 0;
        for r in 0..10 {
            let (c, (m, _)) = v.nearest(r).unwrap();
            assert!(m >= last, "nearest first");
            last = m;
            assert_eq!((c.num as u32 * 37) % N as u32, r as u32, "row {r}");
        }
        assert!(v.nearest(NEAR_MAX as u16).is_none(), "the nearest {NEAR_MAX} only");
        // one pass to find the list, then a lookup a row; again with the same fix and records: no pass
        assert!(s.looks.get() < crate::chan::MAX_CHANNELS as u32 + 20, "{} lookups", s.looks.get());
        let before = s.looks.get();
        let v = view_of(&s, &b, &mut app);
        let _ = v.nearest(3);
        assert!(s.looks.get() - before < 5, "kept, not found again");
    }
}
