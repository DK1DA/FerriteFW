//! The tone picker (OK on MENU 4 or on the tone cell; manual chapters 16 and 11, TONE-2, MENU-17): the RX and
//! TX tones apart, each off, one of the 50 CTCSS tones, or one of the 104 DCS codes normal or inverted, so
//! every cross mode is just a pair. ▲▼ pick RX or TX; the knob or ◀▶ step through the list, live; digits type
//! a tone (885 is 88.5 Hz, 1000 is 100.0 Hz) or, on a DCS code, the code in octal; # turns it off; OK keeps;
//! BACK puts both back (MENU-20). Typed tones need not be on the lists (TONE-15, as nicFW): any CTCSS from
//! 60.0 to 260.0 Hz and any DCS code D000-D777.

use super::forms::picker;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::slot::{tone_at, tone_custom, tone_index, tone_text, CTCSS, DCS_FLAG, DCS_INV};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_core::msg::{Param, RadioCmd};
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tones {
    /// 0 RX, 1 TX.
    pub row: u8,
    /// Both tones when it opened (BACK restores them).
    pub old: (u16, u16),
    /// Digits typed so far and when the last came (a pause of [`TYPE_MS`] starts again).
    pub typed: u16,
    pub nt: u8,
    pub at: u32,
}

const TYPE_MS: u32 = 1500;
/// Off, 50 CTCSS, 104 DCS normal, 104 DCS inverted.
const ALL: i32 = 1 + 50 + 104 + 104;

impl Tones {
    pub fn new(app: &App) -> Self {
        let c = app.slots[app.active as usize].cfg();
        Tones { row: 1, old: (c.rx_tone, c.tx_tone), typed: 0, nt: 0, at: 0 }
    }
}

fn get(app: &App, rx: bool) -> u16 {
    let c = app.slots[app.active as usize].cfg();
    if rx { c.rx_tone } else { c.tx_tone }
}

/// Sets one tone on the active slot, live (MENU-19), kept for undo and saved on a channel (EDIT-4).
pub(crate) fn set(app: &mut App, rx: bool, t: u16) {
    if get(app, rx) == t {
        return;
    }
    app.keep_undo();
    let a = app.active;
    let c = app.slots[a as usize].cfg_mut();
    if rx {
        c.rx_tone = t;
    } else {
        c.tx_tone = t;
    }
    let param = if rx { Param::RxTone } else { Param::TxTone };
    app.out.push(RadioCmd::SlotParam { slot: a, param, value: t as i32 });
    app.save_channel(a, None);
}

/// "CTCSS", "DCS normal", "DCS inverted", "Off"; "Custom ..." off the lists (TONE-15).
fn kind(t: u16) -> &'static str {
    match t {
        0 => "",
        _ if t & DCS_FLAG == 0 && tone_custom(t) => "CTCSS custom",
        _ if t & DCS_FLAG == 0 => "CTCSS",
        _ if tone_custom(t) => "DCS custom",
        _ if t & DCS_INV == 0 => "DCS normal",
        _ => "DCS inverted",
    }
}

fn text(t: u16, b: &mut Buf<16>) {
    tone_text(t, b);
    if b.is_empty() {
        b.set(b"Off");
    }
}

/// The lowest and highest CTCSS a typed tone may be, 0.1 Hz (TONE-15).
pub const CTCSS_MIN: u16 = 600;
pub const CTCSS_MAX: u16 = 2600;

/// Typed digits to a tone: CTCSS in tenths ("885", "1000") or whole hertz ("100"), a standard tone first, then
/// any tone from 60.0 to 260.0 Hz (TONE-15); on a DCS code, its octal digits, any code D000-D777.
pub fn typed_tone(v: u16, dcs: Option<u16>) -> Option<u16> {
    match dcs {
        Some(inv) => {
            let (mut code, mut x, mut k) = (0u16, v, 1u16);
            while x > 0 {
                if x % 10 > 7 || k > 64 {
                    return None;
                }
                code += (x % 10) * k;
                (x, k) = (x / 10, k * 8);
            }
            Some(DCS_FLAG | inv | code)
        }
        None => {
            let ok = |c: u16| (CTCSS_MIN..=CTCSS_MAX).contains(&c).then_some(c);
            let whole = v.checked_mul(10);
            CTCSS.iter().copied().find(|&c| c == v || Some(c) == whole).or_else(|| ok(v)).or_else(|| whole.and_then(ok))
        }
    }
}

