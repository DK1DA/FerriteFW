//! Text entry (manual chapter 7, KEY-12; SETUP-12): multi-tap on the digits, \* cycles upper case, lower case,
//! digits and symbols, # is a space, BACK deletes (with nothing left it leaves), OK keeps. The text being typed
//! is the App's one text buffer; the screen that opened it reads it back (the save form's name) or OK hands it
//! over (a channel's name, the callsign). A key tapped again within a second steps through its letters.
//! Callsigns are capitals and digits only, and * switches letters and digits (the SetupCall mock-up). ◀▶ move
//! the cursor (TEXT-12): typing, spaces and BACK work at it.

use super::forms::{field_box, keypad, para};
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// Whose text it is: where OK writes it. Every caller names its own target ([`super::nav::Dest::Text`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextFor {
    /// The channel the active slot is on, in the channel editor (A4).
    ChannelName,
    /// A stored channel by number: rename from the channel browser (C8).
    Channel(u16),
    /// The new channel in Save as channel or Add repeater (A9, A10): the form reads the buffer.
    SaveName,
    /// A zone by number: rename in the zone editor (A12).
    Zone(u8),
    /// A range preset by index: its name in the preset editor (A11).
    Preset(u8),
    /// 0 8 7 1 (SETUP-12).
    Callsign,
    /// 0 7 3 2's text, the boot screen's line (BOOT-9).
    PowerOn,
    /// A DTMF memory's name (DTMF-12).
    DtmfName(u8),
    /// A grid square (GPS-21): the fixed position (A8), or a new waypoint.
    GridFixed,
    GridWaypoint,
}

impl TextFor {
    fn label(self) -> &'static str {
        match self {
            TextFor::ChannelName | TextFor::Channel(_) | TextFor::SaveName => "Channel name",
            TextFor::Zone(_) => "Zone name",
            TextFor::Preset(_) => "Range name",
            TextFor::Callsign => "Callsign",
            TextFor::PowerOn => "Power-on message",
            TextFor::DtmfName(_) => "DTMF memory name",
            TextFor::GridFixed | TextFor::GridWaypoint => "Grid square",
        }
    }

    pub fn max(self) -> usize {
        match self {
            TextFor::Callsign => 10,
            TextFor::Preset(_) => 12,
            TextFor::DtmfName(_) => 10,
            TextFor::GridFixed | TextFor::GridWaypoint => 6,
            _ => 16,
        }
    }

    /// What the text starts as: the name or callsign being changed (SaveName: the form's own, already there).
    pub(crate) fn current(self, app: &App, store: &impl ChannelStore) -> Buf<16> {
        let s = &app.slots[app.active as usize];
        match self {
            TextFor::ChannelName => s.ch.map_or(Buf::new(), |c| Buf::from(c.name())),
            TextFor::Channel(n) => store.by_num(n).map_or(Buf::new(), |(c, _)| Buf::from(c.name())),
            TextFor::SaveName => app.text,
            TextFor::Zone(z) => store.zone_name(z).unwrap_or_default(),
            TextFor::Preset(i) => store.data().preset(i).map_or(Buf::new(), |p| Buf::from(p.name.as_bytes())),
            TextFor::Callsign => Buf::from(app.set.callsign.as_bytes()),
            TextFor::PowerOn => Buf::from(app.set.pon_text.as_bytes()),
            TextFor::DtmfName(i) => Buf::from(store.data().dtmf_name(i).as_bytes()),
            TextFor::GridFixed if app.set.fixed_on => Buf::from(&super::lists::grid(app.set.fixed)),
            TextFor::GridFixed | TextFor::GridWaypoint => Buf::new(),
        }
    }

    /// Capitals and digits only, * switching them (a callsign, a grid square).
    fn callsign(self) -> bool {
        matches!(self, TextFor::Callsign | TextFor::GridFixed | TextFor::GridWaypoint)
    }
}

/// Multi-tap: what each key types in each mode (KEY-12). Mode 2 (digits) types the digit itself.
pub(crate) const UPPER: [&[u8]; 10] = [b"0", b".-/1", b"ABC2", b"DEF3", b"GHI4", b"JKL5", b"MNO6", b"PQRS7", b"TUV8", b"WXYZ9"];
pub(crate) const LOWER: [&[u8]; 10] = [b"0", b".-/1", b"abc2", b"def3", b"ghi4", b"jkl5", b"mno6", b"pqrs7", b"tuv8", b"wxyz9"];
const SYMBOLS: [&[u8]; 10] = [b"0", b".,?!", b"-/_'", b"()&+", b"@#*=", b":;%\"", b"<>[]", b"$^~|", b"{}\\`", b"0123"];
const DIGITS: [&[u8]; 10] = [b"0", b"1", b"2", b"3", b"4", b"5", b"6", b"7", b"8", b"9"];
pub(crate) const MODES: [&str; 4] = ["ABC", "abc", "123", "Sym"];
/// A key tapped again within this steps through its letters.
pub(crate) const TAP_MS: u32 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Text {
    pub to: TextFor,
    /// Index into [`MODES`].
    pub mode: u8,
    /// Multi-tap: the last digit key + 1 (0 none) and when it was tapped.
    pub last: u8,
    /// The cursor (TEXT-12): how many characters are after it; 0 at the end.
    pub tail: u8,
    pub at: u32,
}

