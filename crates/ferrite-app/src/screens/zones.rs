//! Zones (MENU 0 2 3; manual chapters 10 and 14, CHAN-6, CHAN-11, CHAN-12, CHAN-27 to CHAN-30; C11) and one zone's
//! settings (A12): name, in "scan all", shown in ◀▶ on home, sort by distance, resume, dwell, priority 1 and 2. The
//! knob or ◀▶ change a setting at once (MENU-16, MENU-19); # resets it (MENU-18). On the list OK puts the active
//! slot on the zone and goes home (decision 178), SLOT opens the zone's editor, # makes a new zone, \* deletes one
//! after asking (CHAN-12: its channels go to no zone and stay in All).
//!
//! The page (CHAN-27, decision 155): a compact list, one line per zone (its name, chips for the active slot's zone,
//! "in scan all" and hidden, its channel count), and under it a preview of the selected zone: its name and state,
//! its first four channels (name, distance, frequency) and a footer. Location shows only where it exists (CHAN-29):
//! with a GPS fix (or the fixed position) and a channel with a position, a pinned Nearby row heads the list with
//! the nearest one's direction and distance, and a previewed channel with a position shows how far it is. Nothing
//! switches by itself: the page only informs.
//!
//! Cost: a move redraws the two rows that changed and the preview; the preview is one pass over the zone's
//! channels (the first four drawn, the rest for the bands they span). The Nearby row's nearest channel is found
//! again only when the fix moved about 100 m or the channels changed ([`NearCache`], 12 bytes in the App).

use super::lists::*;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::{Channel, ChannelStore, FLAG_HIDDEN, MAX_CHANNELS, ZoneId};
use crate::data::{Op, Pos, ZoneCfg};
use crate::keys::{KeyEvent, Press};
use crate::local::LOCAL;
use crate::slot::chan_label;
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_ui::assets::{ICON_GPS, UI_12, UI_12B};
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Zones {
    pub cur: Cursor,
    /// 0 the list on a zone, 1 the list on the Nearby row (CHAN-29; deleting asks with the shared confirm), 2 + row:
    /// the zone editor (A12). One byte keeps the state six bytes.
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

    /// On the Nearby row (while it shows).
    pub fn on_nearby(self) -> bool {
        self.mode == 1
    }
}

// ---- the page's layout (240 x 320) ----

/// The list: one-line rows of 21 px from [`LIST_Y`], seven above the preview (six zones under the Nearby row).
pub(crate) const ZROW_H: i32 = 21;
pub(crate) const ZROWS: u16 = 7;
/// The preview panel: from under the list (y 188) to the hint bar.
const PV_Y: i32 = LIST_Y + ZROWS as i32 * ZROW_H;
pub(crate) const PV_ROWS: usize = 4;
const PV_ROW_H: i32 = 18;
const PV_HEAD_H: i32 = 20;
/// The preview's header band (CHAN-27, decision 178): LINE, the theme's strongest neutral fill, between the list's
/// BG and the channels' SURFACE, so the three parts read apart in every theme.
pub(crate) const PV_BAND: u16 = LINE;
/// Preview columns: the frequency right-aligned at 232 in a column for "439.9875" (8 characters), the distance
/// left-aligned in a column for the widest place ("NW 23.4km", 9), the name in what is left, clipped.
pub(crate) const FREQ_W: i32 = 8 * 7;
pub(crate) const DIST_W: i32 = 9 * 7;
const GAP: i32 = 6;
/// Regions: the list rows are R_ROW..; the preview's header, four rows and footer follow them.
const R_PV: usize = R_ROW + ZROWS as usize;

/// The nearest channel with a position (CHAN-29): its number, distance (m) and bearing (degrees).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Near {
    pub num: u16,
    pub m: u32,
    pub deg: u16,
}

/// The Nearby row's last answer and what it was found for (key 0: not yet; num `u16::MAX`: none). 12 bytes.
#[derive(Clone, Copy, Debug)]
pub struct NearCache {
    key: u32,
    m: u32,
    num: u16,
    deg: u16,
}

impl NearCache {
    pub const NONE: NearCache = NearCache { key: 0, m: 0, num: u16::MAX, deg: 0 };

    fn near(&self) -> Option<Near> {
        (self.num != u16::MAX).then_some(Near { num: self.num, m: self.m, deg: self.deg })
    }
}

/// The Nearby view's rows (CHAN-15): the numbers of the nearest [`NEAR_MAX`] listed channels with a position,
/// nearest first, and what they were found for ([`near_key`]). Found in one pass when the key changes, never per
/// draw: the view read its rows again on every redraw, a pass over the channels each, and a full radio (4000
/// channels with positions) spent seconds a frame (sim/tests/limits.rs). 72 bytes.
#[derive(Clone, Copy, Debug)]
pub struct NearList {
    key: u32,
    pub n: u8,
    pub nums: [u16; NEAR_MAX],
}

/// The Nearby view's length: the nearest this many.
pub const NEAR_MAX: usize = 32;

impl NearList {
    pub const NONE: NearList = NearList { key: 0, n: 0, nums: [0; NEAR_MAX] };

    /// The list for the fix and records now, found again only when they changed.
    pub(crate) fn fresh(&mut self, store: &impl ChannelStore, hidden: bool) -> NearList {
        let Some(here) = here(store) else { return NearList::NONE };
        let key = near_key(here, store, hidden);
        if self.key != key {
            *self = Self::find(store, here, hidden);
            self.key = key;
        }
        *self
    }

