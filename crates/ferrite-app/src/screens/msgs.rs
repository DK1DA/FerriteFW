//! APRS messages (MENU 0 5 2; manual chapter 25, APRS-6, APRS-8, APRS-13): the inbox and sent together,
//! newest first, sent ones with their state (C5); one message (C6); and compose (A16), typed with multi-tap
//! (KEY-12: digits, * case and digits, # space, BACK deletes). The draft lives with the APRS task's outgoing
//! message ([`crate::data::Draft`]), so typing costs the App no buffer. Nothing sends without a callsign
//! (APRS-8).

use super::lists::*;
use super::log::wrap;
use super::nav::Dest;
use super::opts::Of;
use super::shared::*;
use super::{Cx, Hints, View};
use crate::chan::ChannelStore;
use crate::data::{Draft, Message, MsgState, Op};
use crate::keys::{KeyEvent, Press};
use crate::{App, Screen};
use ferrite_core::keys::Key;
use ferrite_core::msg::LiveAll;
use ferrite_ui::assets::*;
use ferrite_ui::gfx::*;
use ferrite_ui::panel::Panel;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;
use ufmt::uwrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Msgs {
    pub cur: Cursor,
    /// The selected message's page (C6).
    pub open: bool,
}

const ALL: fn(u16) -> bool = |_| true;

fn state_text(m: &Message, b: &mut Buf<16>) -> u16 {
    b.clear();
    match m.state {
        MsgState::Unread => {
            b.set(b"new");
            BLUE
        }
        MsgState::Read => SUB,
        MsgState::Pending => {
            b.set(b"pending");
            SUB
        }
        MsgState::Acked => {
            b.set(b"acked");
            GREEN
        }
        MsgState::Retrying(n) => {
            let _ = uwrite!(b, "retry {}", n);
            AMBER
        }
        MsgState::Failed => {
            b.set(b"failed");
            RED
        }
    }
}

/// A draft addressed to `to` (empty: a new message), then compose.
fn compose(app: &mut App, store: &impl ChannelStore, to: &[u8]) {
    let d = store.data();
    let mut t = Buf::<9>::new();
    t.set(to);
    d.request(Op::Draft(Draft { to: t, ..d.draft() }));
    app.open(Dest::Compose);
}

impl View for Msgs {
    fn options(self, app: &mut App) -> bool {
        app.open_screen(Of::Messages.screen());
        true
    }

