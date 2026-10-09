//! DTMF (manual chapter 17): the dialer (A14, MENU 0 8 6 1) and the memories (A15, MENU 0 8 6 2: 16 of up to
//! 16 symbols).
//!
//! Dialer: 0-9 type, * and # type themselves, holding 1-4 types A-D (a 12-key pad has no A-D keys); BACK
//! deletes, then leaves; OK sends on the active slot through the transmit checks (DTMF-1): keyed like PTT, the
//! digits at DTMF-3's speed after the first digit delay, then unkeyed; a receive-only build shows the refusal.
//! Hold MENU keeps the digits as a memory (the first empty one).
//!
//! Memories: OK opens one in the dialer, to send or edit it (hold MENU there writes it back; an empty one is
//! typed the same way); * clears one; # names one (up to 10 characters, DTMF-12: "Link on", "IRLP 3500").
//! nicFW has 99 named presets; 16 named ones cover a radio's links and controllers.

use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::{Dtmf, Op, DTMF_MEMS};
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen, MENU_TIMEOUT_MS};
use ferrite_core::keys::Key;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

/// The dialer's memory link: none (a plain dial) or the memory it was opened from.
const NONE: u8 = 0xFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DtmfPad {
    /// false: the dialer; true: the memories list.
    pub list: bool,
    /// The list's selection, or the memory the dialer edits (NONE: a plain dial).
    pub cur: u8,
    /// What the dialer holds.
    pub digits: Dtmf,
}

impl DtmfPad {
    pub fn dialer() -> Self {
        DtmfPad { list: false, cur: NONE, digits: Dtmf::default() }
    }

    pub fn memories() -> Self {
        DtmfPad { list: true, cur: 0, digits: Dtmf::default() }
    }
}

/// Sends what the dialer holds on the active slot: the transmit checks first (DTMF-1).
fn send(app: &mut App, d: &Dtmf) {
    if d.len == 0 {
        return app.show_toast(b"DTMF", b"type digits first", false);
    }
    let mut t = Buf::<16>::new();
    d.text(&mut t);
    super::transmit::dial(app, app.active, t.as_bytes());
}

/// Keeps the dialer's digits in memory `i`, or the first empty one.
fn keep(app: &mut App, s: DtmfPad, store: &impl ChannelStore) {
    let data = store.data();
    let at = if s.cur != NONE { Some(s.cur) } else { (0..DTMF_MEMS).find(|&i| data.dtmf(i).is_none()) };
    match at {
        Some(i) if s.digits.len > 0 && data.request(Op::Dtmf(i, Some(s.digits))) => {
            let mut b = Buf::<16>::new();
            let _ = uwrite!(b, "memory {}", i + 1);
            app.show_toast(b"DTMF saved", b.as_bytes(), false);
        }
        Some(_) if s.digits.len == 0 => app.show_toast(b"DTMF", b"type digits first", false),
        _ => app.show_toast(b"DTMF memories", b"all 16 are used", false),
    }
}