    #[inline(never)]
    fn find(store: &impl ChannelStore, here: Pos, hidden: bool) -> NearList {
        let mut buf = [None; NEAR_MAX];
        let total = nearest(store, here, hidden, &mut buf);
        let mut l = NearList { n: total.min(NEAR_MAX as u16) as u8, ..NearList::NONE };
        for (o, b) in l.nums.iter_mut().zip(buf.iter()) {
            *o = b.map_or(0, |b| b.num);
        }
        l
    }
}

/// What the Nearby answer depends on: the fix to about 100 m (0.001 degree), the records, show hidden.
fn near_key(here: Pos, store: &impl ChannelStore, hidden: bool) -> u32 {
    Mix::new().h(&(here.lat / 10_000)).h(&(here.lon / 10_000)).h(&(store.data().version())).h(&hidden).get()
}

/// The place the zones page measures from: the fix, or the fixed position standing in (GPS-3).
pub(crate) fn here(store: &impl ChannelStore) -> Option<Pos> {
    store.data().fix().map(|f| f.pos)
}

/// Whether a channel shows in the lists (CHAN-24: hidden ones only with "show hidden").
fn listed(c: &Channel, hidden: bool) -> bool {
    hidden || c.flags & FLAG_HIDDEN == 0
}

/// The `k` nearest listed channels with a position, by (distance, number), into `out`; how many have one. One
/// pass over the channels, a record read each (CHAN-15, CHAN-29).
pub(crate) fn nearest(store: &impl ChannelStore, here: Pos, hidden: bool, out: &mut [Option<Near>]) -> u16 {
    out.iter_mut().for_each(|o| *o = None);
    let mut n = 0u16;
    for num in 0..MAX_CHANNELS {
        // one look at an empty number, one record read for a channel (decision 333: no zone map to skip by)
        let Some((c, p)) = store.located(num) else { continue };
        if !listed(&c, hidden) {
            continue;
        }
        n += 1;
        let (m, deg) = dist_bearing(here, p);
        let new = Near { num, m, deg };
        // insert in order; a tie goes to the lower number, which came first
        if let Some(i) = out.iter().position(|o| o.is_none_or(|o| (m, num) < (o.m, o.num))) {
            for j in (i + 1..out.len()).rev() {
                out[j] = out[j - 1];
            }
            out[i] = Some(new);
        }
    }
    n
}

/// The Nearby row (CHAN-29): the nearest listed channel with a position, with a fix; None hides the row. Found
/// again only when [`near_key`] changed.
pub(crate) fn nearby(app: &mut App, store: &impl ChannelStore) -> Option<Near> {
    let here = here(store)?;
    let key = near_key(here, store, app.set.show_hidden);
    if app.near.key != key {
        let mut one = [None];
        nearest(store, here, app.set.show_hidden, &mut one);
        let n = one[0].unwrap_or(Near { num: u16::MAX, m: 0, deg: 0 });
        app.near = NearCache { key, m: n.m, num: n.num, deg: n.deg };
    }
    app.near.near()
}

/// [`nearby`] without the App's cache (the readback, LINK-19): the cached answer when it is current.
pub(crate) fn nearby_now(app: &App, store: &impl ChannelStore) -> Option<Near> {
    let here = here(store)?;
    if app.near.key == near_key(here, store, app.set.show_hidden) {
        return app.near.near();
    }
    let mut one = [None];
    nearest(store, here, app.set.show_hidden, &mut one);
    one[0]
}

// ---- the list's rows ----

/// A zone row's chips (CHAN-30): the active slot's zone, "in scan all", hidden from ◀▶.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Chip {
    Slot(u8),
    Scan,
    Hidden,
}

impl Chip {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Chip::Slot(s) => ["A", "B", "C"][s as usize % 3],
            Chip::Scan => "SCAN",
            Chip::Hidden => "HID",
        }
    }
}

/// What a zone's row shows: name, chips, channel count, and whether it is empty (dimmed).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ZoneRow {
    pub name: crate::chan::Name,
    pub chips: [Option<Chip>; 3],
    pub count: u16,
}

impl ZoneRow {
    pub(crate) fn of(app: &App, store: &impl ChannelStore, z: ZoneId) -> ZoneRow {
        let s = app.slot(app.active);
        if z == LOCAL {
            // CHAN-34: no settings of its own, so no SCAN or HID; dimmed while empty (no position, none in range)
            let chip = (s.on_channel() && s.zone == LOCAL).then_some(Chip::Slot(app.active));
            return ZoneRow { name: crate::chan::Name::from(b"Local".as_slice()), chips: [chip, None, None], count: app.local.len() };
        }
        let c = store.data().zone_cfg(z);
        ZoneRow {
            name: store.zone_name(z).unwrap_or_default(),
            // only the active slot's: B and C on a zone say nothing here (decision 155)
            chips: [(s.on_channel() && s.zone == z).then_some(Chip::Slot(app.active)), c.scan_all.then_some(Chip::Scan), (!c.shown).then_some(Chip::Hidden)],
            count: store.zone_len(z),
        }
    }

    /// "A SCAN HID": the chips as words (the readback).
    pub(crate) fn chip_words(&self, b: &mut Buf<16>) {
        for c in self.chips.iter().flatten() {
            if !b.is_empty() {
                b.push(b" ");
            }
            b.push(c.text().as_bytes());
        }
    }
}