    fn name(self) -> &'static str {
        if self.open { "APRS message" } else { "APRS messages" }
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(if self.open { b"Message" } else { b"Messages" });
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let d = store.data();
        let m = d.message(self.cur.sel);
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Short) if !self.open && m.is_some() => {
                d.request(Op::MessageRead(self.cur.sel));
                app.screen = Screen::Msgs(Msgs { open: true, ..self });
            }
            (Key::Menu, Short) => {
                if let Some(m) = m {
                    compose(app, store, m.peer.as_bytes()); // reply
                }
            }
            (Key::Hash, Short) if !self.open => compose(app, store, b""),
            (Key::Exit, Short) if self.open => app.screen = Screen::Msgs(Msgs { open: false, ..self }),
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let cur = self.cur.step(d, store.data().messages(), ROWS2, &ALL);
        if self.open && cur.sel != self.cur.sel {
            store.data().request(Op::MessageRead(cur.sel));
        }
        app.screen = Screen::Msgs(Msgs { cur, ..self });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let data = cx.store.data();
        app.page(lcd, self.open);
        if !app.gate((self, data.version(), cx.dev.clock_min)) {
            return;
        }
        let row = |m: &Message, time: &mut Buf<12>, st: &mut Buf<16>| -> u16 {
            clock_text(cx.dev.clock_min, m.ago_s, time);
            state_text(m, st)
        };
        if self.open {
            if data.message(self.cur.sel).is_none() {
                app.empty(lcd, b"No such message", b"");
            }
            if let Some(m) = data.message(self.cur.sel) {
                let (mut time, mut st, mut age) = (Buf::<12>::new(), Buf::<16>::new(), Buf::<12>::new());
                let c = row(&m, &mut time, &mut st);
                age_text(m.ago_s, &mut age);
                let top = Row2 { tag: if m.inbound { b"From" } else { b"To" }, title: m.peer.as_bytes(), big: true, right: time.as_bytes(), sub: if m.inbound { b"Received" } else { b"Sent" }, ..Row2::default() };
                if app.dirty(R_TOP, hash(&top)) {
                    row2(lcd, 22, &top);
                }
                let mut b = Buf::<40>::new();
                // The clock time is in the header: here how long ago, and for a sent message its state.
                let y = app.kv(lcd, 0, 58, 0, &w::KV_DETAIL, b"Age", ferrite_ui::v!(b, "{} ago", Txt(age.as_bytes())), TEXT, false);
                let (l, v): (&[u8], &[u8]) = if m.inbound { (b"", b"") } else { (b"State", st.as_bytes()) };
                app.kv(lcd, 1, y, 0, &w::KV_DETAIL, l, v, c, false);
                if app.dirty(R_NOTE, hash(&m.text)) {
                    lcd.fill(0, 100, 240, 196, BG);
                    wrap(lcd, 8, 104, 224, 296, m.text.as_bytes(), TEXT);
                }
            }
            return;
        }
        let n = data.messages();
        let new = (0..n).filter(|&i| data.message(i).is_some_and(|m| m.state == MsgState::Unread)).count();
        let mut l = Buf::<24>::new();
        let _ = uwrite!(l, "{} {}", n, if n == 1 { "message" } else { "messages" });
        if new > 0 {
            let _ = uwrite!(l, " \u{b7} {} new", new);
        }
        app.list_head(lcd, l.as_bytes(), b"");
        let rows = self.cur.rows(n, ROWS2, &ALL, |r, i| {
            if let Some(m) = data.message(i) {
                let (mut time, mut st) = (Buf::<12>::new(), Buf::<16>::new());
                let c = row(&m, &mut time, &mut st);
                let r2 = Row2 {
                    tag: if m.inbound { b"From" } else { b"To" },
                    title: m.peer.as_bytes(),
                    big: true,
                    right: time.as_bytes(),
                    sub: m.text.as_bytes(),
                    sub_right: st.as_bytes(),
                    sub_c: if m.state == MsgState::Failed { c } else { 0 },
                    dot: m.state == MsgState::Unread,
                    selected: i == self.cur.sel,
                    ..Row2::default()
                };
                app.row2(lcd, r, &r2);
            }
        });
        app.rows_end(lcd, rows, ROWS2, ROW2_H);
        if n == 0 {
            app.empty(lcd, b"No messages", b"# writes one");
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.open {
            Hints::new(&[("OK", "Reply"), (ARROWS_UD, "Next"), ("BACK", "List")])
        } else {
            Hints::new(&[("OK", "Open"), ("#", "New"), ("Hold MENU", "More")])
        }
    }
}

// ---- compose (A16) ----

/// Multi-tap letters per digit (KEY-12); 1 has the punctuation.
const TAPS: [&[u8]; 10] = [b" 0", b".,?!-/@'1", b"abc2", b"def3", b"ghi4", b"jkl5", b"mno6", b"pqrs7", b"tuv8", b"wxyz9"];
/// The same key again within this many ms changes the last letter instead of adding one.
pub const TAP_MS: u32 = 1000;
/// APRS limits: an addressee of 9, a message of 67 characters.
const TO_MAX: usize = 9;
const TEXT_MAX: usize = 67;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Compose {
    /// 0 the recipient, 1 the text.
    pub field: u8,
    /// 0 lower case, 1 capitals, 2 digits (* cycles).
    pub case: u8,
    /// The last multi-tap key (its digit + 1, 0 none), how often it was tapped, and when (ms / 64).
    pub key: u8,
    pub taps: u8,
    pub at: u16,
}

/// Not decided yet: the first key or draw puts the cursor on the text when the draft has a recipient.
const UNSET: u8 = 255;

impl Compose {
    pub fn new() -> Self {
        Compose { field: UNSET, case: 0, key: 0, taps: 0, at: 0 }
    }

    fn resolved(self, store: &impl ChannelStore) -> Self {
        if self.field != UNSET {
            return self;
        }
        let to_set = !store.data().draft().to.is_empty();
        Compose { field: to_set as u8, case: if to_set { 0 } else { 1 }, ..self }
    }
}

