//! PIN entry (at power on with a PIN set, and at unlock when chosen; manual chapters 34, 5 and 7, PIN-1 to
//! PIN-3, BOOT-13, KEY-16): masked digits, attempts left, and after three wrong entries a 30 s wait. Nothing
//! else works until it is right; at unlock, BACK leaves the keys locked. The settings-reset hint is always
//! shown (PIN-2).

use super::forms::para;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// Three wrong entries, then a wait (PIN-3).
pub const TRIES: u8 = 3;
pub const WAIT_MS: u32 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pin {
    /// Asked at power on (nothing to go back to), not at unlock.
    pub boot: bool,
    pub typed: [u8; 6],
    pub n: u8,
    pub misses: u8,
    /// While waiting after three misses: when the wait ends (0: no wait).
    pub wait_until: u32,
}

impl Pin {
    pub fn new(boot: bool) -> Self {
        Pin { boot, typed: [0; 6], n: 0, misses: 0, wait_until: 0 }
    }

    fn waiting(self, now: u32) -> Option<u32> {
        let r = self.wait_until.wrapping_sub(now) as i32;
        (self.wait_until != 0 && r > 0).then_some(r as u32)
    }
}

impl View for Pin {
    fn name(self) -> &'static str {
        "PIN entry"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"PIN");
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        let mut s = self;
        if s.waiting(app.now).is_some() {
            return;
        }
        let len = app.prefs.pin_len.clamp(1, 6);
        match (e.key, e.press) {
            (Key::Digit(d), Press::Short) if s.n < len => {
                s.typed[s.n as usize] = d;
                s.n += 1;
            }
            (Key::Exit, Press::Short) if s.n > 0 => s.n -= 1,
            (Key::Exit, Press::Short) if !s.boot => return app.back(), // still locked
            _ => return,
        }
        if s.n == len {
            if s.typed[..len as usize] == app.prefs.pin[..len as usize] {
                if !s.boot {
                    app.locked = false;
                    app.show_toast(b"Keys unlocked", b"", false);
                }
                return app.go_home();
            }
            s.n = 0;
            s.misses += 1;
            if s.misses.is_multiple_of(TRIES) {
                s.wait_until = app.now.wrapping_add(WAIT_MS).max(1); // PIN-3
            }
        }
        app.screen = Screen::Pin(s);
    }

    /// PIN-1: before anything else is usable, PTT included at power on.
    fn ptt(self, app: &mut App, down: bool) {
        if down && !self.boot {
            super::nav::transmit(app);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        let len = app.prefs.pin_len.clamp(1, 6) as i32;
        let wait = self.waiting(app.now).map(|ms| ms.div_ceil(1000));
        if app.dirty(R_TOP, hash(&self.boot)) {
            lcd.fill(0, 21, 240, 279, BG);
            w::field(lcd, 8, 224, 50, 70, 65, Align::Center, b"Enter your PIN", &UI_12B, TEXT, BG);
            para(lcd, 16, 220, 208, 4, 17, b"Forgotten it? Hold MENU while switching on: a settings reset clears the PIN and keeps your channels.", &UI_12, SUB, BG);
        }
        if app.dirty(R_FREQ, hash(&(self.n, len))) {
            let x0 = 120 - (len * 28 - 12) / 2;
            lcd.fill(0, 96, 240, 20, BG);
            for i in 0..len {
                let x = x0 + i * 28;
                if i < self.n as i32 {
                    lcd.fill_r(x, 98, 16, 16, 8, TEXT); // PIN-2: masked
                } else {
                    lcd.fill_r(x, 98, 16, 16, 8, SUB);
                    lcd.fill_r(x + 2, 100, 12, 12, 6, BG);
                }
            }
        }
        let mut t = Buf::<40>::new();
        let mut col = SUB;
        match wait {
            Some(s) => {
                let _ = uwrite!(t, "Three wrong entries. Wait {} s", s);
                col = AMBER;
            }
            None if self.misses > 0 => {
                let _ = uwrite!(t, "Wrong PIN. {} attempts left", TRIES - self.misses % TRIES);
                col = AMBER;
            }
            None => {
                let _ = uwrite!(t, "{} digits", len);
            }
        }
        if app.dirty(R_NOTE, hash(&(t, col))) {
            w::field(lcd, 8, 224, 136, 154, 150, Align::Center, t.as_bytes(), &UI_12, col, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.boot {
            Hints::new(&[("0-9", "PIN"), ("BACK", "Delete")])
        } else {
            Hints::new(&[("0-9", "PIN"), ("BACK", "Cancel")])
        }
    }
}