/// A chip's width at UI_12B: text plus 4 px each side; the slot's has its dot too.
fn chip_w(c: Chip) -> i32 {
    text4_width(c.text().as_bytes(), &UI_12B) + 8 + if matches!(c, Chip::Slot(_)) { 9 } else { 0 }
}

/// A chip at x on the row at y (15 px tall, the 9 px capitals centred: DISP-19). The active slot's is filled in its
/// colour with a dot ("● A"); SCAN is amber, HID dim, outlined. On a selected row they take the row's dark text.
fn chip(lcd: &mut impl Panel, x: i32, y: i32, c: Chip, selected: bool, bg: u16) -> i32 {
    let wd = chip_w(c);
    let (cy, h) = (y + 3, 15);
    match c {
        Chip::Slot(s) => {
            let (fill, fg) = if selected { (BLUE_FG, w::slot_colour(s)) } else { (w::slot_colour(s), BLUE_FG) };
            lcd.fill_r(x, cy, wd, h, 2, fill);
            lcd.fill_r(x + 4, cy + 5, 6, 6, 3, fg);
            lcd.text4_rows(x + 13, cy + 12, c.text().as_bytes(), &UI_12B, fg, fill, cy + 1, cy + h - 1);
        }
        _ => {
            let fg = if selected { BLUE_FG } else if c == Chip::Scan { AMBER } else { DIM };
            lcd.text4_rows(x + 4, cy + 12, c.text().as_bytes(), &UI_12B, fg, bg, cy + 1, cy + h - 1);
            lcd.frame_r(x, cy, wd, h, 2, fg);
        }
    }
    wd
}

/// One zone's line (CHAN-27): name, chips after it, the count at the right; an empty zone dimmed. In bands, with its
/// part of the scroll cue (DISP-25). The channel browser's zone list draws its zones with it too, `h` tall.
pub(crate) fn zone_row(lcd: &mut impl Panel, y: i32, h: i32, r: &ZoneRow, selected: bool, cue: w::Cue) {
    ferrite_ui::canvas::banded(lcd, 0, y, 240, h, BG, |b| {
        let bg = if selected { BLUE } else { BG };
        let grey = r.count == 0;
        let (fg, sub) = if selected { (BLUE_FG, BLUE_FG) } else if grey { (DIM, DIM) } else { (TEXT, SUB) };
        b.fill(0, y, 240, h, bg);
        let base = y + h / 2 + 5;
        let mut cnt = Buf::<8>::new();
        let _ = uwrite!(cnt, "{}", r.count);
        let cw = text4_width(cnt.as_bytes(), &UI_12);
        b.text4_rows(232 - cw, base, cnt.as_bytes(), &UI_12, sub, bg, y, y + h);
        // the chips keep their room; the name is clipped to what is left
        let chips: i32 = r.chips.iter().flatten().map(|&c| chip_w(c) + 4).sum();
        let room = 232 - cw - 8 - 8 - chips;
        let name = w::fit(r.name.as_bytes(), &UI_12, room);
        let mut x = 8 + b.text4_rows(8, base, name, &UI_12, fg, bg, y, y + h) + 6;
        for &c in r.chips.iter().flatten() {
            x += chip(b, x, y + (h - ZROW_H) / 2, c, selected, bg) + 4;
        }
        cue.draw(b);
    });
}

/// The Nearby row (CHAN-29): the GPS mark in green, "Nearby", and only the nearest channel's place at the right.
fn nearby_row(lcd: &mut impl Panel, y: i32, place: &[u8], selected: bool, cue: w::Cue) {
    ferrite_ui::canvas::banded(lcd, 0, y, 240, ZROW_H, BG, |b| {
        let bg = if selected { BLUE } else { BG };
        let (fg, sub, mark) = if selected { (BLUE_FG, BLUE_FG, BLUE_FG) } else { (TEXT, SUB, GREEN) };
        b.fill(0, y, 240, ZROW_H, bg);
        let base = y + ZROW_H / 2 + 5;
        b.mask4(8, y + 4, &ICON_GPS, mark, None, bg);
        b.text4_rows(24, base, b"Nearby", &UI_12, fg, bg, y, y + ZROW_H);
        let pw = text4_width(place, &UI_12);
        b.text4_rows(232 - pw, base, place, &UI_12, sub, bg, y, y + ZROW_H);
        cue.draw(b);
    });
}

// ---- the preview ----

/// One previewed channel: name, place (empty without a position or a fix) and frequency (CHAN-27, CHAN-28).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct PvRow {
    pub name: crate::chan::Name,
    pub place: Buf<12>,
    pub freq: Buf<16>,
}

/// The preview of a zone or of Nearby: header (name in capitals, state), up to four rows, the footer, and whether
/// the distance column shows (a row has a place).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct Preview {
    pub head: crate::chan::Name,
    pub state: Buf<24>,
    pub rows: [Option<PvRow>; PV_ROWS],
    pub foot: Buf<40>,
}

impl Preview {
    pub(crate) fn dist_col(&self) -> bool {
        self.rows.iter().flatten().any(|r| !r.place.is_empty())
    }
}

/// The bands a zone spans, as the header names them (CHAN-27): in frequency order.
const BAND_LABELS: [(&str, Freq10, Freq10); 9] = [
    ("HF", 0, mhz(50)),
    ("6m", mhz(50), mhz(54)),
    ("air", mhz(108), mhz(137)),
    ("2m", mhz(144), mhz(148)),
    ("marine", mhz(156), mhz(163)),
    ("1.25m", mhz(220), mhz(225)),
    ("70cm", mhz(420), mhz(450)),
    ("VHF", mhz(54), mhz(300)),
    ("UHF", mhz(300), u32::MAX),
];