impl Text {
    pub fn new(to: TextFor) -> Self {
        Text { to, mode: 0, last: 0, tail: 0, at: 0 }
    }

    /// The mode * goes to: KEY-12's case and symbols; a callsign has capitals and digits only (SETUP-12).
    fn next_mode(self) -> u8 {
        match (self.to.callsign(), self.mode) {
            (true, 0) => 2,
            (true, _) => 0,
            (false, m) => (m + 1) % 4,
        }
    }
}

/// Opens text entry for `to`, starting from its current text, remembering the screen that asked for BACK.
pub(crate) fn open(app: &mut App, to: TextFor, store: &impl ChannelStore) {
    app.text = to.current(app, store);
    app.open(super::nav::Dest::Text(to));
}

/// The characters key d types in a mode.
pub(crate) fn legend(mode: u8, d: u8) -> &'static [u8] {
    [UPPER, LOWER, DIGITS, SYMBOLS][mode as usize % 4][d as usize % 10]
}

/// One digit key into a text: a new character, or the next one on the same key within [`TAP_MS`]. Returns
/// the (last, at) pair to remember. Shared with setup's callsign page.
pub(crate) fn tap(b: &mut Buf<16>, max: usize, mode: u8, d: u8, last: u8, at: u32, now: u32) -> (u8, u32) {
    let keys = legend(mode, d);
    let again = last == d + 1 && now.wrapping_sub(at) < TAP_MS && !b.is_empty() && keys.len() > 1;
    if again {
        let cur = b.as_bytes()[b.len() - 1];
        let i = keys.iter().position(|&c| c == cur).map_or(0, |i| (i + 1) % keys.len());
        let mut t = Buf::<16>::from(&b.as_bytes()[..b.len() - 1]);
        t.push(&keys[i..i + 1]);
        *b = t;
    } else if b.len() < max {
        b.push(&keys[..1]);
    }
    (d + 1, now)
}

/// BACK in a text: the last character goes.
pub(crate) fn delete(b: &mut Buf<16>) {
    let n = b.len().saturating_sub(1);
    *b = Buf::from(&b.as_bytes()[..n]);
}

