//! What the screens share for drawing: region dirty-tracking, the status bar, the hint bar and toast, list
//! rows, the edit grid's cells, the help box, and the slot texts several screens show. Each region of the
//! screen is hashed from what it shows; a screen draws a region only when its hash changed (HOME-7), so a meter
//! update never touches the frequency and a still screen costs nothing. Layouts follow the look review's
//! mock-ups (`docs/img/screens`).

use crate::chan::ChannelStore;
use crate::slot::{band_name, chan_label};
use crate::App;
use core::hash::{Hash, Hasher};
use ferrite_core::msg::{Device, LiveAll};
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row, StatusBar};

// Regions. Screens reuse the numbers; a screen change clears the content and forgets every hash.
pub(crate) const R_HINT: usize = 0;
pub(crate) const R_FRAME: usize = 1; // home: three cards' frames (1-3)
pub(crate) const R_HEAD: usize = 4; // card headers (4-6)
pub(crate) const R_FREQ: usize = 7; // frequencies (7-9)
pub(crate) const R_INFO: usize = 10; // chips, or the compact card's name (10-12)
pub(crate) const R_SUB: usize = 13; // S-meter text, or the compact card's second line (13-15)
pub(crate) const R_ROW: usize = 1; // lists and the edit grid: rows 1-13
pub(crate) const R_TOP: usize = 14; // menu header / Recent row / edit header
pub(crate) const R_HELP: usize = 15;
pub(crate) const R_NOTE: usize = 16;
pub(crate) const R_LINE: usize = 17; // edit: the other two slots (17-18)
pub(crate) const R_PAGE: usize = 19; // a screen's page: see App::new_page
pub(crate) const REGIONS: usize = 20;

/// What a screen draws from besides the App: the Radio task's snapshot, the radio-wide state, the channels.
pub struct Cx<'a, S: ChannelStore> {
    pub live: &'a LiveAll,
    pub dev: &'a Device,
    pub store: &'a S,
}

/// The hint bar's key + label pairs (KEY-9): at most three ([`w::HINT_MAX`]; four overflow 240 px at 12 px).
/// One rule: a screen lists its three most important, in the order they show; what it leaves out first is the
/// universal (BACK Back, which every screen has), then the least used of its own keys.
#[derive(Clone, Copy, Debug)]
pub struct Hints {
    items: [(&'static str, &'static str); w::HINT_MAX],
    n: u8,
}

impl Hints {
    pub const fn new(items: &[(&'static str, &'static str)]) -> Self {
        assert!(items.len() <= w::HINT_MAX, "at most three hints (KEY-9)");
        let mut h = Hints { items: [("", ""); w::HINT_MAX], n: 0 };
        while (h.n as usize) < items.len() && (h.n as usize) < w::HINT_MAX {
            h.items[h.n as usize] = items[h.n as usize];
            h.n += 1;
        }
        h
    }

    pub fn as_slice(&self) -> &[(&'static str, &'static str)] {
        &self.items[..self.n as usize]
    }
}

pub(crate) const ARROWS_LR: &str = "\x1b\x1a";
pub(crate) const ARROWS_UD: &str = "\x18\x19";
pub(crate) const ARROWS_4: &str = "\x1b\x18\x19\x1a";

/// FNV-1a, for region hashes.
struct Fnv(u32);
impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0 as u64
    }
    fn write(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ x as u32).wrapping_mul(16_777_619);
        }
    }
}

pub(crate) fn hash<T: Hash>(t: &T) -> u32 {
    let mut h = Fnv(2_166_136_261);
    t.hash(&mut h);
    h.0 | 1 // never 0, which means "not drawn"
}

/// Off, A+B, A+B+C.
pub(crate) const WATCH_TEXT: [&str; 3] = ["Off", "A+B", "A+B+C"];

impl App {
    /// True (and remembered) when region r's content differs from what is on screen.
    pub(crate) fn dirty(&mut self, r: usize, h: u32) -> bool {
        let d = self.drawn.hash[r] != h;
        self.drawn.hash[r] = h;
        d
    }

    /// A screen with pages (setup, About, a list and its detail): when the page changes, the content area is
    /// cleared and every region but the hint bar forgotten, as on a screen change. Returns true then.
    pub(crate) fn page<K: Hash>(&mut self, lcd: &mut impl Panel, key: K) -> bool {
        if !self.dirty(R_PAGE, hash(&key) | 1) {
            return false;
        }
        lcd.fill(0, w::SBAR_H, 240, w::HINT_Y - w::SBAR_H, BG);
        self.drawn.hash[R_HINT + 1..R_PAGE].fill(0);
        true
    }

    /// [`App::page`] by number.
    pub(crate) fn new_page(&mut self, lcd: &mut impl Panel, page: u32) -> bool {
        self.page(lcd, page)
    }

