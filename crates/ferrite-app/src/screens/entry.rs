//! Direct entry (manual chapters 6, 9 and 10; NAV-4, VFO-11 to VFO-14): a digit on home opens it with that
//! digit. In VFO mode it is a frequency, \* the point (the point comes by itself after three MHz digits);
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Entry {
    pub to: EntryFor,
    /// Digits and the point as typed.
    pub buf: [u8; 9],
    pub n: u8,
}

/// Home VFOs tune 18 MHz and up (VFO-14), to what the chips reach.
const LO: Freq10 = mhz(18);
const HI: Freq10 = mhz(1000) - 1;

impl Entry {
    pub fn new(to: EntryFor, first: Option<u8>) -> Self {
        let mut e = Entry { to, buf: [0; 9], n: 0 };
        if let Some(d) = first {
            e.push(b'0' + d);
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
            if !point && self.n > 0 && (self.n as usize) < self.buf.len() {
                self.buf[self.n as usize] = c;
                self.n += 1;
            }
            return;
        }
        if !point && self.n == 3 {
            self.buf[3] = b'.'; // three MHz digits: the point comes by itself
            self.n = 4;
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

    /// The typed channel number (0-999, typed as 1-1000).
    pub fn number(&self) -> Option<u16> {
        let v = self.typed().iter().fold(0u32, |a, &c| a * 10 + (c - b'0') as u32);
        (v >= 1 && v <= MAX_CHANNELS as u32).then(|| v as u16 - 1)
    }
}

/// The step a frequency snaps to: its band's own step on another band, else the slot's (VFO-12). A typed
/// channel or repeater frequency is kept as typed.
fn snap(app: &App, to: EntryFor, f: Freq10) -> Freq10 {
    if to != EntryFor::Vfo {
        return f;
    }
    let s = &app.slots[app.active as usize];
    let step = match band_of(f) {
        Some(b) if band_of(s.vfo) != Some(b) => STEPS[BANDS[b].step as usize].0,
        _ => STEPS[s.cfg.step as usize % STEPS.len()].0,
    };
    (f + step / 2) / step * step
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
        Ok(()) if crate::slot::band_of(f) == Some(crate::slot::BAND_125M) => Ok("Allowed, 0.25 W"),
        Ok(()) if ferrite_core::txplan::check(f, r, false).is_err() => Ok("Extend TX"),
        Ok(()) => Ok("Allowed"),
        Err(_) if app.set.region().is_none() => Err("Region not set"),
        Err(_) => Err("Not amateur"),
    }
}

fn refuse(f: Freq10) -> Option<&'static str> {
    if f < LO {
        Some("Below 18 MHz: HF mode (MENU 0 3 5)") // VFO-14
    } else if f > HI {
        Some("Above what the radio receives")
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
        b.set(if self.freq_mode() { b"Frequency" } else { b"Channel" });
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        let mut s = self;
        match (e.key, e.press) {
            (Key::Digit(d), Press::Short) => s.push(b'0' + d),
            (Key::Star, Press::Short) => s.push(b'.'),
            (Key::Exit, Press::Short) if s.n == 0 => return app.back(),
            (Key::Exit, Press::Short) => {
                s.n -= 1;
                if s.n == 3 && s.buf[3] == b'.' && s.freq_mode() {
                    s.n = 2; // the automatic point goes with the digit before it
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
            lcd.fill(0, 21, 240, 92, BG);
            w::badge(lcd, 8, 26, a, true, BG);
            let what: &[u8] = match self.to {
                EntryFor::Vfo => b"Frequency, MHz",
                EntryFor::Channel => b"Channel number",
                EntryFor::ChannelFreq => b"Channel frequency, MHz",
                EntryFor::Repeater => b"Repeater output, MHz",
            };
            w::field(lcd, 28, 204, 24, 41, 37, Align::Left, what, &UI_12, SUB, BG);
            let font = if text4_width(self.typed(), &DIG_46) <= 200 { &DIG_46 } else { &DIG_28 };
            let x = 8 + lcd.text4_rows(8, 96, self.typed(), font, TEXT, BG, 48, 100);
            lcd.fill(x + 3, 92, 16, 4, BLUE);
            lcd.fill(0, 112, 240, 1, LINE);
        }
        // The rows under it: what the digits mean.
        let mut rows: [(&[u8], Buf<40>, u16); 4] = [(b"", Buf::new(), TEXT), (b"", Buf::new(), TEXT), (b"", Buf::new(), TEXT), (b"", Buf::new(), TEXT)];
        let mut note: &[u8] = b"";
        if self.freq_mode() {
            if let Some(f) = self.freq() {
                let sf = snap(app, self.to, f);
                rows[0].0 = b"Band";
                rows[0].1.set(band_name(sf).as_bytes());
                rows[1].0 = b"Tunes to";
                let mut fb = [0u8; 16];
                let n = ferrite_core::freq::format(sf, &mut fb);
                rows[1].1.set(&fb[..n]);
                rows[2].0 = b"Step";
                let st = match band_of(sf) {
                    Some(b) if band_of(app.slots[a as usize].vfo) != Some(b) => BANDS[b].step,
                    _ => app.slots[a as usize].cfg.step,
                };
                rows[2].1.set(STEPS[st as usize % STEPS.len()].1.as_bytes());
                rows[3].0 = b"Transmit";
                match refuse(sf) {
                    Some(why) => {
                        rows[3].1.set(b"-");
                        (rows[3].2, note) = (AMBER, why.as_bytes());
                    }
                    None => match tx_text(app, sf, cx.dev.tx_built) {
                        Ok(t) => (rows[3].1, rows[3].2) = (Buf::from(t.as_bytes()), GREEN),
                        Err(t) => (rows[3].1, rows[3].2) = (Buf::from(t.as_bytes()), AMBER),
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
        let mut tip = Buf::<64>::new();
        match self.to {
            EntryFor::Vfo => tip.set(b"After three digits the point comes by itself; * types it (52*525)."),
            EntryFor::Channel => tip.set(b"Type the channel number, then OK."),
            _ => tip.set(b"* is the point. OK keeps it, BACK deletes."),
        }
        if app.dirty(R_HELP, hash(&tip)) {
            para(lcd, 8, 250, 224, 3, 15, tip.as_bytes(), &UI_12, SUB, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        let back = if self.n == 0 { ("BACK", "Back") } else { ("BACK", "Delete") };
        if self.freq_mode() {
            Hints::new(&[("*", "Point"), ("OK", "Go"), back])
        } else {
            Hints::new(&[("0-9", "Number"), ("OK", "Go"), back])
        }
    }
}

/// OK: go there (the VFO, a channel), or hand the frequency back to the editor that asked.
fn commit(e: Entry, app: &mut App, store: &impl ChannelStore) {
    if e.to == EntryFor::Channel {
        let Some(num) = e.number() else { return };
        match store.by_num(num) {
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
    let f = snap(app, e.to, f);
    if let Some(why) = refuse(f) {
        return app.show_toast(why.as_bytes(), b"", false);
    }
    match e.to {
        EntryFor::Vfo => {
            let a = app.active as usize;
            let s = &mut app.slots[a];
            let (old, new) = (band_near(s.vfo), band_near(f));
            s.mem[old] = s.vfo;
            if old != new {
                // A new band: its mode and step, and the offset's direction kept with its amount (NAV-2).
                s.cfg.mode = BANDS[new].mode;
                s.cfg.step = BANDS[new].step;
                if s.cfg.offset != 0 {
                    s.cfg.offset = s.cfg.offset.signum() * BANDS[new].shift;
                }
            }
            (s.vfo, s.mem[new], s.vfo_mode) = (f, f, true);
            app.send_slot(a as u8);
            app.go_home();
        }
        EntryFor::ChannelFreq => {
            if f < mhz(64) {
                return app.show_toast(b"Channels are 64 MHz and up", b"", false); // CHAN-22
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
        EntryFor::Channel => {}
    }
}