fn band_bit(f: Freq10) -> u16 {
    // the first label that holds it: a ham or service band before the plain VHF and UHF
    BAND_LABELS.iter().position(|&(_, lo, hi)| f >= lo && f < hi).map_or(0, |i| 1 << i)
}

/// "2m · 70cm": at most three, then "+N".
fn bands_text(bits: u16, b: &mut Buf<24>) {
    // frequency order: HF, 6m, VHF (54-300 outside the named bands sorts by its low edge), ...
    const ORDER: [usize; 9] = [0, 1, 7, 2, 3, 4, 5, 6, 8];
    let total = bits.count_ones();
    for (shown, &i) in ORDER.iter().filter(|&&i| bits & 1 << i != 0).enumerate() {
        if shown == 3 {
            let _ = uwrite!(b, " +{}", total - 3);
            break;
        }
        if shown > 0 {
            b.push(" \u{b7} ".as_bytes());
        }
        b.push(BAND_LABELS[i].0.as_bytes());
    }
}

fn caps(s: &[u8]) -> crate::chan::Name {
    let mut n = crate::chan::Name::new();
    for &c in s {
        n.push(&[c.to_ascii_uppercase()]);
    }
    n
}

/// A previewed row for channel `c`: its place from `here` when both it and the fix have a position.
fn pv_row(app: &App, store: &impl ChannelStore, c: &Channel, here: Option<Pos>, near: Option<Near>) -> PvRow {
    let mut r = PvRow { name: crate::chan::Name::from(c.name()), ..PvRow::default() };
    if r.name.is_empty() {
        let mut l = Buf::<32>::new();
        chan_label(c.num, &mut l);
        r.name.set(l.as_bytes());
    }
    mhz_text(c.rx, &mut r.freq);
    let place = match near {
        Some(n) => Some((n.m, n.deg)),
        None => here.zip(store.position(c.num)).map(|(h, p)| dist_bearing(h, p)),
    };
    if let Some((m, deg)) = place {
        place_text(m, deg, app.set.units, &mut r.place);
    }
    r
}

/// The preview of zone `z` (CHAN-27): one pass over its channels in number order (CHAN-10): the first four listed
/// ones drawn, every one counted for "+N more" and the bands. Header right: "scan on", else the bands; footer: "+N
/// more", then "★ NAME" for the first priority channel while in scan (the star, not the word "priority", so a full
/// name fits 240 px without scrolling: decision 157), or "scan off".
pub(crate) fn preview_zone(app: &App, store: &impl ChannelStore, z: ZoneId) -> Preview {
    if z == LOCAL {
        return preview_local(app, store);
    }
    let hidden = app.set.show_hidden;
    let here = here(store);
    let cfg = store.data().zone_cfg(z);
    let mut p = Preview { head: caps(store.zone_name(z).unwrap_or_default().as_bytes()), ..Preview::default() };
    let (mut bits, mut listed_n, mut filled) = (0u16, 0u16, 0usize);
    // the zone's own channels in order (decision 333: a slot's list, or paged through flash)
    for i in 0..store.zone_len(z) {
        let Some(c) = store.zone_at(z, i) else { continue };
        bits |= band_bit(c.rx);
        if !listed(&c, hidden) {
            continue;
        }
        listed_n += 1;
        if filled < PV_ROWS {
            p.rows[filled] = Some(pv_row(app, store, &c, here, None));
            filled += 1;
        }
    }
    if cfg.scan_all {
        p.state.set(b"scan on");
    } else if bits == 0 {
        p.state.set(b"empty");
    } else {
        bands_text(bits, &mut p.state);
    }
    let more = listed_n.saturating_sub(filled as u16);
    if more > 0 {
        let _ = uwrite!(p.foot, "+{} more", more);
    }
    let tail: Option<Buf<24>> = if cfg.scan_all {
        cfg.pri[0].and_then(|n| store.get(n)).map(|c| {
            let mut t = Buf::<24>::from("\u{2605} ".as_bytes());
            t.push(pv_row(app, store, &c, None, None).name.as_bytes());
            t
        })
    } else {
        Some(Buf::from(b"scan off".as_slice()))
    };
    if let Some(t) = tail {
        if !p.foot.is_empty() {
            p.foot.push(" \u{b7} ".as_bytes());
        }
        p.foot.push(t.as_bytes());
    }
    p
}

/// The preview of Local (CHAN-34): "within 50 km" and its four nearest with their places, "+N more" for the rest;
/// without a position "no position" and what it needs; none in range says so.
fn preview_local(app: &App, store: &impl ChannelStore) -> Preview {
    let l = app.local;
    let mut p = Preview { head: crate::chan::Name::from(b"LOCAL".as_slice()), ..Preview::default() };
    let km = crate::local::range_km(app);
    match l.state() {
        crate::local::State::Ready => _ = uwrite!(p.state, "within {} km", km),
        _ => {
            p.state.set(b"no position");
            p.foot.set(b"Needs a GPS fix, or set 0 6 9");
            return p;
        }
    }
    let here = here(store);
    let mut filled = 0;
    for c in (0..l.len()).filter_map(|i| app.zat(store, LOCAL, i)).filter(|c| listed(c, app.set.show_hidden)) {
        if filled < PV_ROWS {
            p.rows[filled] = Some(pv_row(app, store, &c, here, None));
        }
        filled += 1;
    }
    if filled > PV_ROWS {
        let _ = uwrite!(p.foot, "+{} more \u{b7} nearest first", filled - PV_ROWS);
    } else if filled == 0 {
        let _ = uwrite!(p.foot, "No channel with a position within {} km", km);
    } else {
        p.foot.set(b"nearest first");
    }
    p
}