impl View for Compose {
    fn name(self) -> &'static str {
        "Compose message"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Compose");
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let data = store.data();
        let mut d = data.draft();
        let mut s = self.resolved(store);
        let now = (app.now / 64) as u16;
        let to = s.field == 0;
        // the field being typed in, as one buffer
        let mut cur: Buf<67> = if to { Buf::from(d.to.as_bytes()) } else { d.text };
        let max = if to { TO_MAX } else { TEXT_MAX };
        match (e.key, e.press) {
            (Key::Digit(k), Short) => {
                let set = TAPS[k as usize];
                let ch = |taps: u8| {
                    let c = if s.case == 2 { b'0' + k } else { set[taps as usize % set.len()] };
                    if s.case == 1 || to { c.to_ascii_uppercase() } else { c }
                };
                // the recipient takes capitals, digits and "-" only
                let ok = |c: u8| !to || c.is_ascii_alphanumeric() || c == b'-';
                let again = s.key == k + 1 && now.wrapping_sub(s.at) < (TAP_MS / 64) as u16 && s.case != 2 && !cur.is_empty();
                s.taps = if again { s.taps.wrapping_add(1) } else { 0 };
                while !ok(ch(s.taps)) {
                    s.taps = s.taps.wrapping_add(1);
                }
                if again {
                    pop(&mut cur);
                    cur.push(&[ch(s.taps)]);
                } else if cur.len() < max {
                    cur.push(&[ch(s.taps)]);
                }
                s.key = k + 1;
                s.at = now;
            }
            (Key::Star, Short) => {
                s.case = (s.case + 1) % 3;
                s.key = 0;
            }
            (Key::Hash, Short) if !to && cur.len() < max => {
                cur.push(b" ");
                s.key = 0;
            }
            (Key::Exit, Short) => {
                if !cur.is_empty() {
                    pop(&mut cur);
                } else if to {
                    return app.back(); // the draft stays with the APRS task
                } else {
                    s.field = 0;
                    s.case = 1;
                }
                s.key = 0;
            }
            (Key::Down | Key::Up, Short) => {
                s.field ^= 1;
                s.key = 0;
                s.case = if s.field == 0 { 1 } else { 0 };
            }
            (Key::Menu, Short) if to => {
                s.field = 1;
                s.key = 0;
                s.case = 0;
            }
            (Key::Menu, Short) => {
                // APRS-8: nothing sends without a callsign.
                if data.callsign().is_none() {
                    return app.show_toast(b"Set a callsign first", b"MENU 0 8 7 1", false);
                }
                if d.to.is_empty() || d.text.is_empty() {
                    return app.show_toast(b"Nothing to send", if d.to.is_empty() { b"add a recipient" } else { b"write a message" }, false);
                }
                if data.request(Op::Send) {
                    app.back();
                    app.show_toast(b"Message queued", d.to.as_bytes(), false);
                } else {
                    app.show_toast(b"Not sent", b"try again", false);
                }
                return;
            }
            _ => return,
        }
        if to {
            d.to.set(cur.as_bytes());
        } else {
            d.text = cur;
        }
        data.request(Op::Draft(d));
        app.screen = Screen::Compose(s);
    }

    fn tick(self, app: &mut App, _: &LiveAll) {
        // The pending letter is kept once the tap time has passed: its underline goes.
        if self.key != 0 && ((app.now / 64) as u16).wrapping_sub(self.at) >= (TAP_MS / 64) as u16 {
            app.screen = Screen::Compose(Compose { key: 0, ..self });
        }
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        let d = cx.store.data().draft();
        let me = self.resolved(cx.store);
        if me != self {
            app.screen = Screen::Compose(me);
            return me.render(app, lcd, cx);
        }
        app.page(lcd, 0u8);
        let pending = self.key != 0;
        // the recipient
        if app.dirty(R_TOP, hash(&(d.to, self.field, pending && self.field == 0))) {
            field_box(lcd, 24, 34, b"To", d.to.as_bytes(), self.field == 0, pending && self.field == 0, b"callsign-SSID");
        }
        if app.dirty(R_ROW, hash(&(d.text, self.field, pending && self.field == 1))) {
            let (y, h) = (86, 120);
            lcd.fill(0, y, 240, h + 24, BG);
            lcd.text4(8, y + 12, b"Message", &UI_12, SUB, BG);
            let mut c = Buf::<12>::new();
            let _ = uwrite!(c, "{}/{}", d.text.len(), TEXT_MAX);
            let cw = text4_width(c.as_bytes(), &UI_12);
            lcd.text4(232 - cw, y + 12, c.as_bytes(), &UI_12, SUB, BG);
            let focus = self.field == 1;
            lcd.fill_r(8, y + 18, 224, h, 4, if focus { FOCUS } else { SURFACE });
            lcd.frame_r(8, y + 18, 224, h, 4, if focus { BLUE } else { LINE });
            let lines = wrap_in(lcd, 14, y + 22, 212, y + 18 + h - 4, d.text.as_bytes(), if focus { FOCUS } else { SURFACE });
            if focus {
                caret(lcd, d.text.as_bytes(), 14, y + 22, 212, lines, pending);
            }
        }
        let case = ["abc", "ABC", "123"][self.case as usize % 3];
        if app.dirty(R_NOTE, hash(&(case, cx.store.data().callsign()))) {
            let y = 236;
            lcd.fill(0, y, 240, 60, BG);
            let mut b = Buf::<40>::new();
            let _ = uwrite!(b, "Typing: {}", case);
            lcd.text4(8, y + 13, b.as_bytes(), &UI_12, SUB, BG);
            match cx.store.data().callsign() {
                Some(c) => {
                    let mut f = Buf::<40>::new();
                    let _ = uwrite!(f, "From {}", Txt(c.as_bytes()));
                    lcd.text4(8, y + 31, f.as_bytes(), &UI_12, SUB, BG);
                }
                None => {
                    lcd.text4(8, y + 31, b"No callsign: set one to send", &UI_12, AMBER, BG);
                }
            }
            lcd.text4(8, y + 49, b"1 . , ? !   0 space   # space", &UI_12, DIM, BG);
        }
    }

    fn hints(self, _: &App) -> Hints {
        if self.field == 0 {
            Hints::new(&[("OK", "Next"), ("*", "Case"), ("BACK", "Delete")])
        } else {
            Hints::new(&[("OK", "Send"), ("*", "Case"), ("BACK", "Delete")])
        }
    }
}

