//! Direct entry (manual chapters 6, 9 and 10; NAV-4, VFO-11 to VFO-14): a digit on home opens it with that
//! digit. In VFO mode it is a frequency, \* the point (the point comes by itself after three MHz digits; a
//! second \* after 100-130 takes it away again, for a fourth MHz digit: 1 2 9 \* \* 6 is 1296 MHz, VFO-21), and
//! \* after a digit past the point is OK (146 \* 52 \*, VFO-24);
//! the screen shows the digits, the band, the frequency it snaps to on the step, and whether transmit is
//! allowed there or why not. In channel mode it is a channel number, shown with its name or "empty". OK goes
//! there, BACK deletes a digit (with none left, it leaves). The same screen types a frequency for the channel
//! editor and for Add repeater.

use super::forms::para;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::{ChannelStore, MAX_CHANNELS};
use crate::keys::{KeyEvent, Press};
use crate::slot::{band_name, band_of, band_near, chan_label, BANDS, STEPS};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// What the typed value is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryFor {
    /// The active slot's VFO (A1).
    Vfo,
    /// A channel number, in channel mode (A2).
    Channel,
    /// The channel being edited: its frequency (A4).
    ChannelFreq,
    /// Add repeater's frequency (A10).
    Repeater,
    /// Split's TX frequency on the active slot (MENU 5's 4th choice): kept as typed; BACK keeps the old shift.
    SplitTx,
    /// The band scope's centre (SCOPE-3).
    ScopeCentre,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Entry {
    pub to: EntryFor,
    /// Digits and the point as typed.
    pub buf: [u8; 10],
    pub n: u8,
    /// Four MHz digits (1000-1300 MHz): the automatic point comes after the fourth.
    pub wide: bool,
}

/// Home VFOs tune 18 MHz and up (VFO-14), to what the chips reach.
const LO: Freq10 = mhz(18);
const HI: Freq10 = mhz(1300) - 1;

impl Entry {
    pub fn new(to: EntryFor, first: Option<u8>) -> Self {
        let mut e = Entry { to, buf: [0; 10], n: 0, wide: false };
        if let Some(d) = first {
            e.push(b'0' + d);
        }
        e
    }

    /// Prefilled with a frequency ("146.52"; trailing zeros after the point dropped), to edit or keep with OK.
    pub fn with_freq(to: EntryFor, f: Freq10) -> Self {
        let mut e = Entry { to, buf: [0; 10], n: 0, wide: f >= mhz(1000) };
        let (int, mut frac, mut k) = (f / 100_000, f % 100_000, 5);
        while k > 1 && frac % 10 == 0 {
            frac /= 10;
            k -= 1;
        }
        let mut idig = [0u8; 4];
        let mut m = 0;
        let mut v = int;
        loop {
            idig[m] = b'0' + (v % 10) as u8;
            m += 1;
            v /= 10;
            if v == 0 || m == idig.len() {
                break;
            }
        }
        for i in (0..m).rev() {
            e.push(idig[i]);
        }
        e.push(b'.'); // after three digits the point is already there: a second one is ignored
        let mut digits = [0u8; 5];
        for i in (0..k).rev() {
            digits[i] = b'0' + (frac % 10) as u8;
            frac /= 10;
        }
        for &c in &digits[..k] {
            e.push(c);
        }
        e
    }

    fn typed(&self) -> &[u8] {
        &self.buf[..self.n as usize]
    }

    fn freq_mode(&self) -> bool {
        self.to != EntryFor::Channel
    }