/// The preview of Nearby (CHAN-29): the four nearest channels with a position, "+N more" for the rest.
pub(crate) fn preview_nearby(app: &App, store: &impl ChannelStore) -> Preview {
    let mut p = Preview { head: crate::chan::Name::from(b"NEARBY".as_slice()), state: Buf::from(b"nearest first".as_slice()), ..Preview::default() };
    let Some(here) = here(store) else { return p };
    let mut best = [None; PV_ROWS];
    let n = nearest(store, here, app.set.show_hidden, &mut best);
    for (slot, b) in p.rows.iter_mut().zip(best) {
        *slot = b.and_then(|b| store.get(b.num).map(|c| pv_row(app, store, &c, None, Some(b))));
    }
    let shown = p.rows.iter().flatten().count() as u16;
    if n > shown {
        let _ = uwrite!(p.foot, "+{} more", n - shown);
    }
    p
}

/// A preview row's columns (CHAN-27): the name from 8 to its end, the place from its x, the frequency from its x to
/// 232; none overlaps, whatever the name's length (it is clipped) or the frequency's.
pub(crate) fn pv_columns(r: &PvRow, dist_col: bool) -> (&[u8], i32, i32) {
    let fw = text4_width(r.freq.as_bytes(), &UI_12);
    let fx = 232 - fw.max(FREQ_W);
    let dx = if dist_col { fx - GAP - DIST_W } else { fx };
    (w::fit(r.name.as_bytes(), &UI_12, dx - GAP - 8), dx, 232 - fw)
}

fn pv_row_draw(lcd: &mut impl Panel, y: i32, r: Option<&PvRow>, dist_col: bool) {
    ferrite_ui::canvas::banded(lcd, 0, y, 240, PV_ROW_H, SURFACE, |b| {
        b.fill(0, y, 240, PV_ROW_H, SURFACE);
        let Some(r) = r else { return };
        let base = y + 13;
        let (name, dx, fx) = pv_columns(r, dist_col);
        b.text4_rows(8, base, name, &UI_12, TEXT, SURFACE, y, y + PV_ROW_H);
        if dist_col && !r.place.is_empty() {
            let place = w::fit(r.place.as_bytes(), &UI_12, fx - GAP - dx);
            b.text4_rows(dx, base, place, &UI_12, GREEN, SURFACE, y, y + PV_ROW_H);
        }
        b.text4_rows(fx, base, r.freq.as_bytes(), &UI_12, SUB, SURFACE, y, y + PV_ROW_H);
    });
}

impl App {
    /// The preview panel (CHAN-27): each part redrawn when it changed.
    fn preview(&mut self, lcd: &mut impl Panel, p: &Preview) {
        if self.dirty(R_PV, Mix::new().h(&p.head).h(&p.state).get()) {
            // A filled band (decision 178): the list (BG) above it and the channels (SURFACE) under it can't be
            // mistaken for each other; a DIM rule on its top edge ends the list.
            ferrite_ui::canvas::banded(lcd, 0, PV_Y, 240, PV_HEAD_H, PV_BAND, |b| {
                b.fill(0, PV_Y, 240, PV_HEAD_H, PV_BAND);
                b.fill(0, PV_Y, 240, 2, DIM);
                let sw = text4_width(p.state.as_bytes(), &UI_12);
                b.text4_rows(232 - sw, PV_Y + 16, p.state.as_bytes(), &UI_12, SUB, PV_BAND, PV_Y + 2, PV_Y + PV_HEAD_H);
                let name = w::fit(p.head.as_bytes(), &UI_12B, 232 - sw - 8 - 8);
                b.text4_rows(8, PV_Y + 16, name, &UI_12B, TEXT, PV_BAND, PV_Y + 2, PV_Y + PV_HEAD_H);
            });
        }
        let dist = p.dist_col();
        for (i, r) in p.rows.iter().enumerate() {
            if self.dirty(R_PV + 1 + i, Mix::new().any(&r).h(&dist).get()) {
                pv_row_draw(lcd, PV_Y + PV_HEAD_H + i as i32 * PV_ROW_H, r.as_ref(), dist);
            }
        }
        let fy = PV_Y + PV_HEAD_H + PV_ROWS as i32 * PV_ROW_H;
        if self.dirty(R_PV + 1 + PV_ROWS, Mix::new().h(&p.foot).get()) {
            ferrite_ui::canvas::banded(lcd, 0, fy, 240, w::HINT_Y - fy, SURFACE, |b| {
                b.fill(0, fy, 240, w::HINT_Y - fy, SURFACE);
                b.text4_rows(8, fy + 14, w::fit(p.foot.as_bytes(), &UI_12, 224), &UI_12, DIM, SURFACE, fy, w::HINT_Y);
            });
        }
    }
}

/// "6 zones · 21 ch": the zones shown and every channel stored.
pub(crate) fn head_text(store: &impl ChannelStore) -> Buf<32> {
    let n = (0..store.zones()).filter(|&i| keep(store, false)(i)).count();
    let mut l = Buf::<32>::new();
    let _ = uwrite!(l, "{} {} \u{b7} {} ch", n, if n == 1 { "zone" } else { "zones" }, store.count());
    l
}