impl View for Text {
    fn name(self) -> &'static str {
        "Text entry"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Text");
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn own_arrows(self) -> bool {
        true
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        let mut s = self;
        // TEXT-12: the keys work on the text before the cursor; what is after it is put back after.
        let n = app.text.len();
        let k = (s.tail as usize).min(n);
        let after = Buf::<16>::from(&app.text.as_bytes()[n - k..]);
        app.text = Buf::from(&app.text.as_bytes()[..n - k]);
        let max = self.to.max() - k;
        match (e.key, e.press) {
            (Key::Left, Press::Short | Press::Repeat) => {
                s.tail = (k + !app.text.is_empty() as usize) as u8;
                s.last = 0;
            }
            (Key::Right, Press::Short | Press::Repeat) => {
                s.tail = k.saturating_sub(1) as u8;
                s.last = 0;
            }
            (Key::Digit(d), Press::Short) => {
                (s.last, s.at) = tap(&mut app.text, max, s.mode, d, s.last, s.at, app.now);
            }
            (Key::Star, Press::Short) => {
                // KEY-12: case and symbols; a callsign has capitals and digits only (SETUP-12).
                s.mode = self.next_mode();
                s.last = 0;
            }
            (Key::Hash, Press::Short) if !self.to.callsign() => {
                if app.text.len() < max {
                    app.text.push(b" ");
                }
                s.last = 0;
            }
            (Key::Exit, Press::Short) if app.text.is_empty() && after.is_empty() => return app.back(),
            (Key::Exit, Press::Short) => {
                delete(&mut app.text); // at the start, nothing before the cursor to delete
                s.last = 0;
            }
            (Key::Menu, Press::Short) => {
                app.text.push(after.as_bytes());
                return keep(self.to, app, store);
            }
            _ => {}
        }
        app.text.push(after.as_bytes());
        app.screen = Screen::Text(s);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        let t = app.text;
        if app.dirty(R_TOP, hash(&(self.to, t, self.mode, self.tail))) {
            w::field(lcd, 8, 224, 24, 41, 37, Align::Left, self.to.label().as_bytes(), &UI_12B, TEXT, BG);
            field_box(lcd, 8, 44, 224, 30, b"", t.as_bytes(), true, self.tail == 0);
            if self.tail > 0 {
                // TEXT-12: the cursor inside the text, where field_box draws it (8 px in, baseline 64)
                let k = t.len() - (self.tail as usize).min(t.len());
                let x = 16 + text4_width(&t.as_bytes()[..k], &UI_12B);
                lcd.fill(x, 64 - 12, 2, 15, TEXT);
            }
            let mut c = Buf::<16>::new();
            let _ = uwrite!(c, "{}/{}", t.len(), self.to.max());
            w::field(lcd, 8, 100, 78, 95, 91, Align::Left, c.as_bytes(), &UI_12, SUB, BG);
            lcd.fill(108, 78, 132, 17, BG);
            w::tag(lcd, 232, 78, MODES[self.mode as usize % 4].as_bytes(), BLUE, BG);
        }
        if app.dirty(R_HELP, hash(&(self.mode, self.to))) {
            let m = self.mode;
            let (star, hash_): (&[u8], &[u8]) = if self.to.callsign() {
                (if m == 0 { b"digits" } else { b"letters" }, b"")
            } else {
                (MODES[self.next_mode() as usize].as_bytes(), b"space")
            };
            keypad(lcd, 104, |d| legend(m, d), star, hash_);
            let tip: &[u8] = if self.to.callsign() {
                b"Tap a key again for its next letter. Capitals only."
            } else {
                b"Tap a key again for its next letter; * changes case."
            };
            para(lcd, 8, 206, 224, 2, 17, tip, &UI_12, SUB, BG);
        }
    }

    fn hints(self, app: &App) -> Hints {
        let back = if app.text.is_empty() { ("BACK", "Back") } else { ("BACK", "Delete") };
        Hints::new(&[("*", MODES[self.next_mode() as usize]), ("OK", "Keep"), back])
    }
}

/// OK: the text goes to the record it was typed for, and the screen that asked comes back.
fn keep(to: TextFor, app: &mut App, store: &impl ChannelStore) {
    let t = app.text;
    match to {
        TextFor::ChannelName => super::chedit::edit(app, |c| c.set_name(t.as_bytes())),
        TextFor::Channel(n) => {
            if let Some((mut c, _)) = store.by_num(n) {
                c.set_name(t.as_bytes());
                app.write(crate::chan::ChanWrite::Put { ch: c, from: None });
                // A slot on that channel shows the new name too.
                for s in app.slots.iter_mut().filter(|s| s.ch.is_some_and(|c| c.num == n)) {
                    if let Some(sc) = &mut s.ch {
                        sc.set_name(t.as_bytes());
                    }
                }
            }
        }
        TextFor::SaveName => {}
        TextFor::Zone(z) => {
            store.data().request(crate::data::Op::RenameZone(z, t));
        }
        TextFor::Preset(i) => {
            let d = store.data();
            if let Some(mut p) = d.preset(i) {
                p.name = Buf::from(t.as_bytes());
                d.request(crate::data::Op::Preset(i, p));
            }
        }
        TextFor::Callsign => app.set.set_callsign(t.as_bytes()), // SETUP-12
        TextFor::PowerOn => {
            app.set.pon_text = crate::settings::Text::EMPTY;
            t.as_bytes().iter().for_each(|&c| _ = app.set.pon_text.push(c));
        }
        TextFor::DtmfName(i) => {
            store.data().request(crate::data::Op::DtmfName(i, Buf::from(t.as_bytes())));
        }
        TextFor::GridFixed | TextFor::GridWaypoint => {
            // GPS-21: 4 or 6 characters, the square's centre; anything else stays here to be fixed
            let Some(p) = super::lists::grid_pos(t.as_bytes()) else {
                return app.show_toast(b"Grid square", b"like FN31 or FN31PR", false);
            };
            app.back();
            if to == TextFor::GridFixed {
                (app.set.fixed, app.set.fixed_on) = (p, true);
                return app.show_toast(b"Fixed position", &super::lists::grid(p), false);
            }
            if store.data().request(crate::data::Op::AddWaypoint(p, Buf::from(t.as_bytes()))) {
                app.show_toast(b"Waypoint added", t.as_bytes(), false);
            } else {
                app.show_toast(b"Waypoint", b"no room", false);
            }
            return;
        }
    }
    app.back();
}
