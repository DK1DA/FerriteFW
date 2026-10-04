//! PIN entry (A17; manual chapter 34, PIN-1 to PIN-3, BOOT-13, KEY-16): masked digits, attempts left, the
//! settings-reset hint, and a 30 s wait after three misses. Three uses: at power on ([`App::ask_pin`]; nothing
//! else works until it's right, hold BACK included), at unlock (hold # while locked, with "at power on and
//! unlock"), and setting a new PIN (MENU 0 9 3 1), where the digits show as typed.

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

/// Misses before the wait, and the wait (PIN-3).
const TRIES: u8 = 3;
const WAIT_S: u16 = 30;
/// PIN length (0 9 3 1: 4-6 digits).
const MIN: u8 = 4;
const MAX: u8 = 6;

/// What the PIN is asked for.
const POWER_ON: u8 = 0;
const UNLOCK: u8 = 1;
const SET: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pin {
    /// The digits typed, packed two to a byte, and how many.
    digits: [u8; 3],
    n: u8,
    /// POWER_ON, UNLOCK, or SET + the PIN mode to switch on.
    purpose: u8,
}

impl Pin {
    pub fn power_on() -> Self {
        Pin { digits: [0; 3], n: 0, purpose: POWER_ON }
    }

    pub fn unlock() -> Self {
        Pin { purpose: UNLOCK, ..Pin::power_on() }
    }

    /// Setting a new PIN, then turning on PIN mode `mode` (PIN_MODES).
    pub fn set(mode: u8) -> Self {
        Pin { purpose: SET + mode, ..Pin::power_on() }
    }

    /// Asked at power on: nothing else is usable until it's right (PIN-1).
    pub fn blocks(self) -> bool {
        self.purpose == POWER_ON
    }

    fn secs(app: &App) -> u16 {
        (app.now / 1000) as u16
    }

    /// Seconds still to wait (PIN-3).
    fn waiting(self, app: &App) -> u16 {
        if app.set.pin_wait == 0 {
            return 0;
        }
        let left = app.set.pin_wait.wrapping_sub(Pin::secs(app)) as i16;
        left.max(0) as u16
    }

    fn digit(self, i: usize) -> u8 {
        self.digits[i / 2] >> (4 * (i % 2)) & 15
    }

    /// The digits typed as a number.
    fn value(self) -> u32 {
        (0..self.n as usize).fold(0, |v, i| v * 10 + self.digit(i) as u32)
    }

    fn typed(mut self, d: u8) -> Self {
        let i = self.n as usize;
        self.digits[i / 2] = self.digits[i / 2] & !(15 << (4 * (i % 2))) | d << (4 * (i % 2));
        self.n += 1;
        self
    }

    fn put(self, app: &mut App) {
        app.screen = Screen::Pin(self);
    }

    fn submit(self, app: &mut App) {
        if self.n < MIN {
            return app.show_toast(b"PIN", b"4 to 6 digits", false);
        }
        if self.purpose >= SET {
            let s = &mut app.set;
            (s.pin, s.pin_len, s.pin_mode) = (self.value(), self.n, self.purpose - SET);
            app.back();
            return app.show_toast(b"PIN set", b"", false);
        }
        if self.value() == app.set.pin && self.n == app.set.pin_len {
            app.set.pin_misses = 0;
            if self.purpose == UNLOCK {
                app.locked = false;
                app.back();
                app.show_toast(b"Keys unlocked", b"", false);
            } else {
                app.go_home();
            }
            return;
        }
        let now = Pin::secs(app);
        let s = &mut app.set;
        s.pin_misses += 1;
        if s.pin_misses >= TRIES {
            s.pin_misses = 0;
            s.pin_wait = now.wrapping_add(WAIT_S).max(1);
        }
        Pin { n: 0, ..self }.put(app);
        app.show_toast(b"Wrong PIN", b"", false);
    }
}

impl View for Pin {
    fn own_hold_menu(self) -> bool {
        true
    }

    fn name(self) -> &'static str {
        "PIN entry"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"PIN");
    }

    /// At power on and at unlock every key is the PIN's (PIN-1; the keys are locked otherwise, KEY-16).
    fn modal(self) -> bool {
        self.purpose < SET
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        if e.press != Press::Short {
            return;
        }
        let wait = self.waiting(app);
        match e.key {
            Key::Digit(d) if wait == 0 && self.n < MAX => self.typed(d).put(app),
            Key::Left => Pin { n: self.n.saturating_sub(1), ..self }.put(app),
            Key::Menu if wait == 0 => self.submit(app),
            Key::Exit if self.blocks() => Pin { n: self.n.saturating_sub(1), ..self }.put(app),
            Key::Exit => app.back(),
            _ => {}
        }
    }

    fn ptt(self, app: &mut App, down: bool) {
        // At power on PTT waits for the PIN too (PIN-1); otherwise PTT works as always (KEY-14).
        if down && !self.blocks() {
            super::nav::transmit(app);
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        let set = self.purpose >= SET;
        let head: &[u8] = if set { b"New PIN" } else { b"Enter PIN" };
        if app.dirty(R_TOP, hash(&head)) {
            w::field(lcd, 10, 220, 40, 66, 60, Align::Center, head, &UI_15B, TEXT, BG);
            let t: &[u8] = if set { b"4 to 6 digits." } else { b"This radio has a PIN." };
            w::field(lcd, 10, 220, 70, 88, 84, Align::Center, t, &UI_12, SUB, BG);
        }
        // Six boxes: a dot per digit typed (PIN-2), the digit itself when setting.
        let n = self.n;
        if app.dirty(R_ROW, hash(&(n, self.value(), set))) {
            let mut digits = [0u8; 6];
            for (i, d) in digits.iter_mut().enumerate().take(n as usize) {
                *d = b'0' + self.digit(i);
            }
            for i in 0..MAX as i32 {
                let (bx, filled) = (36 + 29 * i, i < n as i32);
                lcd.fill(bx, 110, 24, 32, BG);
                lcd.frame_r(bx, 110, 24, 32, 4, if i == n as i32 { BLUE } else { LINE });
                if filled && set {
                    lcd.text4_aligned(bx + 12, 132, Align::Center, &digits[i as usize..i as usize + 1], &UI_15B, TEXT, BG);
                } else if filled {
                    lcd.fill_r(bx + 8, 122, 8, 8, 4, TEXT);
                }
            }
        }
        let wait = self.waiting(app);
        let mut line = Buf::<32>::new();
        let mut c = SUB;
        if wait > 0 {
            let _ = uwrite!(line, "Wait {} s", wait);
            c = AMBER;
        } else if !set {
            let left = TRIES - app.set.pin_misses;
            let _ = uwrite!(line, "{} attempt{} left", left, if left == 1 { "" } else { "s" });
            if left < TRIES {
                c = AMBER;
            }
        }
        if app.dirty(R_NOTE, hash(&(line, c))) {
            w::field(lcd, 10, 220, 156, 174, 170, Align::Center, line.as_bytes(), &UI_12, c, BG);
        }
        if !set && app.dirty(R_HELP, 1) {
            w::wrap(lcd, 16, 208, 200, 260, 214, 15, b"Forgot it? Hold MENU while switching on: a settings reset clears the PIN.", &UI_12, SUB, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        let ok = if self.purpose >= SET { "Save" } else { "Unlock" };
        if self.blocks() {
            Hints::new(&[("0-9", "Type"), ("OK", ok), ("BACK", "Delete")])
        } else {
            Hints::new(&[("0-9", "Type"), ("OK", ok), ("BACK", "Cancel")])
        }
    }
}