/// The zone list's page: with the Nearby row, one row fewer for zones.
pub(crate) fn zone_rows(near: bool) -> u16 {
    if near { ZROWS - 1 } else { ZROWS }
}


/// The editor's rows: label and help (layout as data; values from [`value`]).
const ROWS: [(&str, crate::text::Txt); 8] = [
    ("Name", txt!("Up to 16 characters")),
    ("Scan all", txt!("In the set \"zones in scan all\" scans")),
    ("Shown on home", txt!("Home's left and right stop on it (CHAN-11)")),
    ("Sort by distance", txt!("Nearest channel first, with a GPS fix")),
    ("Resume", txt!("After a hit; the radio's rule unless set")),
    ("Dwell", txt!("Wait before resuming")),
    ("Priority 1", txt!("Checked during scans (SCAN-12)")),
    ("Priority 2", txt!("Checked during scans (SCAN-12)")),
];

/// The defaults, for the dot (MENU-12) and # (MENU-18).
const DEFAULT: ZoneCfg = ZoneCfg { scan_all: false, shown: true, by_distance: false, resume: 0, dwell: 255, pri: [None, None] };

/// The list's rows that exist: the stored zones, and Local after them when it is on (CHAN-34).
pub(crate) fn keep(store: &impl ChannelStore, local: bool) -> impl Fn(u16) -> bool + '_ {
    move |i| store.zone_name(i).is_some() || local && i == store.zones()
}

/// The list's length and whether Local is in it (after the stored zones, CHAN-34).
pub(crate) fn list_n(app: &App, store: &impl ChannelStore) -> (u16, bool) {
    let local = crate::local::on(app);
    (store.zones() + local as u16, local)
}

/// The zone on list row `i`: a stored zone's number, or Local after them (CHAN-34).
pub(crate) fn zid(store: &impl ChannelStore, i: u16) -> ZoneId {
    if i == store.zones() { LOCAL } else { i }
}

fn value(store: &impl ChannelStore, z: ZoneId, c: &ZoneCfg, row: usize, b: &mut Buf<24>) {
    let on = |v: bool| if v { "On" } else { "Off" };
    b.clear();
    let _ = match row {
        0 => {
            b.set(store.zone_name(z).unwrap_or_else(|| crate::chan::Name::from(b"")).as_bytes());
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
        _ => match c.pri[row - 6].and_then(|n| (0..store.zone_len(z)).filter_map(|i| store.zone_at(z, i)).find(|ch| ch.num == n)) {
            // the channel's name (the number and name together didn't fit beside "Priority 1" at 14 px); the
            // number when it has none
            Some(ch) if !ch.name().is_empty() => {
                b.push(ch.name());
                Ok(())
            }
            Some(ch) => {
                let mut l = Buf::<32>::new();
                chan_label(ch.num, &mut l);
                b.push(l.as_bytes());
                Ok(())
            }
            None => uwrite!(b, "None"),
        },
    };
}

/// The editor's rows, for the readback (LINK-19).
pub(crate) const EDITOR_ROWS: usize = ROWS.len();

/// Editor row `i`: its label, value and whether it differs from the default (MENU-12).
pub(crate) fn editor_row(store: &impl ChannelStore, z: ZoneId, i: usize) -> (&'static str, Buf<24>, bool) {
    let c = store.data().zone_cfg(z);
    let (mut v, mut dv) = (Buf::<24>::new(), Buf::<24>::new());
    value(store, z, &c, i, &mut v);
    value(store, z, &DEFAULT, i, &mut dv);
    (ROWS[i].0, v, i > 0 && v != dv)
}

/// A setting stepped by d.
fn change(store: &impl ChannelStore, z: ZoneId, c: &mut ZoneCfg, row: usize, d: i32) {
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
        6 | 7 => {
            // None, then the zone's channels in order
            let n = store.zone_len(z) as i32;
            let at = c.pri[row - 6].and_then(|num| (0..store.zone_len(z)).position(|i| store.zone_at(z, i).is_some_and(|ch| ch.num == num)));
            let i = (at.map_or(-1, |p| p as i32) + d + 1).rem_euclid(n + 1) - 1;
            c.pri[row - 6] = if i < 0 { None } else { store.zone_at(z, i as u16).map(|ch| ch.num) };
        }
        _ => {}
    }
}

impl Zones {
    /// One row up (-1) or down (+1) through the Nearby row (when it shows) and the zones, wrapping (CHAN-29).
    fn step(self, near: bool, app: &App, store: &impl ChannelStore, dir: i32) -> Zones {
        let (n, local) = list_n(app, store);
        let k = keep(store, local);
        let vis = zone_rows(near);
        let first = (0..n).find(|&i| k(i));
        let last = (0..n).rev().find(|&i| k(i));
        if self.on_nearby() && near {
            return match if dir > 0 { first } else { last } {
                Some(i) => Zones { cur: Cursor { sel: i, top: self.cur.top }.shown(n, vis, &k), mode: 0 },
                None => self,
            };
        }
        if near && (dir < 0 && Some(self.cur.sel) == first || dir > 0 && Some(self.cur.sel) == last) {
            return Zones { mode: 1, ..self };
        }
        Zones { cur: self.cur.step(dir, n, vis, &k).shown(n, vis, &k), mode: 0 }
    }

