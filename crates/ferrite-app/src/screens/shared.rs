//! What the screens share for drawing: region dirty-tracking, the status bar, the hint bar and toast, list
//! rows, the edit grid's cells, the help box, and the slot texts several screens show. Each region of the
//! screen is hashed from what it shows; a screen draws a region only when its hash changed (HOME-7), so a meter
//! update never touches the frequency and a still screen costs nothing. Layouts follow the look review's
//! mock-ups (`docs/img/screens`).

use crate::chan::ChannelStore;
use crate::slot::band_name;
use crate::App;
use core::hash::{Hash, Hasher};
use ferrite_core::msg::{Device, LiveAll};
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row, StatusBar};

// Regions. Screens reuse the numbers; a screen change clears the content and forgets every hash.
/// A region's hash after its box was just filled with its background: [`App::dirty_shape`] draws it without a
/// blank (bit 1 clear, so no hash `dirty_shape` makes equals it).
pub(crate) const FRESH: u32 = 0xFFFF_FFF0;
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
pub(crate) const R_SCROLL: usize = 20; // the scroll cue (DISP-24)
pub(crate) const REGIONS: usize = 21;

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

/// HOME-13: how long the knob rests before home draws its heavier parts again.
pub const KNOB_REST_MS: u32 = 400;

impl App {
    /// HOME-13: the knob stepped within [`KNOB_REST_MS`]. Signed, since a step can carry a time just ahead of
    /// the last tick's.
    pub(crate) fn knob_busy(&self) -> bool {
        self.knob_at.is_some_and(|t| (self.now.wrapping_sub(t) as i32) < KNOB_REST_MS as i32)
    }

    /// HOME-13: region r is on the screen and waits while the knob turns; it redraws once the knob rests (its
    /// hash still differs then). A region not yet drawn is never held.
    pub(crate) fn held(&self, r: usize) -> bool {
        self.drawn.hash[r] != 0 && self.knob_busy()
    }

    /// True (and remembered) when region r's content differs from what is on screen.
    pub(crate) fn dirty(&mut self, r: usize, h: u32) -> bool {
        let d = self.drawn.hash[r] != h;
        self.drawn.hash[r] = h;
        d
    }

    /// As [`App::dirty`] for a box whose content can take two layouts (`shape`: e.g. a bare frequency or a name
    /// over a small one). Some(true): blank the box first (first draw, or the layout changed); Some(false): draw
    /// straight over it, since each part repaints its own rows and the rest is already background. Blanking on
    /// every change showed as tearing on the radio while the knob turned (Oct 12, 2026). The layout rides in the
    /// hash's low bit; bit 1 keeps it non-zero (0 = not drawn).
    pub(crate) fn dirty_shape(&mut self, r: usize, h: u32, shape: bool) -> Option<bool> {
        let prev = self.drawn.hash[r];
        let h = (h & !3) | 2 | shape as u32;
        if !self.dirty(r, h) {
            return None;
        }
        Some(prev != FRESH && (prev == 0 || prev & 1 != shape as u32))
    }

    /// DISP-24: the scroll cue beside rows `R_ROW + from ..+ count`, drawn from `y` for `h` when there are rows
    /// above or below the page (`top` of `n`, `shown` on it); redrawn whenever those rows redraw (a list's blank
    /// rows paint full width, over the cue) or the page moves, and cleared once a list no longer scrolls. A page
    /// whose every row draws its own part of the cue (a [`super::table::Table`]) passes no rows: a row's change
    /// then redraws that row only, not the whole cue as well (Oct 13, 2026).
    pub(crate) fn scroll_cue(&mut self, lcd: &mut impl Panel, rows: (usize, usize), y: i32, h: i32, more: bool, top: u32, shown: u32, n: u32) {
        let under = hash(&&self.drawn.hash[R_ROW + rows.0..(R_ROW + rows.0 + rows.1).min(R_PAGE)]);
        let prev = self.drawn.hash[R_SCROLL];
        // bit 0: the cue is on the screen
        if !self.dirty(R_SCROLL, (hash(&(under, y, h, more, top, shown, n)) & !1) | more as u32) {
            return;
        }
        if more {
            self.drawn.cue = w::Cue::new(y, h, top, shown, n);
            w::scroll_cue(lcd, self.drawn.cue);
        } else if prev & 1 != 0 {
            lcd.fill(w::CUE_X, y, 3, h, BG);
        }
    }

