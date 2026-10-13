//! Save the VFO as a channel (hold V/M on a VFO, MENU 8 on a VFO; manual chapters 9 and 10, VFO-9, CHAN-22)
//! and Add repeater (MENU 0 2 2; chapter 10, MENU-10): one form. Save shows the number (the first free one
//! after the slot's last channel), the zone (the one being browsed) and the name; Add repeater asks the
//! frequency, shift and tone first and ends on the channel editor. The knob or ◀▶ change a row, OK on the
//! name or frequency types it, OK elsewhere saves. Hold V/M in HF mode saves its frequency and mode as an HF
//! channel (CHAN-26), with the same rows.

use super::chedit::ChEdit;
use super::entry::{Entry, EntryFor};
use super::forms::para;
use super::nav::Dest;
use super::shared::*;
use super::text::TextFor;
use super::{Cx, Hints, View};
use crate::chan::{ChanWrite, Channel, ChannelStore, MAX_CHANNELS, NO_ZONE};
use crate::keys::{KeyEvent, Press};
use crate::slot::{band_near, chan_label, offset_text, tone_at, tone_index, tone_text, SlotCfg, BANDS, DEFAULT_CFG, SQ_GLOBAL, STEPS};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets::{self as w, Row};

/// What the form saves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// The active slot's VFO (A9).
    Vfo,
    /// Add repeater (A10).
    Repeater,
    /// HF mode's frequency and mode (CHAN-26).
    Hf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Save {
    pub kind: Kind,
    pub row: u8,
    /// The new channel's number ([`UNSET`] until the store has been asked).
    pub num: u16,
    pub zone: u8,
    /// Add repeater: output frequency, shift and TX tone.
    pub freq: Freq10,
    pub offset: i32,
    pub tone: u16,
}

pub const UNSET: u16 = 0xFFFF;

/// One row of the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum F {
    Freq,
    Shift,
    Tone,
    Name,
    Number,
    Zone,
    Save,
}

const SAVE_ROWS: [F; 4] = [F::Number, F::Zone, F::Name, F::Save];
const REPEATER_ROWS: [F; 7] = [F::Freq, F::Shift, F::Tone, F::Name, F::Number, F::Zone, F::Save];

impl F {
    fn label(self) -> &'static str {
        match self {
            F::Freq => "Frequency",
            F::Shift => "Shift",
            F::Tone => "Tone",
            F::Name => "Name",
            F::Number => "Number",
            F::Zone => "Zone",
            F::Save => "Save",
        }
    }
}

/// The usual repeater shift for a frequency: its band's amount; down on 2 m below 147 MHz, else up.
fn auto_shift(f: Freq10) -> i32 {
    let s = BANDS[band_near(f)].shift;
    if f >= mhz(144) && f < mhz(147) { -s } else { s }
}

impl Save {
    pub fn new(repeater: bool) -> Self {
        Save { kind: if repeater { Kind::Repeater } else { Kind::Vfo }, row: 0, num: UNSET, zone: NO_ZONE, freq: 0, offset: 0, tone: 0 }
    }

    /// Hold V/M in HF mode (CHAN-26).
    pub fn hf() -> Self {
        Save { kind: Kind::Hf, ..Save::new(false) }
    }

    fn repeater(self) -> bool {
        self.kind == Kind::Repeater
    }

    fn rows(self) -> &'static [F] {
        if self.repeater() { &REPEATER_ROWS } else { &SAVE_ROWS }
    }

    fn at(self) -> F {
        self.rows()[self.row as usize % self.rows().len()]
    }

    /// Add repeater's frequency, typed (A1): the shift follows it.
    pub fn set_freq(&mut self, f: Freq10) {
        self.freq = f;
        self.offset = auto_shift(f);
    }

    fn freq_of(self, app: &App) -> Freq10 {
        match self.kind {
            Kind::Repeater => self.freq,
            Kind::Hf => app.set.hf.freq,
            Kind::Vfo => app.slots[app.active as usize].vfo,
        }
    }

    /// First use: the number, zone and frequency from the slot and the store, and an empty name.
    fn ready(self, app: &mut App, store: &impl ChannelStore) -> Self {
        if self.num != UNSET {
            return self;
        }
        let mut s = self;
        let sl = &app.slots[app.active as usize];
        let from = sl.ch.map_or(0, |c| (c.num + 1) % MAX_CHANNELS);
        s.num = store.free_num(from).unwrap_or(UNSET - 1);
        s.zone = if sl.zone < store.zones() { sl.zone } else { NO_ZONE }; // the zone being browsed
        s.set_freq(sl.vfo);
        app.text.clear();
        app.screen = Screen::Save(s);
        s
    }

    /// Why it can't be saved, if it can't.
    fn refuse(self, app: &App) -> Option<&'static str> {
        if self.kind == Kind::Repeater && self.freq_of(app) < crate::chan::HF_TOP {
            Some("Below 64 MHz: save it from HF mode (hold V/M there).") // CHAN-26
        } else if self.num >= MAX_CHANNELS {
            Some("No free channel number: all 1000 are used.")
        } else {
            None
        }
    }
}