    fn push(&mut self, c: u8) {
        let point = self.typed().contains(&b'.');
        if !self.freq_mode() {
            if c != b'.' && self.n < 4 {
                self.buf[self.n as usize] = c;
                self.n += 1;
            }
            return;
        }
        if c == b'.' {
            // VFO-21: a second * right after the point on 100-130 takes it away, for 1000-1300 MHz
            let three = self.buf[..3].iter().fold(0u32, |a, &c| a * 10 + (c.wrapping_sub(b'0')) as u32);
            if point && !self.wide && self.n == 4 && self.buf[3] == b'.' && (100..=130).contains(&three) {
                (self.n, self.wide) = (3, true);
                return;
            }
            if !point && self.n > 0 && (self.n as usize) < self.buf.len() {
                self.buf[self.n as usize] = c;
                self.n += 1;
            }
            return;
        }
        let mhz_digits = if self.wide { 4 } else { 3 };
        if !point && self.n == mhz_digits {
            self.buf[self.n as usize] = b'.'; // three (or four) MHz digits: the point comes by itself
            self.n += 1;
        }
        if (self.n as usize) < self.buf.len() {
            self.buf[self.n as usize] = c;
            self.n += 1;
        }
    }

    /// The typed frequency, 10 Hz (digits after the point beyond five are not accepted).
    pub fn freq(&self) -> Option<Freq10> {
        let t = self.typed();
        if t.is_empty() {
            return None;
        }
        let p = t.iter().position(|&c| c == b'.').unwrap_or(t.len());
        let int = t[..p].iter().fold(0u32, |a, &c| a * 10 + (c - b'0') as u32);
        let mut frac = 0u32;
        for i in 0..5 {
            frac = frac * 10 + t.get(p + 1 + i).map_or(0, |&c| (c - b'0') as u32);
        }
        Some(mhz(int) + frac)
    }

    /// A whole frequency is typed, so it goes in without OK (VFO-17): from three decimals (kHz) on, once it is on
    /// the step's grid, and always at five. Returns it, exactly as typed (VFO-12). A leading 0 counts as a MHz
    /// digit, so 0 7 8 0 0 0 is 78.000 MHz.
    fn complete(&self, app: &App) -> Option<Freq10> {
        let t = self.typed();
        let p = t.iter().position(|&c| c == b'.').filter(|_| self.freq_mode())?;
        let decimals = t.len() - p - 1;
        let f = self.freq()?;
        (decimals >= 5 || decimals >= 3 && f % step_at(app, f) == 0).then_some(f)
    }

    /// The entry holding exactly f (5 decimals), so OK's path commits it.
    fn set_freq(&mut self, f: Freq10) {
        self.wide = f >= mhz(1000);
        let mut b = Buf::<12>::new();
        let _ = uwrite!(b, "{}.{}", f / 100_000, ferrite_ui::text::Zero(f % 100_000, 5));
        let n = b.as_bytes().len().min(self.buf.len());
        self.buf[..n].copy_from_slice(&b.as_bytes()[..n]);
        self.n = n as u8;
    }

    /// VFO-24: \* now does what OK does: a frequency with the point and at least one digit after it.
    fn accepts_star(&self) -> bool {
        let t = self.typed();
        self.freq_mode() && t.iter().position(|&c| c == b'.').is_some_and(|p| p + 1 < t.len())
    }

    /// The typed channel number (0-999, typed as 1-1000).
    pub fn number(&self) -> Option<u16> {
        let v = self.typed().iter().fold(0u32, |a, &c| a * 10 + (c - b'0') as u32);
        (v >= 1 && v <= MAX_CHANNELS as u32).then(|| v as u16 - 1)
    }
}

/// The step at f: its band's own on another band, else the active slot's (VFO-12).
fn step_at(app: &App, f: Freq10) -> Freq10 {
    let s = &app.slots[app.active as usize];
    match band_of(f) {
        Some(b) if band_of(s.vfo) != Some(b) => STEPS[crate::slot::band_step(b, app.set.region()) as usize].0,
        _ => STEPS[s.cfg.step as usize % STEPS.len()].0,
    }
}