impl View for DtmfPad {
    fn name(self) -> &'static str {
        if self.list { "DTMF memories" } else { "DTMF dialer" }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(if self.list { b"DTMF mem" } else { b"DTMF" });
    }

    fn timeout(self) -> Option<u32> {
        self.list.then_some(MENU_TIMEOUT_MS)
    }

    fn own_hold_menu(self) -> bool {
        !self.list
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let mut s = self;
        if s.list {
            let data = store.data();
            match (e.key, e.press) {
                (Key::Up | Key::Down, Short | Repeat) => {
                    let d = if e.key == Key::Up { DTMF_MEMS - 1 } else { 1 };
                    s.cur = (s.cur + d) % DTMF_MEMS;
                }
                (Key::Menu, Short) => {
                    app.open(super::nav::Dest::DtmfDialer);
                    app.screen = Screen::Dtmf(DtmfPad { list: false, cur: s.cur, digits: data.dtmf(s.cur).unwrap_or_default() });
                    return;
                }
                (Key::Star, Short) if data.dtmf(s.cur).is_some() => {
                    data.request(Op::Dtmf(s.cur, None));
                    app.show_toast(b"DTMF memory", b"cleared", false);
                }
                (Key::Hash, Short) if data.dtmf(s.cur).is_some() => {
                    return super::text::open(app, super::text::TextFor::DtmfName(s.cur), store);
                }
                (Key::Hash, Short) => return app.show_toast(b"Empty memory", b"type digits first", false),
                (Key::Exit, Short) => return app.back(),
                _ => return,
            }
        } else {
            match (e.key, e.press) {
                (Key::Digit(d), Short) => {
                    s.digits.push(d);
                }
                // A-D: hold 1-4 (the pad has no A-D keys)
                (Key::Digit(d @ 1..=4), Long) => {
                    s.digits.push(9 + d);
                }
                (Key::Star, Short) => {
                    s.digits.push(14);
                }
                (Key::Hash, Short) => {
                    s.digits.push(15);
                }
                (Key::Menu, Short) => return send(app, &s.digits),
                (Key::Menu, Long) => return keep(app, s, store),
                (Key::Exit, Short) if s.digits.len > 0 => s.digits.pop(),
                (Key::Exit, Short) => return app.back(),
                _ => return,
            }
        }
        app.screen = Screen::Dtmf(s);
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.page(lcd, self.list);
        if self.list {
            let data = cx.store.data();
            let used = (0..DTMF_MEMS).filter(|&i| data.dtmf(i).is_some()).count();
            let mut r = Buf::<16>::new();
            let _ = uwrite!(r, "{} of {}", used, DTMF_MEMS);
            app.list_head(lcd, b"DTMF memories", r.as_bytes());
            // 9 rows on the page, scrolled to keep the selection in view
            let top = self.cur.saturating_sub(8);
            for k in 0..9u8 {
                let i = top + k;
                let mut name = Buf::<16>::new();
                let _ = uwrite!(name, "{} ", i + 1);
                name.push(data.dtmf_name(i).as_bytes());
                let mut v = Buf::<16>::new();
                match data.dtmf(i) {
                    Some(m) => m.text(&mut v),
                    None => v.set(b"-"),
                }
                app.row1(lcd, k as usize, name.as_bytes(), v.as_bytes(), false, i == self.cur, false);
            }
            app.help_box(lcd, 300 - 38, 34, b"OK opens one; # names it; * clears it.", b"MENU 0 8 6 2");
            return;
        }
        // Dialer: what's typed, large; the slot it goes out on; how A-D are typed.
        let mut t = Buf::<16>::new();
        self.digits.text(&mut t);
        let mut head = Buf::<24>::new();
        if self.cur != NONE {
            let _ = uwrite!(head, "Memory {} ", self.cur + 1);
            head.push(cx.store.data().dtmf_name(self.cur).as_bytes());
        } else {
            head.set(b"Dialer");
        }
        let mut right = Buf::<16>::new();
        let _ = uwrite!(right, "{}/16", self.digits.len);
        app.list_head(lcd, head.as_bytes(), right.as_bytes());
        if app.dirty(R_NOTE, hash(&t)) {
            lcd.fill(8, 60, 224, 60, BG);
            lcd.frame_r(8, 60, 224, 60, 3, if t.is_empty() { LINE } else { BLUE });
            let s: &[u8] = if t.is_empty() { b"type the digits" } else { t.as_bytes() };
            let (f, c) = if t.is_empty() { (&UI_12, DIM) } else { (&UI_15B, TEXT) };
            w::field(lcd, 12, 216, 70, 110, 96, Align::Center, s, f, c, BG);
        }
        let a = app.active;
        let mut slot = Buf::<32>::new();
        let _ = uwrite!(slot, "Sends on slot {} \u{b7} ", ["A", "B", "C"][a as usize % 3]);
        slot.push(w::freq3(app.slot(a).freq()).as_bytes());
        if app.dirty(R_LINE, hash(&slot)) {
            w::field(lcd, 8, 224, 128, 146, 142, Align::Left, slot.as_bytes(), &UI_12, SUB, BG);
        }
        app.help_box(lcd, 300 - 52, 48, b"Hold 1-4 for A-D. Hold MENU keeps it as a memory.", b"MENU 0 8 6 1");
    }

    fn hints(self, _: &App) -> Hints {
        if self.list {
            Hints::new(&[("OK", "Open"), ("#", "Name"), ("*", "Clear")])
        } else {
            Hints::new(&[("0-9 * #", "Type"), ("OK", "Send"), ("BACK", "Delete")])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols() {
        let d = Dtmf::parse(b"*79#ab").unwrap();
        let mut t = Buf::<16>::new();
        d.text(&mut t);
        assert_eq!(t.as_bytes(), b"*79#AB");
        assert!(Dtmf::parse(b"12x").is_none());
        assert!(Dtmf::parse(b"12345678901234567").is_none(), "17 is one too many");
    }
}
