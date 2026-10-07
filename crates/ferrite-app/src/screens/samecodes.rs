//! SAME location codes (A13; manual chapter 23, WX-10; MENU 0 4 9 2): up to 8 six-digit codes, FIPS in the
//! US and CLC in Canada. A warning shows when any of its areas is in the list; with none, every warning shows.
//! OK edits the selected place (digits type, OK keeps six digits, BACK deletes one, then cancels); * clears
//! it. US and Canada only: elsewhere the screen says so (and the menu item is hidden, WX-3).

use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::keys::{KeyEvent, Press};
use crate::settings::SAME_CODES;
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::{Buf, Zero};
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SameCodes {
    pub cur: u8,
    /// Editing the selected place: the digits typed so far (packed, 6 at most) and how many; `edit` false = the list.
    edit: bool,
    buf: [u8; 3],
    n: u8,
}

const DIGITS: u8 = 6;

impl SameCodes {
    pub fn new() -> Self {
        SameCodes { cur: 0, edit: false, buf: [0; 3], n: 0 }
    }

    fn digit(self, i: u8) -> u8 {
        self.buf[i as usize / 2] >> (4 * (i % 2)) & 15
    }

    fn value(self) -> u32 {
        (0..self.n).fold(0, |v, i| v * 10 + self.digit(i) as u32)
    }

    fn typed(mut self, d: u8) -> Self {
        let i = self.n as usize;
        self.buf[i / 2] = self.buf[i / 2] & !(15 << (4 * (i % 2))) | d << (4 * (i % 2));
        self.n += 1;
        self
    }
}

impl View for SameCodes {
    fn name(self) -> &'static str {
        "Location codes"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Locations");
    }

    fn timeout(self) -> Option<u32> {
        Some(MENU_TIMEOUT_MS)
    }

    fn key(self, app: &mut App, e: KeyEvent, _: &impl ChannelStore) {
        use Press::*;
        let mut s = self;
        if !app.set.wx_region() {
            if let (Key::Exit, Short) = (e.key, e.press) {
                app.back();
            }
            return;
        }
        let i = s.cur as usize % SAME_CODES;
        match (e.key, e.press, s.edit) {
            (Key::Up | Key::Down, Short | Repeat, false) => {
                let d = if e.key == Key::Up { SAME_CODES - 1 } else { 1 };
                s.cur = ((i + d) % SAME_CODES) as u8;
            }
            (Key::Menu, Short, false) => (s.edit, s.n) = (true, 0),
            (Key::Star, Short, false) if app.set.same_codes[i] != 0 => {
                app.set.same_codes[i] = 0;
                app.show_toast(b"Location code", b"cleared", false);
            }
            (Key::Exit, Short, false) => return app.back(),
            (Key::Digit(d), Short, true) if s.n < DIGITS => s = s.typed(d),
            (Key::Menu, Short, true) if s.n == DIGITS && s.value() != 0 => {
                app.set.same_codes[i] = s.value();
                s.edit = false;
                let mut b = Buf::<16>::new();
                let _ = uwrite!(b, "{}", Zero(s.value(), 6));
                app.show_toast(b"Location code", b.as_bytes(), false);
            }
            (Key::Menu, Short, true) => app.show_toast(b"Location code", b"six digits, not all 0", false),
            (Key::Exit, Short, true) if s.n > 0 => s.n -= 1,
            (Key::Exit, Short, true) => s.edit = false,
            _ => return,
        }
        app.screen = Screen::SameCodes(s);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, _: &Cx<impl ChannelStore>) {
        app.page(lcd, app.set.wx_region());
        if !app.set.wx_region() {
            app.list_head(lcd, b"Location codes", b"");
            app.empty(lcd, b"US and Canada only", b"SAME alerts aren't sent here");
            return;
        }
        let used = app.set.same_codes.iter().filter(|&&c| c != 0).count();
        let mut r = Buf::<16>::new();
        let _ = uwrite!(r, "{} of {}", used, SAME_CODES);
        let left: &[u8] = if app.set.region == 2 { b"CLC codes" } else { b"FIPS codes" };
        app.list_head(lcd, left, r.as_bytes());
        for k in 0..SAME_CODES {
            let mut name = Buf::<8>::new();
            let _ = uwrite!(name, "{}", k + 1);
            let mut v = Buf::<16>::new();
            let sel = k == self.cur as usize;
            if sel && self.edit {
                for j in 0..DIGITS {
                    v.push(&[if j < self.n { b'0' + self.digit(j) } else { b'_' }]);
                }
            } else {
                match app.set.same_codes[k] {
                    0 => v.set(b"-"),
                    c => {
                        let _ = uwrite!(v, "{}", Zero(c, 6));
                    }
                }
            }
            app.row1(lcd, k, name.as_bytes(), v.as_bytes(), false, sel, false);
        }
        let help: &[u8] = if used == 0 {
            b"None set: every warning shows. Six digits each."
        } else {
            b"A warning shows when one of its areas is here."
        };
        app.help_box(lcd, 300 - 38, 34, help, b"MENU 0 4 9 2");
    }

    fn hints(self, _: &App) -> Hints {
        if self.edit {
            Hints::new(&[("0-9", "Type"), ("OK", "Keep"), ("BACK", "Delete")])
        } else {
            Hints::new(&[("OK", "Edit"), ("*", "Clear"), ("BACK", "Back")])
        }
    }
}