impl View for Tones {
    fn name(self) -> &'static str {
        "Tone picker"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Tones");
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let mut s = self;
        let rx = self.row == 0;
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => {
                s.row ^= 1;
                s.nt = 0;
            }
            (Key::Digit(d), Short) => {
                if s.nt >= 4 || app.now.wrapping_sub(s.at) > TYPE_MS {
                    (s.typed, s.nt) = (0, 0);
                }
                (s.typed, s.nt, s.at) = (s.typed * 10 + d as u16, s.nt + 1, app.now);
                let t = get(app, rx);
                let dcs = (t & DCS_FLAG != 0).then_some(t & DCS_INV);
                if let Some(t) = typed_tone(s.typed, dcs) {
                    set(app, rx, t);
                }
            }
            (Key::Hash, Short) => set(app, rx, 0), // MENU-18: off is the default
            (Key::Star, Short) => app.do_undo(),
            (Key::Menu, Short) => return app.back(), // MENU-20: OK keeps
            (Key::Exit, Short) => {
                set(app, true, self.old.0);
                set(app, false, self.old.1);
                app.undo = None;
                return app.back();
            }
            _ => return,
        }
        app.screen = Screen::Tones(s);
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let rx = self.row == 0;
        let t = get(app, rx);
        // A custom tone sits after the standard one below it (tone_index): down steps to that one, not past it.
        let i = tone_index(t) + if tone_custom(t) && d < 0 { 1 } else { 0 };
        set(app, rx, tone_at(i + d));
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.editor_head(lcd, cx);
        let (rx, tx) = (get(app, true), get(app, false));
        for (i, (label, t)) in [("RX tone", rx), ("TX tone", tx)].into_iter().enumerate() {
            let f = self.row as usize == i;
            if app.dirty(R_ROW + i, hash(&(t, f))) {
                let y = 46 + i as i32 * 36;
                let (bg, border) = if f { (FOCUS, BLUE) } else { (BG, LINE) };
                // 33 px: the 9 px capitals centre (DISP-19)
                lcd.fill(6, y, 228, 33, BG);
                lcd.fill_r(6, y, 228, 33, 2, bg);
                let mut b = Buf::<16>::new();
                text(t, &mut b);
                w::field(lcd, 14, 70, y + 2, y + 30, y + 21, Align::Left, label.as_bytes(), &UI_12, if f { BLUE } else { SUB }, bg);
                w::field(lcd, 84, 70, y + 2, y + 30, y + 21, Align::Left, b.as_bytes(), &UI_12B, TEXT, bg);
                w::field(lcd, 154, 72, y + 2, y + 30, y + 21, Align::Right, kind(t).as_bytes(), &UI_12, SUB, bg);
                if f {
                    w::knob_mark(lcd, 14 + text4_width(label.as_bytes(), &UI_12) + 4, y + 9, BLUE, bg);
                }
                lcd.frame_r(6, y, 228, 33, 2, border);
            }
        }
        let t = if self.row == 0 { rx } else { tx };
        if app.dirty(R_SUB, hash(&t)) {
            picker(lcd, 40, 122, 160, 5, 18, tone_index(t), ALL, |i, b| text(tone_at(i), b), BG);
        }
        let mut typed = Buf::<32>::new();
        if self.nt > 0 && app.now.wrapping_sub(self.at) <= TYPE_MS {
            let _ = ufmt::uwrite!(typed, "Typed {}", self.typed);
        }
        if app.dirty(R_NOTE, hash(&typed)) {
            let s: &[u8] = if typed.is_empty() { b"885 is 88.5 Hz; any 60-260 Hz" } else { typed.as_bytes() };
            w::field(lcd, 8, 224, 216, 233, 229, Align::Center, s, &UI_12, if typed.is_empty() { DIM } else { TEXT }, BG);
        }
        app.help_box(lcd, 250, 46, b"RX and TX are set apart: any cross mode is a pair.", b"MENU 4");
    }

    fn hints(self, app: &App) -> Hints {
        if app.undo.is_some() {
            Hints::new(&[(ARROWS_UD, "RX/TX"), ("*", "Undo"), ("OK", "Keep")])
        } else {
            Hints::new(&[(ARROWS_UD, "RX/TX"), ("Knob", "Change"), ("OK", "Keep")])
        }
    }
}