/// Why a frequency can't be tuned here, if it can't.
/// Whether transmit is allowed at f, and the reason either way (VFO-12; the rules are chapter 15's,
/// [`ferrite_core::txplan`]). Display only: the Radio task decides when it keys.
fn tx_text(app: &App, f: Freq10, tx_built: bool) -> Result<&'static str, &'static str> {
    let r = app.set.tx_region();
    if !tx_built {
        return Err("RX-only build");
    }
    match ferrite_core::txplan::check(f, r, app.set.extend_tx) {
        Ok(()) if ferrite_core::txplan::locked_125m(f) => Err("Locked until measured"), // TX-10
        Ok(()) if crate::slot::band_of(f) == Some(crate::slot::BAND_125M) => Ok("Allowed, 0.25 W"),
        Ok(()) if ferrite_core::txplan::check(f, r, false).is_err() => Ok("Extend TX"),
        Ok(()) => Ok("Allowed"),
        Err(_) if app.set.region().is_none() => Err("Region not set"),
        Err(_) => Err("Not amateur"),
    }
}

/// Why a frequency can't be tuned, as "head: body" (the toast splits it): below 18 MHz is HF mode's (VFO-14);
/// anything else no chip receives names the ranges that are (VFO-21). Each fits the entry's note on one line.
pub(crate) fn refuse(f: Freq10) -> Option<&'static str> {
    if f < LO {
        Some("< 18 MHz: HF mode, MENU 0 3 4") // VFO-14
    } else if f > HI || !ferrite_core::freq::receivable(f) {
        Some("Can't tune: 18-630, 756-1300 MHz")
    } else {
        None
    }
}