    /// The scroll cue on the screen, for the rows beside it to draw with (none once the region is forgotten).
    pub(crate) fn cue(&self) -> w::Cue {
        if self.drawn.hash[R_SCROLL] & 1 != 0 { self.drawn.cue } else { w::Cue::default() }
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
            alert: self.alert & crate::ALERT_UNREAD != 0,
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

    /// The hint bar, or the toast over it (HOME-9); a shortcut's popup leaves the hint bar alone.
    pub(crate) fn render_hint(&mut self, lcd: &mut impl Panel) {
        if let Some(t) = self.toast.filter(|t| !t.center) {
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
            w::list_row(lcd, y, h, r, self.cue());
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
        if self.aprs_slot(s) {
            b.push(b"APRS"); // APRS-17
        } else if lv.scanning {
            b.push(b"SCAN");
        } else if sl.on_channel() {
            crate::slot::chan_num(sl.ch.map_or(0, |c| c.num), b);
        } else {
            b.push(b"VFO");
        }
        b.push(" \u{b7} ".as_bytes());
        // BCR-21: a slot in the broadcast band says so where the band goes
        b.push(if self.in_bcast_band(s) && !lv.scanning { b"Broadcast".as_slice() } else { band_name(self.shown_freq(s, live)).as_bytes() });
    }

    /// A channel slot's name, the cards' main line in channel mode (HOME-4); empty for a VFO, a scan or a
    /// channel without a name, which show their frequency large instead.
    pub(crate) fn chan_name(&self, s: u8, live: &LiveAll) -> Buf<16> {
        let sl = &self.slots[s as usize];
        let mut n = Buf::<16>::new();
        if !live.slots[s as usize].scanning && !self.aprs_slot(s) {
            if let Some(c) = sl.ch.filter(|_| sl.on_channel()) {
                n.set(c.name());
            }
        }
        n
    }

    /// The slot's frequency, or the scan's current step while it scans.
    pub(crate) fn shown_freq(&self, s: u8, live: &LiveAll) -> u32 {
        let lv = &live.slots[s as usize];
        if lv.scanning { lv.freq } else { self.tuned_freq(s) }
    }

    /// The slot's state word and colour on a card: Muted or RX muted (quick mute), RX, relay, off (not watched),
    /// heard (a tone hint), scanning, idle; on a broadcast slot FM (playing), muted (a slot heard has the speaker) or off, and "FM on B" on
    /// another slot in the band (BCR-22, BCR-27); "Watch off: relay" while broadcast plays (BCR-15).
    pub(crate) fn state(&self, s: u8, live: &LiveAll) -> (&'static str, u16) {
        let lv = &live.slots[s as usize];
        // HOME-15: quick mute takes the state word, "RX muted" while it receives (the broadcast slot too).
        if self.mute & 1 << s != 0 && !self.relay_off(s) && !lv.relay {
            return if lv.squelch_open { ("RX muted", AMBER) } else { ("Muted", AMBER) };
        }
        let watched = s == self.active || self.watch_mask() & 1 << s != 0;
        if self.in_bcast_band(s) && !lv.scanning {
            return match self.bcast_slot() {
                Some(b) if b != s => (["FM on A", "FM on B", "FM on C"][b as usize % 3], DIM),
                Some(_) if !self.radio.fm_on => ("off", DIM),
                Some(_) if self.bcast_heard(live) => ("FM", GREEN),
                Some(_) => ("muted", AMBER),
                None => ("off", DIM),
            };
        }
        if self.relay_off(s) {
            ("Watch off: relay", AMBER)
        } else if lv.relay {
            ("relay", AMBER)
        } else if self.monitor & 1 << s != 0 {
            ("MON", AMBER) // a latched monitor: squelch held open (decided Oct 6, 2026)
        } else if !watched {
            ("off", DIM)
        } else if lv.heard_tone != 0 {
            ("heard", AMBER) // TSRCH-13: the card shows "Heard 88.5" ([`heard_text`])
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

/// The tone hint on a card (TSRCH-13): "Heard 88.5", "Heard D023I". At most 11 characters (77 px at UI_12), so a
/// card's state field widens to [`HEARD_W`] for it.
pub(crate) fn heard_text(t: u16, b: &mut Buf<16>) {
    let mut tone = Buf::<16>::new();
    crate::slot::tone_text(t, &mut tone);
    let tb = tone.as_bytes();
    b.set(b"Heard ");
    b.push(tb.strip_prefix(b"T ").unwrap_or(tb));
}

/// The state field's width for a tone hint (TSRCH-13): 11 characters.
pub(crate) const HEARD_W: i32 = 80;