impl View for Save {
    fn name(self) -> &'static str {
        if self.repeater() { "Add repeater" } else { "Save as channel" }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(if self.repeater() { b"Repeater" } else { b"Save" });
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let mut s = self.ready(app, store);
        let n = s.rows().len() as u8;
        match (e.key, e.press) {
            (Key::Up, Short | Repeat) => s.row = (s.row + n - 1) % n,
            (Key::Down, Short | Repeat) => s.row = (s.row + 1) % n,
            (Key::Hash, Short) => match s.at() {
                F::Shift => s.offset = auto_shift(s.freq),
                F::Tone => s.tone = 0,
                F::Zone => s.zone = NO_ZONE,
                _ => {}
            },
            (Key::Menu, Short) => match s.at() {
                F::Name => {
                    return super::text::open(app, TextFor::SaveName, store);
                }
                F::Freq => {
                    app.open(Dest::Entry(0));
                    app.screen = Screen::Entry(Entry::new(EntryFor::Repeater, None));
                    return;
                }
                _ => return save(s, app, store),
            },
            (Key::Exit, Short) => return app.back(),
            _ => return,
        }
        app.screen = Screen::Save(s);
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let s = self.ready(app, store);
        app.screen = Screen::Save(change(s, app, store, d));
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let s = self.ready(app, cx.store);
        let f = s.freq_of(app);
        if app.dirty(R_TOP, hash(&(s.kind, f))) {
            ferrite_ui::canvas::banded(lcd, 0, 21, 240, 72, BG, |lcd| {
                let head: &[u8] = match s.kind {
                    Kind::Repeater => b"Add a repeater",
                    Kind::Hf => b"Save this HF frequency",
                    Kind::Vfo => b"Save this VFO as a channel",
                };
                w::field(lcd, 8, 224, 24, 41, 37, Align::Left, head, &UI_12B, TEXT, BG);
                w::freq(lcd, 8, 232, 84, f, &DIG_46, &DIG_28, TEXT, SUB, BG);
                lcd.fill(0, 92, 240, 1, LINE);
            });
        }
        for (i, &r) in s.rows().iter().enumerate() {
            let mut v = Buf::<32>::new();
            match r {
                F::Freq => v.set(w::freq3(s.freq).as_bytes()),
                F::Shift => {
                    let mut b = Buf::<16>::new();
                    offset_text(s.offset, true, &mut b);
                    v.set(if b.is_empty() { b"Simplex" } else { b.as_bytes() });
                }
                F::Tone => {
                    let mut b = Buf::<16>::new();
                    tone_text(s.tone, &mut b);
                    v.set(if b.is_empty() { b"Off" } else { b.as_bytes() });
                }
                F::Name => v.set(if app.text.is_empty() { b"(none)" } else { app.text.as_bytes() }),
                F::Number if s.num < MAX_CHANNELS => chan_label(s.num, &mut v),
                F::Number => v.set(b"none free"),
                F::Zone => v.set(cx.store.zone_label(s.zone, b"None", b"?").as_bytes()),
                F::Save => v.set(b">"),
            }
            let row = Row { num: None, name: r.label().as_bytes(), value: v.as_bytes(), dot: false, selected: i as u8 == s.row, grey: false };
            app.list_row(lcd, i, 96 + 24 * i as i32, 24, &row);
        }
        let why = s.refuse(app);
        let tip: &[u8] = match s.at() {
            F::Name => b"OK: type the name. It can stay empty.",
            F::Freq => b"OK: type it. The shift follows the band.",
            F::Number => b"Knob: the next free number.",
            F::Zone => b"Knob: another zone, or none.",
            F::Shift | F::Tone => b"Knob: change it. #: back to the usual one.",
            F::Save => b"OK saves it.",
        };
        if app.dirty(R_NOTE, hash(&(why, tip))) {
            let y = 100 + 24 * s.rows().len() as i32;
            match why {
                Some(t) => para(lcd, 8, y, 224, (296 - y) / 16, 16, t.as_bytes(), &UI_12, AMBER, BG),
                None => para(lcd, 8, y, 224, (296 - y) / 16, 16, tip, &UI_12, SUB, BG),
            };
        }
    }

    fn hints(self, _: &App) -> Hints {
        match self.at() {
            F::Name | F::Freq => Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Type"), ("BACK", "Cancel")]),
            F::Save => Hints::new(&[(ARROWS_UD, "Move"), ("OK", "Save"), ("BACK", "Cancel")]),
            _ => Hints::new(&[("Knob", "Change"), ("OK", "Save"), ("BACK", "Cancel")]),
        }
    }
}