    /// The list as it shows now: off the Nearby row when it has gone.
    fn settled(self, near: bool) -> Zones {
        if self.on_nearby() && !near { Zones { mode: 0, ..self } } else { self }
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
        let near = self.edit().is_none() && nearby(app, store).is_some();
        let mut s = self.settled(near);
        let z = if s.edit().is_some() { s.cur.sel }  else { zid(store, s.cur.sel) };
        let d = store.data();
        match (e.key, e.press, s.edit()) {
            (Key::Up | Key::Down, Short | Repeat, _) => {
                let dir = if e.key == Key::Up { -1 } else { 1 };
                match s.edit() {
                    Some(r) => s.mode = 2 + (r as i32 + dir).rem_euclid(ROWS.len() as i32) as u8,
                    None => s = s.step(near, app, store, dir),
                }
            }
            // OK on Nearby: the channel browser's Nearby view (CHAN-29)
            (Key::Menu, Short, None) if s.on_nearby() => {
                let b = super::browse::nearby_view(store, app);
                return app.open_screen(Screen::Browse(b));
            }
            // OK on a zone: the active slot takes it and home shows it (CHAN-27, decision 178)
            (Key::Menu, Short, None) if store.zone_name(z).is_some() || z == LOCAL => return accept(app, store, z),
            // SLOT on Local: its setting, MENU 0 2 5 (CHAN-34: Local has no zone settings); BACK returns here
            (Key::Slot, Short, None) if z == LOCAL && !s.on_nearby() => return app.open_screen(Screen::Group(super::groups::Group { g: 2, sub: 0, row: 4 })),
            // SLOT on a zone: its editor (A12); SLOT edits, as hold SLOT on home and EDIT-6
            (Key::Slot, Short, None) if !s.on_nearby() && store.zone_name(z).is_some() => s.mode = 2,
            (Key::Menu, Short, Some(0)) => return super::text::open(app, super::text::TextFor::Zone(z), store), // rename (A7)
            (Key::Hash, Short, None) => {
                if d.request(Op::NewZone) {
                    let (n, local) = list_n(app, store);
                    let made = store.zones() - 1;
                    s.cur = Cursor { sel: made, top: s.cur.top }.shown(n, zone_rows(near), &keep(store, local));
                    s.mode = 0;
                    app.show_toast(b"New zone", store.zone_name(made).unwrap_or_default().as_bytes(), false);
                } else {
                    let mut m = Buf::<24>::new();
                    let _ = uwrite!(m, "{} zones at most", crate::chan::MAX_ZONES);
                    app.show_toast(b"No room", m.as_bytes(), false);
                }
            }
            // CHAN-12, MENU-23: the shared confirm asks.
            (Key::Star, Short, None) if !s.on_nearby() && store.zone_name(z).is_some() => return super::confirm::ask(app, super::confirm::Ask::DeleteZone(z)),
            (Key::Hash, Short, Some(r)) => {
                // MENU-18: back to the default
                let mut c = d.zone_cfg(z);
                match r {
                    1 => c.scan_all = DEFAULT.scan_all,
                    2 => c.shown = DEFAULT.shown,
                    3 => c.by_distance = DEFAULT.by_distance,
                    4 => c.resume = DEFAULT.resume,
                    5 => c.dwell = DEFAULT.dwell,
                    6 | 7 => c.pri[r as usize - 6] = None,
                    _ => {}
                }
                d.request(Op::Zone(z, c));
            }
            // BACK, or SLOT again (EDIT-6): back to the list on the zone
            (Key::Exit | Key::Slot, Short, Some(_)) => s.mode = 0,
            (Key::Exit, Short, None) => return app.back(),
            _ => return,
        }
        app.screen = Screen::Zones(s);
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        match self.edit() {
            Some(r) if r > 0 => {
                let z = self.cur.sel;
                let mut c = store.data().zone_cfg(z);
                change(store, z, &mut c, r as usize, d);
                store.data().request(Op::Zone(z, c));
            }
            Some(_) => {}
            None => {
                let near = nearby(app, store).is_some();
                let mut s = self.settled(near);
                for _ in 0..d.unsigned_abs().min(255) {
                    s = s.step(near, app, store, d.signum());
                }
                app.screen = Screen::Zones(s);
            }
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let store = cx.store;
        let data = store.data();
        app.page(lcd, self.edit().is_some());
        // what the page shows besides its state: the records, the fix (to ~100 m), the active slot's zone, the units
        // and "show hidden"
        let s = app.slot(app.active);
        let at = (app.active, s.on_channel().then_some(s.zone));
        let fix = here(store).map(|p| (p.lat / 10_000, p.lon / 10_000));
        if !app.gate((self, data.version(), fix, at, app.set.units, app.set.show_hidden, app.local)) {
            return;
        }
        let z = self.cur.sel;
        if let Some(row) = self.edit() {
            let c = data.zone_cfg(z);
            let mut r = Buf::<16>::new();
            let _ = uwrite!(r, "{} channels", store.zone_len(z));
            app.list_head(lcd, store.zone_name(z).unwrap_or_default().as_bytes(), r.as_bytes());
            for (i, (label, _)) in ROWS.iter().enumerate() {
                let mut v = Buf::<24>::new();
                value(store, z, &c, i, &mut v);
                let mut dv = Buf::<24>::new();
                value(store, z, &DEFAULT, i, &mut dv);
                app.row1(lcd, i, label.as_bytes(), v.as_bytes(), i > 0 && v != dv, i as u8 == row, false);
            }
            let mut p = Buf::<16>::new();
            let _ = uwrite!(p, "MENU 0 2 3");
            app.help_txt(lcd, 300 - 38, 34, ROWS[row as usize].1, p.as_bytes());
            return;
        }
        let near = nearby(app, store);
        let s = self.settled(near.is_some());
        let (n, local) = list_n(app, store);
        let k = keep(store, local);
        app.list_head(lcd, head_text(store).as_bytes(), b"");
        let mut first = 0;
        if let Some(nr) = near {
            let mut place = Buf::<12>::new();
            place_text(nr.m, nr.deg, app.set.units, &mut place);
            if app.dirty(R_ROW, Mix::new().h(&1u8).h(&place).h(&s.on_nearby()).get()) {
                nearby_row(lcd, LIST_Y, place.as_bytes(), s.on_nearby(), app.cue());
            }
            first = 1;
        }
        let vis = zone_rows(near.is_some());
        let cur = s.cur.shown(n, vis, &k);
        let rows = cur.rows(n, vis, &k, |row, i| {
            let r = ZoneRow::of(app, store, zid(store, i));
            let sel = !s.on_nearby() && i == cur.sel;
            let at = first + row;
            if app.dirty(R_ROW + at, Mix::new().h(&2u8).any(&r).h(&sel).get()) {
                zone_row(lcd, LIST_Y + at as i32 * ZROW_H, ZROW_H, &r, sel, app.cue());
            }
        });
        for i in first + rows..ZROWS as usize {
            if app.dirty(R_ROW + i, 1) {
                lcd.fill(0, LIST_Y + i as i32 * ZROW_H, 240, ZROW_H, BG);
            }
        }
        let more = cur.more(n, vis, rows, &k);
        app.scroll_cue(lcd, (first, vis as usize), LIST_Y + first as i32 * ZROW_H, vis as i32 * ZROW_H, more, cur.top as u32, vis as u32, n as u32);
        if rows == 0 {
            app.empty(lcd, b"No zones", b"# makes one");
        } else {
            app.drawn.hash[R_NOTE] = 0;
        }
        let p = if s.on_nearby() {
            preview_nearby(app, store)
        } else if rows > 0 {
            preview_zone(app, store, zid(store, cur.sel))
        } else {
            Preview::default()
        };
        app.preview(lcd, &p);
    }

    fn hints(self, _: &App) -> Hints {
        match self.edit() {
            Some(_) => Hints::new(&[(ARROWS_UD, "Move"), ("Knob", "Change"), ("#", "Reset")]),
            None if self.on_nearby() => Hints::new(&[("OK", "Open"), ("#", "New"), ("BACK", "Back")]),
            None => Hints::new(&[("OK", "Use"), ("ABC", "Edit"), ("#", "New")]), // KEY-9: three; \* still deletes (CHAN-12)
        }
    }
}

/// Where OK on zone `z` puts the active slot (CHAN-27, decision 178): its position in the zone. The slot's own last
/// channel there when it was last on this zone (`zone` and `pos` stay while it is on its VFO), else the zone's first
/// channel a slot can sit on in number order (CHAN-10): listed (CHAN-24: hidden ones only with "show hidden") and
/// not an HF channel (CHAN-26). None: no such channel.
pub(crate) fn accept_pos(app: &App, store: &impl ChannelStore, z: ZoneId) -> Option<u16> {
    let hidden = app.set.show_hidden;
    let usable = |p: u16| app.zat(store, z, p).is_some_and(|c| !c.is_hf() && listed(&c, hidden));
    let s = app.slot(app.active);
    if s.zone == z && s.ch.is_some() && usable(s.pos) {
        return Some(s.pos);
    }
    (0..app.zlen(store, z)).find(|&p| usable(p))
}

/// OK on zone `z` (CHAN-27, decision 178): the active slot goes to channel mode on [`accept_pos`] and the page goes
/// home, where ◀▶ and the knob go on from there (NAV-1). The APRS slot refuses (APRS-19, KISS-9) and an empty zone
/// says so; both stay on the page. A retune ends a scan on the slot, as ◀▶ on home does; a slot already on that
/// channel is left as it is.
fn accept(app: &mut App, store: &impl ChannelStore, z: ZoneId) {
    let a = app.active;
    if app.aprs_refuse(a) {
        return;
    }
    let Some(pos) = accept_pos(app, store, z) else {
        return if z == LOCAL && app.local.is_empty() {
            crate::local::refuse(app) // CHAN-34: no position, or none in range
        } else if app.zlen(store, z) == 0 {
            app.show_toast(b"No channels", b"in this zone", false)
        } else {
            app.show_toast(b"No channels", b"HF or hidden only", false)
        };
    };
    let s = *app.slot(a);
    if !(s.on_channel() && s.zone == z && s.pos == pos) {
        app.set_channel(a, z, pos, store);
    }
    app.go_home();
    let mut h = Buf::<20>::new();
    let _ = uwrite!(h, "Slot {}", w::LETTERS[a as usize] as char);
    app.show_toast(h.as_bytes(), store.zone_label(z, b"", b"").as_bytes(), false);
}

/// After the shared confirm deleted a zone: the list again from its first zone.
pub(crate) fn after_delete(store: &impl ChannelStore) -> Zones {
    Zones { mode: 0, cur: Cursor::first(store.zones(), &keep(store, false)) }
}

#[cfg(test)]
#[path = "zones_tests.rs"]
mod tests;