impl View for Entry {
    fn name(self) -> &'static str {
        match self.to {
            EntryFor::Channel => "Channel entry",
            _ => "Frequency entry",
        }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(if self.freq_mode() { b"Freq" } else { b"Channel" }); // 8 or fewer (SBAR-5)
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        let mut s = self;
        match (e.key, e.press) {
            (Key::Digit(d), Press::Short) => {
                s.push(b'0' + d);
                if let Some(f) = s.complete(app) {
                    app.screen = Screen::Entry(s); // a refusal leaves the digits on show
                    let mut whole = s;
                    whole.set_freq(f);
                    return commit(whole, app, store);
                }
            }
            // VFO-24: \* after the point and a digit is OK (146 \* 52 \*), one-handed; before that it is the point
            // (and \* \* on 100-130 the fourth MHz digit, VFO-11). It never types a second point.
            (Key::Star, Press::Short) if s.accepts_star() => return commit(s, app, store),
            (Key::Star, Press::Short) => s.push(b'.'),
            (Key::Exit, Press::Short) if s.n == 0 => return app.back(),
            (Key::Exit, Press::Short) => {
                s.n -= 1;
                let auto = if s.wide { 4 } else { 3 };
                if s.n == auto && s.buf[auto as usize] == b'.' && s.freq_mode() {
                    s.n = auto - 1; // the automatic point goes with the digit before it
                }
                if s.n < 3 {
                    s.wide = false;
                }
            }
            (Key::Menu, Press::Short) => return commit(s, app, store),
            _ => return,
        }
        app.screen = Screen::Entry(s);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let a = app.active;
        // What is typed, big, with a block cursor.
        if app.dirty(R_TOP, hash(&(a, self.to, self.typed()))) {
            ferrite_ui::canvas::banded(lcd, 0, 21, 240, 92, BG, |lcd| {
                w::badge(lcd, 8, 26, a, true, BG);
                let what: &[u8] = match self.to {
                    EntryFor::Vfo => b"Frequency, MHz",
                    EntryFor::Channel => b"Channel number",
                    EntryFor::ChannelFreq => b"Channel frequency, MHz",
                    EntryFor::Repeater => b"Repeater output, MHz",
                    EntryFor::SplitTx => b"Split: TX frequency, MHz",
                    EntryFor::ScopeCentre => b"Scope centre, MHz",
                };
                w::field(lcd, 28, 204, 24, 41, 37, Align::Left, what, &UI_12, SUB, BG);
                // As home shows a frequency: MHz and three decimals big, the 10 Hz digits small, so the size never
                // changes while typing (a digit lands in the size it will have on the card).
                let t = self.typed();
                let cut = match t.iter().position(|&c| c == b'.') {
                    Some(p) if self.freq_mode() => t.len().min(p + 4),
                    _ => t.len(),
                };
                let mut x = 8 + lcd.text4_rows(8, 96, &t[..cut], &DIG_46, TEXT, BG, 48, 100);
                if cut < t.len() {
                    x += lcd.text4_rows(x, 96, &t[cut..], &DIG_28, TEXT, BG, 48, 100);
                }
                lcd.fill(x + 3, 92, 16, 4, BLUE);
                lcd.fill(0, 112, 240, 1, LINE);
            });
        }
        // The rows under it: what the digits mean.
        let mut rows: [(&[u8], Buf<40>, u16); 4] = [(b"", Buf::new(), TEXT), (b"", Buf::new(), TEXT), (b"", Buf::new(), TEXT), (b"", Buf::new(), TEXT)];
        let mut note: &[u8] = b"";
        if self.freq_mode() {
            if let Some(f) = self.freq() {
                // VFO-12: tuned exactly as typed; the step row says whether it sits on the step's grid.
                rows[0].0 = b"Band";
                rows[0].1.set(band_name(f).as_bytes());
                rows[1].0 = b"Step";
                let step = step_at(app, f);
                let name = STEPS.iter().find(|s| s.0 == step).map_or("", |s| s.1);
                rows[1].1.set(name.as_bytes());
                rows[1].1.push(if f % step == 0 { b", on it" } else { b", off it" });
                rows[2].0 = b"Transmit";
                match refuse(f) {
                    Some(why) => {
                        rows[2].1.set(b"-");
                        (rows[2].2, note) = (AMBER, why.as_bytes());
                    }
                    None => match tx_text(app, f, cx.dev.tx_built) {
                        Ok(t) => (rows[2].1, rows[2].2) = (Buf::from(t.as_bytes()), GREEN),
                        Err(t) => (rows[2].1, rows[2].2) = (Buf::from(t.as_bytes()), AMBER),
                    },
                }
            }
        } else if let Some(num) = self.number() {
            rows[0].0 = b"Channel";
            let mut l = Buf::<32>::new();
            chan_label(num, &mut l);
            rows[0].1.set(l.as_bytes());
            rows[1].0 = b"Name";
            match cx.store.by_num(num) {
                Some((c, _)) => {
                    rows[1].1.set(if c.name().is_empty() { b"(no name)" } else { c.name() });
                    rows[2].0 = b"Zone";
                    rows[2].1.set(cx.store.zone_label(c.zone, b"None", b"?").as_bytes());
                    rows[3].0 = b"Frequency";
                    rows[3].1.set(w::freq3(c.rx).as_bytes());
                }
                None => (rows[1].1, rows[1].2) = (Buf::from(b"empty"), AMBER),
            }
        } else if self.n > 0 {
            note = b"Channels are 1 to 1000";
        }
        for (i, (label, v, c)) in rows.iter().enumerate() {
            app.kv(lcd, i, 116 + 22 * i as i32, 22, &w::KV_STATUS, label, v.as_bytes(), *c, false);
        }
        if app.dirty(R_NOTE, hash(&note)) {
            para(lcd, 8, 210, 224, 2, 17, note, &UI_12, AMBER, BG);
        }
        let tip: &[u8] = match self.to {
            EntryFor::Vfo => b"After three digits the point comes by itself; * types it, and * again enters (52*525*).",
            EntryFor::Channel => b"Type the channel number, then OK.",
            EntryFor::SplitTx => b"Transmit here whatever RX is on. OK or * keeps it; BACK keeps the old shift.",
            _ => b"* is the point, then * or OK keeps it. BACK deletes.",
        };
        if app.dirty(R_HELP, hash(&tip)) {
            para(lcd, 8, 250, 224, 3, 15, tip, &UI_12, SUB, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        let back = if self.n == 0 { ("BACK", "Back") } else { ("BACK", "Delete") };
        if self.accepts_star() {
            Hints::new(&[("*/OK", "Go"), back]) // VFO-24
        } else if self.freq_mode() {
            Hints::new(&[("*", "Point"), ("OK", "Go"), back])
        } else {
            Hints::new(&[("0-9", "Number"), ("OK", "Go"), back])
        }
    }
}

/// OK: go there (the VFO, a channel), or hand the frequency back to the editor that asked.
/// A frequency typed for a slot's VFO (VFO-11; the terminal's and the link's `slot`, TERM-35): the slot goes to
/// its VFO there; a new band brings its mode and step, and the offset's direction kept with its amount (NAV-2).
/// `f` has passed [`refuse`].
pub(crate) fn tune_vfo(app: &mut App, slot: u8, f: Freq10) {
    let region = app.set.region();
    let s = &mut app.slots[slot as usize];
    let (old, new) = (band_near(s.vfo), band_near(f));
    s.mem[old] = s.vfo;
    if old != new {
        s.cfg.mode = crate::slot::band_mode(new, region);
        s.cfg.narrow = s.cfg.mode == 1;
        s.cfg.step = crate::slot::band_step(new, region);
        if s.cfg.offset != 0 && !s.cfg.split {
            s.cfg.offset = s.cfg.offset.signum() * BANDS[new].shift;
        }
    }
    (s.vfo, s.mem[new], s.vfo_mode) = (f, f, true);
    app.send_slot(slot);
}

fn commit(e: Entry, app: &mut App, store: &impl ChannelStore) {
    if e.to == EntryFor::Channel {
        let Some(num) = e.number() else { return };
        match store.by_num(num) {
            Some((c, _)) if c.is_hf() => super::hf::recall(app, &c), // CHAN-26
            Some((c, pos)) => {
                let a = app.active as usize;
                let s = &mut app.slots[a];
                (s.zone, s.pos, s.ch, s.vfo_mode) = (c.zone, pos, Some(c), false);
                app.send_slot(a as u8);
                app.go_home();
            }
            None => {
                let mut l = Buf::<32>::new();
                chan_label(num, &mut l);
                app.show_toast(l.as_bytes(), b"is empty", false);
            }
        }
        return;
    }
    let Some(f) = e.freq() else { return };
    if let Some(why) = refuse(f) {
        // VFO-21: the entry stays open with its digits, so they can be fixed
        let (head, body) = why.split_once(": ").unwrap_or((why, ""));
        return app.show_toast(head.as_bytes(), body.as_bytes(), false);
    }
    match e.to {
        EntryFor::Vfo => {
            tune_vfo(app, app.active, f);
            app.go_home();
        }
        EntryFor::ChannelFreq => {
            if f < crate::chan::HF_TOP {
                return app.show_toast(b"Below 64 MHz", b"save it in HF mode", false); // CHAN-26
            }
            super::chedit::edit(app, |c| c.rx = f);
            app.back();
        }
        EntryFor::Repeater => {
            app.back();
            if let Screen::Save(mut s) = app.screen {
                s.set_freq(f);
                app.screen = Screen::Save(s);
            }
        }
        EntryFor::SplitTx => {
            // TX-37: refused when it needs the other relay state; entry stays open to type another
            if app.set_split(f) {
                app.back();
            }
        }
        EntryFor::ScopeCentre => {
            app.back();
            if let Screen::Scope(s) = app.screen {
                app.radio.centre = f;
                app.screen = super::scope::recentre(app, s, f);
            }
        }
        EntryFor::Channel => {}
    }
}