/// The knob or ◀▶ on a row.
fn change(mut s: Save, app: &App, store: &impl ChannelStore, d: i32) -> Save {
    match s.at() {
        F::Freq => {
            let step = STEPS[crate::slot::band_step(band_near(s.freq), app.set.region()) as usize].0 as i32;
            s.set_freq((s.freq as i32 + d * step).clamp(mhz(64) as i32, mhz(1300) as i32 - 1) as Freq10);
        }
        F::Shift => {
            let a = BANDS[band_near(s.freq)].shift.max(60_000);
            let i = (s.offset.signum() + 3) % 3; // simplex, +, -
            s.offset = [0, a, -a][(i + d).rem_euclid(3) as usize];
        }
        F::Tone => {
            // Off and the CTCSS tones: what repeaters use.
            s.tone = tone_at((tone_index(s.tone) + d).rem_euclid(51));
        }
        F::Number => {
            let from = if s.num < MAX_CHANNELS { s.num } else { 0 };
            if let Some(n) = (1..MAX_CHANNELS as i32)
                .map(|k| (from as i32 + d.signum() * k).rem_euclid(MAX_CHANNELS as i32) as u16)
                .find(|&n| store.by_num(n).is_none())
            {
                s.num = n;
            }
        }
        F::Zone => {
            let n = store.zones() as i32 + 1;
            let i = if s.zone == NO_ZONE { n - 1 } else { s.zone as i32 };
            let k = (i + d).rem_euclid(n);
            s.zone = if k == n - 1 { NO_ZONE } else { k as u8 };
        }
        F::Name | F::Save => {}
    }
    let _ = app;
    s
}

/// OK: the channel is written; Save stays on the VFO (V/M now reaches it), Add repeater goes to it and ends
/// in the channel editor (MENU-10).
fn save(s: Save, app: &mut App, store: &impl ChannelStore) {
    if let Some(why) = s.refuse(app) {
        return app.show_toast(b"Not saved:", why.split(':').next().unwrap_or(why).as_bytes(), false);
    }
    let a = app.active as usize;
    let f = s.freq_of(app);
    let cfg = match s.kind {
        Kind::Repeater => {
            let b = &BANDS[band_near(f)];
            let mut c = DEFAULT_CFG;
            (c.mode, c.offset, c.tx_tone) = (b.mode, s.offset, s.tone);
            c
        }
        // CHAN-26: the frequency and mode; HF mode keeps its own step, bandwidth and AGC
        Kind::Hf => SlotCfg { mode: app.set.hf.mode, narrow: false, step: BANDS[band_near(f)].step, ..DEFAULT_CFG },
        Kind::Vfo => app.slots[a].cfg,
    };
    let cfg = SlotCfg { squelch: SQ_GLOBAL, ..cfg }; // a new channel follows the slot's level (CHAN-14)
    let mut ch = Channel { num: s.num, zone: s.zone, rx: f, name: [0; 16], cfg, flags: 0, tones: 0 };
    ch.set_name(app.text.as_bytes());
    app.write(ChanWrite::Put { ch, from: None });
    if s.kind == Kind::Hf {
        let mut l = Buf::<32>::new();
        l.push(b"Saved as ");
        chan_label(s.num, &mut l);
        app.show_toast(l.as_bytes(), b"HF channel", false);
        return app.back(); // HF mode, as it was
    }
    let pos = (0..store.zone_len(s.zone)).filter_map(|i| store.zone_at(s.zone, i)).filter(|o| o.num < s.num).count() as u16;
    let sl = &mut app.slots[a];
    (sl.ch, sl.zone, sl.pos) = (Some(ch), s.zone, pos);
    let mut l = Buf::<32>::new();
    l.push(b"Saved as ");
    chan_label(s.num, &mut l);
    let z = store.zone_label(s.zone, b"no zone", b"");
    app.show_toast(l.as_bytes(), z.as_bytes(), false);
    if s.repeater() {
        app.slots[a].vfo_mode = false;
        app.send_slot(a as u8);
        app.screen = Screen::ChEdit(ChEdit { cell: 0 });
    } else {
        app.back();
    }
}