    pub(crate) fn status_bar(&self, live: &LiveAll, dev: &Device) -> StatusBar {
        let mut name = Buf::<10>::new();
        self.screen.title(self, &mut name);
        // SBAR-16: TW with three slots watched, DW with two, counting the relay rule's drops.
        let m = self.watch_mask() & !self.relay_mask;
        // HF-5: in HF mode TW/DW and SCN are absent (everything pauses); SBAR-18: the alerts-paused bell.
        // TX-34: watching pauses while transmitting, so no TW/DW then either.
        let hf = super::hf::in_hf(self);
        // SBAR-16: only where watch matters (home, the Status page, Watch and scan settings); elsewhere it
        // cost the screen name its room.
        let here = matches!(self.screen, crate::Screen::Home | crate::Screen::Status(_) | crate::Screen::Group(super::groups::Group { g: 1, .. }));
        let n = if hf || self.radio.keyed.is_some() || !here { 0 } else { m.count_ones() as u8 };
        StatusBar {
            name,
            locked: self.locked,
            // SBAR-11: the bar shows what changes what a key does, so SCN is the active slot scanning; a
            // watched slot's scan shows on its card.
            scan: !hf && live.slots[self.active as usize].scanning,
            watch: if self.watch == 0 || n < 2 { 0 } else { n },
            inbox: self.inbox,
            alert: self.alert,
            clock: dev.clock_min,
            utc: dev.utc,
            gps: dev.gps,
            bt: dev.bt,
            battery: ((dev.battery_pct as u32 + 12) / 25).clamp(1, 4) as u8,
            battery_low: dev.battery_low,
            battery_cutoff: dev.battery_cutoff,
            blink_off: dev.battery_cutoff && (self.now / 500) % 2 == 1,
            alerts_paused: hf && self.set.same_on(),
        }
    }

    /// The hint bar, or the toast over it (HOME-9).
    pub(crate) fn render_hint(&mut self, lcd: &mut impl Panel) {
        if let Some(t) = self.toast {
            if self.dirty(R_HINT, hash(&(1u8, t.head, t.body, t.icon))) {
                w::toast(lcd, t.head.as_bytes(), t.body.as_bytes(), t.icon);
            }
            return;
        }
        let h = self.screen.hints(self);
        if self.dirty(R_HINT, hash(&(0u8, h.as_slice()))) {
            w::hint_bar(lcd, h.as_slice());
        }
    }

    /// List row `i` (region R_ROW + i) at y, drawn when what it shows changed (MENU-12).
    pub(crate) fn list_row(&mut self, lcd: &mut impl Panel, i: usize, y: i32, h: i32, r: &Row) {
        if self.dirty(R_ROW + i, hash(&(r.num, r.name, r.value, r.dot, r.selected, r.grey))) {
            w::list_row(lcd, y, h, r);
        }
    }

    /// Edit grid cell `i` (region R_ROW + i): label over value, focused or greyed (EDIT-3, EDIT-7).
    pub(crate) fn grid_cell(&mut self, lcd: &mut impl Panel, i: usize, x: i32, y: i32, w_: i32, h: i32, label: &[u8], value: &[u8], focused: bool, grey: bool) {
        if self.dirty(R_ROW + i, hash(&(label, value, focused, grey))) {
            w::edit_cell(lcd, x, y, w_, h, label, value, focused, grey);
        }
    }

    /// The help box at the foot of a list: one line of help and the item's path (MENU-15).
    pub(crate) fn help_box(&mut self, lcd: &mut impl Panel, y: i32, h: i32, text: &[u8], path: &[u8]) {
        if self.dirty(R_HELP, hash(&(text, path))) {
            w::help_box(lcd, y, h, text, path);
        }
    }

    /// A message page (prompts, power off): a centred heading in `c`, an optional second line, wrapped text.
    pub(crate) fn message(&mut self, lcd: &mut impl Panel, head: &[u8], line: &[u8], text: &[u8], c: u16) {
        if self.dirty(R_TOP, hash(&(head, line, text))) {
            use ferrite_ui::assets::{UI_12, UI_15B};
            w::field(lcd, 10, 220, 60, 86, 80, Align::Center, head, &UI_15B, c, BG);
            w::field(lcd, 10, 220, 92, 116, 110, Align::Center, line, &UI_15B, TEXT, BG);
            w::wrap(lcd, 16, 208, 124, 290, 140, 15, text, &UI_12, SUB, BG);
        }
    }

    // ---- slot texts (home cards, edit header) ----

    /// "VFO · VHF", "CH 012 · UHF", "SCAN · Airband".
    pub(crate) fn head_text(&self, s: u8, live: &LiveAll, b: &mut Buf<32>) {
        let sl = &self.slots[s as usize];
        let lv = &live.slots[s as usize];
        b.clear();
        if lv.scanning {
            b.push(b"SCAN");
        } else if sl.on_channel() {
            chan_label(sl.ch.map_or(0, |c| c.num), b);
        } else {
            b.push(b"VFO");
        }
        b.push(" \u{b7} ".as_bytes());
        b.push(band_name(self.shown_freq(s, live)).as_bytes());
    }

    /// The slot's frequency, or the scan's current step while it scans.
    pub(crate) fn shown_freq(&self, s: u8, live: &LiveAll) -> u32 {
        let lv = &live.slots[s as usize];
        if lv.scanning { lv.freq } else { self.slots[s as usize].freq() }
    }

    /// The slot's state word and colour on a card: RX, relay, off (not watched), scanning, idle.
    pub(crate) fn state(&self, s: u8, live: &LiveAll) -> (&'static str, u16) {
        let lv = &live.slots[s as usize];
        let watched = s == self.active || self.watch_mask() & 1 << s != 0;
        if lv.relay {
            ("relay", AMBER)
        } else if !watched {
            ("off", DIM)
        } else if lv.squelch_open {
            ("RX", GREEN)
        } else if lv.scanning {
            ("scanning", SUB)
        } else if self.paused_until.is_some() && s != self.active {
            ("paused", SUB)
        } else {
            ("idle", SUB)
        }
    }
}