fn pop<const N: usize>(b: &mut Buf<N>) {
    let t = *b;
    b.set(&t.as_bytes()[..t.len().saturating_sub(1)]);
}

/// A one-line field: label, a box with the value, a hint when empty.
fn field_box(lcd: &mut impl Panel, y: i32, h: i32, label: &[u8], v: &[u8], focus: bool, pending: bool, empty: &[u8]) {
    lcd.fill(0, y, 240, h + 26, BG);
    lcd.text4(8, y + 12, label, &UI_12, SUB, BG);
    let bg = if focus { FOCUS } else { SURFACE };
    lcd.fill_r(8, y + 18, 224, h, 4, bg);
    lcd.frame_r(8, y + 18, 224, h, 4, if focus { BLUE } else { LINE });
    let base = y + 18 + h / 2 + 6;
    if v.is_empty() {
        lcd.text4(16, base, empty, &UI_12, DIM, bg);
        if focus {
            lcd.fill(16, base - 13, 1, 16, BLUE);
        }
    } else {
        let wv = lcd.text4(16, base - 1, v, &UI_15B, TEXT, bg);
        if focus {
            if pending {
                let lw = text4_width(&v[v.len() - 1..], &UI_15B);
                lcd.fill(16 + wv - lw, base + 2, lw, 2, BLUE);
            } else {
                lcd.fill(16 + wv + 1, base - 15, 1, 18, BLUE);
            }
        }
    }
}

/// Wrapped text on a box's colour; returns the lines used.
fn wrap_in(lcd: &mut impl Panel, x: i32, y: i32, w_: i32, y_end: i32, text: &[u8], bg: u16) -> i32 {
    let mut rest = text;
    let mut line = 0;
    while !rest.is_empty() && y + (line + 1) * 17 <= y_end {
        let cut = cut_line(rest, w_);
        lcd.text4(x, y + 13 + line * 17, &rest[..cut], &UI_12, TEXT, bg);
        rest = &rest[cut..];
        line += 1;
    }
    line
}

/// Where a wrapped line ends: after the last space that fits, or where it no longer fits.
fn cut_line(s: &[u8], w_: i32) -> usize {
    if text4_width(s, &UI_12) <= w_ {
        return s.len();
    }
    let fit = w::fit(s, &UI_12, w_).len().max(1);
    match s[..fit].iter().rposition(|&c| c == b' ') {
        Some(p) if p > 0 => p + 1,
        _ => fit,
    }
}

/// The caret after the text (an underline under a letter still being tapped).
fn caret(lcd: &mut impl Panel, text: &[u8], x: i32, y: i32, w_: i32, lines: i32, pending: bool) {
    // the last line's text
    let mut rest = text;
    let mut last: &[u8] = b"";
    while !rest.is_empty() {
        let cut = cut_line(rest, w_);
        last = &rest[..cut];
        rest = &rest[cut..];
    }
    let line = (lines - 1).max(0);
    let lw = text4_width(last, &UI_12);
    let base = y + 13 + line * 17;
    if pending && !last.is_empty() {
        lcd.fill(x + lw - 7, base + 2, 7, 2, BLUE);
    } else {
        lcd.fill(x + lw + 1, base - 12, 1, 15, BLUE);
    }
}
