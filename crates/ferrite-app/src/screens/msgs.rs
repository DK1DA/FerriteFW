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
const TAPS: [&[u8]; 10] = [b" 0", slow!(".,?!-/@'1").as_bytes(), b"abc2", b"def3", b"ghi4", b"jkl5", b"mno6", b"pqrs7", b"tuv8", b"wxyz9"];
/// The same key again within this many ms changes the last letter instead of adding one.
pub const TAP_MS: u32 = 1000;
/// APRS limits: an addressee of 9, a message of 67 characters.
const TO_MAX: usize = 9;
const TEXT_MAX: usize = 67;
/// Predictive: what 1 cycles through after a word (TEXT-2).
const PUNCT: &[u8] = b".,?!'/-";
/// The case of the next predicted word (TEXT-2): Capital, CAPS, lower.
const PCASES: [&str; 3] = ["Abc", "ABC", "abc"];
/// Compose's text is the draft's, unless a quick message is being edited (TEXT-7).
const DRAFT: u8 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Compose {
    /// 0 the recipient, 1 the text.
    pub field: u8,
    /// Multi-tap: 0 lower case, 1 capitals, 2 digits (* cycles).
    pub case: u8,
    /// The last multi-tap key (its digit + 1, 0 none), how often it was tapped, and when (ms / 64).
    pub key: u8,
    pub taps: u8,
    pub at: u16,
    /// Predictive (TEXT-2): the candidate picked for the keys typed (the matches best first, each followed by
    /// its short form), the case of the next word (index into [`PCASES`]), the punctuation mark + 1 when 1 was
    /// the last key, and the quick message highlighted on an empty message (TEXT-6). The keys typed for the
    /// word in progress are the App's text buffer, as digits.
    pub pick: u8,
    pub pcase: u8,
    pub punct: u8,
    pub quick: u8,
    /// [`DRAFT`], or the quick message being edited (TEXT-7).
    pub edit: u8,
    /// The quick list is on screen (set when drawn: the hint bar has no store to ask).
    pub list: bool,
    /// The cursor (TEXT-12): how many characters of the message are after it; 0 at the end. Kept as the count
    /// after it, it stays put while the text before it changes.
    pub tail: u8,
}

/// Not decided yet: the first key or draw puts the cursor on the text when the draft has a recipient.
const UNSET: u8 = 255;

impl Compose {
    pub fn new() -> Self {
        Compose { field: UNSET, case: 0, key: 0, taps: 0, at: 0, pick: 0, pcase: 0, punct: 0, quick: 0, edit: DRAFT, list: false, tail: 0 }
    }

    /// Quick message `i` in the editor (TEXT-7).
    pub fn quick_edit(i: u8) -> Self {
        Compose { field: 1, edit: i, ..Compose::new() }
    }

    fn resolved(self, store: &impl ChannelStore) -> Self {
        if self.field != UNSET {
            return self;
        }
        let d = store.data().draft();
        let to_set = !d.to.is_empty();
        // a message already begun goes on in lower case; a new one (or after . ? !) starts with a capital
        Compose { field: to_set as u8, case: if to_set { 0 } else { 1 }, pcase: next_case(d.text.as_bytes(), 2), ..self }
    }

    pub(crate) fn quick_edit_on(self) -> bool {
        (self.edit as usize) < crate::words::QUICK
    }
}

/// Quick message `i` (TEXT-7): the owner's, kept with the APRS store, or the factory text.
pub(crate) fn quick_text(store: &impl ChannelStore, i: usize) -> Buf<67> {
    store.data().quick(i).unwrap_or_else(|| Buf::from(crate::words::quick(i)))
}

/// The text being composed: the draft's, or the quick message being edited.
fn full_of(store: &impl ChannelStore, s: Compose) -> Buf<67> {
    if s.quick_edit_on() { quick_text(store, s.edit as usize) } else { store.data().draft().text }
}

/// The text before the cursor (TEXT-12): what typing and deleting work on.
fn text_of(store: &impl ChannelStore, s: Compose) -> Buf<67> {
    let t = full_of(store, s);
    Buf::from(&t.as_bytes()[..t.len() - (s.tail as usize).min(t.len())])
}

/// The text before the cursor back in, with what was after the cursor after it.
fn put_text(store: &impl ChannelStore, s: Compose, head: Buf<67>) {
    let full = full_of(store, s);
    let mut t = head;
    t.push(&full.as_bytes()[full.len() - (s.tail as usize).min(full.len())..]);
    let data = store.data();
    if s.quick_edit_on() {
        data.request(Op::Quick(s.edit, Some(t)));
    } else {
        data.request(Op::Draft(Draft { text: t, ..data.draft() }));
    }
}

/// A candidate: the word, whether it is shown in capitals, and how many bytes before it a phrase's short form
/// replaces (0 for a word).
type Cand = (Buf<16>, bool, usize);

/// Candidate `i` for `keys` after `head` (TEXT-2, TEXT-4): the dictionary's matches best first, each followed by
/// its short form, and by a phrase's when the words before it in `head` complete one (see you + later → CUL).
fn candidate(keys: &[u8], head: &[u8], i: usize) -> Option<Cand> {
    let mut k = 0;
    for n in 0.. {
        let w = crate::words::nth(keys, n)?;
        let short = crate::words::short(w.text.as_bytes()).map(|s| (s, 0));
        let phrase = crate::words::phrase(head, w.text.as_bytes());
        for c in [Some((w.text, w.caps, 0)), short.map(|(s, d)| (Buf::from(s), true, d)), phrase.map(|(s, d)| (Buf::from(s), true, d))].into_iter().flatten() {
            if k == i {
                return Some(c);
            }
            k += 1;
        }
    }
    None
}

fn candidates(keys: &[u8], head: &[u8]) -> usize {
    (0..).take_while(|&i| candidate(keys, head, i).is_some()).count()
}

/// A word as it goes in: capitals for Q-codes and shorthand, else the case chosen (Capital, CAPS, lower).
fn cased(w: &[u8], caps: bool, pcase: u8) -> Buf<16> {
    let mut b = Buf::<16>::from(w);
    let mut t = [0u8; 16];
    for (i, (o, &c)) in t.iter_mut().zip(b.as_bytes()).enumerate() {
        *o = if caps || pcase == 1 || (pcase == 0 && i == 0) { c.to_ascii_uppercase() } else { c };
    }
    let n = b.len();
    b.set(&t[..n]);
    b
}

/// What the word in progress shows as: the picked candidate, or the keys as digits when nothing matches; and
/// how many bytes of `head` it replaces (a phrase's short form).
fn pending(keys: &[u8], head: &[u8], s: Compose) -> (Buf<16>, bool, usize) {
    match candidate(keys, head, s.pick as usize) {
        Some((w, caps, drop)) => (cased(w.as_bytes(), caps, s.pcase), true, drop),
        None => (Buf::from(keys), false, 0),
    }
}

/// After a word or a mark: the next word's case. Capital starts a message and follows . ? !; CAPS stays.
fn next_case(t: &[u8], pcase: u8) -> u8 {
    let end = t.iter().rev().find(|&&c| c != b' ');
    match (pcase, end) {
        (1, _) => 1,
        (_, None | Some(b'.' | b'?' | b'!')) => 0,
        _ => 2,
    }
}

/// The 67-character limit (TEXT-5): a short beep, and why in a toast.
fn refuse_full(app: &mut App) {
    app.beep_refused();
    app.show_toast(slow!("67 characters").as_bytes(), slow!("the APRS limit").as_bytes(), false);
}

/// Room before the cursor: the 67 characters less what is after it.
fn room(s: Compose) -> usize {
    TEXT_MAX.saturating_sub(s.tail as usize)
}

/// The word in progress goes into the text (no space); false when it can't (no word, or no room).
fn accept(app: &mut App, s: &mut Compose, t: &mut Buf<67>) -> bool {
    if app.text.is_empty() {
        return true;
    }
    let keys = app.text;
    let (w, ok, drop) = pending(keys.as_bytes(), t.as_bytes(), *s);
    if !ok {
        app.show_toast(b"No word", slow!("hold * to spell it").as_bytes(), false); // TEXT-3
        return false;
    }
    if t.len() - drop + w.len() > room(*s) {
        refuse_full(app);
        return false;
    }
    let head = *t;
    t.set(&head.as_bytes()[..head.len() - drop]);
    t.push(w.as_bytes());
    app.text.clear();
    s.pick = 0;
    s.pcase = next_case(t.as_bytes(), if s.pcase == 0 { 2 } else { s.pcase });
    true
}

impl Compose {
    /// Predictive keys on the message text (TEXT-2 to TEXT-6). True when the key was taken.
    fn pred_key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) -> bool {
        use Press::*;
        let mut s = self;
        let mut t = text_of(store, s);
        let keys = app.text;
        let quick_list = t.is_empty() && s.tail == 0 && keys.is_empty() && !s.quick_edit_on();
        match (e.key, e.press) {
            (Key::Digit(d @ 2..=9), Short) => {
                if t.len() + keys.len() + 1 > room(s) || keys.len() >= crate::words::WORD_MAX {
                    refuse_full(app);
                    return true;
                }
                app.text.push(&[b'0' + d]);
                s.pick = 0;
                s.punct = 0;
            }
            // hold a digit: the digit itself (73, 146.52)
            (Key::Digit(d), Long) => {
                if !accept(app, &mut s, &mut t) {
                    return true;
                }
                if t.len() + 1 > room(s) {
                    refuse_full(app);
                    return true;
                }
                t.push(&[b'0' + d]);
                s.punct = 0;
            }
            // 1: punctuation after a word, again for the next mark
            (Key::Digit(1), Short) => {
                if s.punct > 0 && keys.is_empty() {
                    let i = s.punct as usize % PUNCT.len();
                    pop(&mut t);
                    t.push(&PUNCT[i..i + 1]);
                    s.punct = i as u8 + 1;
                } else {
                    if !accept(app, &mut s, &mut t) {
                        return true;
                    }
                    if t.len() + 1 > room(s) {
                        refuse_full(app);
                        return true;
                    }
                    t.push(&PUNCT[..1]);
                    s.punct = 1;
                }
                s.pcase = next_case(t.as_bytes(), s.pcase);
            }
            // # (and 0): the word, then a space
            (Key::Hash | Key::Digit(0), Short) => {
                if !accept(app, &mut s, &mut t) {
                    return true;
                }
                if !t.is_empty() && t.len() < room(s) {
                    t.push(b" ");
                }
                s.punct = 0;
                s.pcase = next_case(t.as_bytes(), s.pcase);
            }
            (Key::Star, Short) => s.pcase = (s.pcase + 1) % 3,
            (Key::Up | Key::Down, Short | Repeat) if !keys.is_empty() || quick_list => {
                let d = if e.key == Key::Up { -1 } else { 1 };
                return self.pred_knob(app, d, store);
            }
            (Key::Exit, Short) if !keys.is_empty() => {
                let mut k = keys;
                pop(&mut k);
                app.text = k;
                s.pick = 0;
            }
            (Key::Menu, Short) if quick_list => {
                // TEXT-6: the quick message goes in; OK again sends
                t = quick_text(store, s.quick as usize);
                s.pcase = next_case(t.as_bytes(), 2);
            }
            (Key::Menu, Short) if !keys.is_empty() => {
                if !accept(app, &mut s, &mut t) {
                    return true;
                }
                put_text(store, s, t);
                app.screen = Screen::Compose(s);
                return false; // OK goes on to send (or keep the quick message)
            }
            _ => return false,
        }
        put_text(store, s, t);
        app.screen = Screen::Compose(s);
        true
    }

    /// The knob or ▲▼ in predictive entry: through the candidates, or the quick list (TEXT-2, TEXT-6).
    fn pred_knob(self, app: &mut App, d: i32, store: &impl ChannelStore) -> bool {
        let keys = app.text;
        if !keys.is_empty() {
            let n = candidates(keys.as_bytes(), text_of(store, self).as_bytes()).max(1) as i32;
            let pick = (self.pick as i32 + d).rem_euclid(n) as u8;
            app.screen = Screen::Compose(Compose { pick, ..self });
            return true;
        }
        if full_of(store, self).is_empty() && !self.quick_edit_on() {
            let quick = (self.quick as i32 + d).rem_euclid(crate::words::QUICK as i32) as u8;
            app.screen = Screen::Compose(Compose { quick, ..self });
            return true;
        }
        false
    }
}

/// OK on the text, or PTT in compose (APRS-18): the message goes to the APRS task's queue, a word being
/// predicted taken as it stands.
pub(crate) fn send(app: &mut App, store: &impl ChannelStore) {
    let Screen::Compose(c) = app.screen else { return };
    let mut s = c.resolved(store);
    if s.field == 1 && app.set.predictive && !app.text.is_empty() {
        let mut t = text_of(store, s);
        if !accept(app, &mut s, &mut t) {
            return; // it said why
        }
        put_text(store, s, t);
        app.screen = Screen::Compose(Compose { key: 0, pick: 0, punct: 0, ..s });
    }
    let data = store.data();
    let d = data.draft();
    // APRS-8: nothing sends without a callsign.
    if data.callsign().is_none() {
        return app.show_toast(slow!("Set a callsign first").as_bytes(), slow!("MENU 0 8 7 1").as_bytes(), false);
    }
    let text = full_of(store, s);
    if d.to.is_empty() || text.is_empty() {
        return app.show_toast(slow!("Nothing to send").as_bytes(), if d.to.is_empty() { slow!("add a recipient").as_bytes() } else { slow!("write a message").as_bytes() }, false);
    }
    if data.request(Op::Send) {
        app.back();
        app.show_toast(slow!("Message queued").as_bytes(), d.to.as_bytes(), false);
    } else {
        app.show_toast(slow!("Not sent").as_bytes(), slow!("try again").as_bytes(), false);
    }
}

impl View for Compose {
    fn name(self) -> &'static str {
        if self.quick_edit_on() { "Quick message" } else { "Compose message" }
    }

    /// ◀▶ move the cursor in the message (TEXT-12); ▲▼ and the knob stay on the words.
    fn own_arrows(self) -> bool {
        true
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(if self.quick_edit_on() { b"Quick" } else { b"Compose" });
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let mut s = self.resolved(store);
        // TEXT-10: hold * switches predictive and multi-tap for message text; the choice is kept.
        if s.field == 1 && (e.key, e.press) == (Key::Star, Long) {
            let mut t = text_of(store, s);
            if app.set.predictive && !accept(app, &mut s, &mut t) {
                app.text.clear();
            }
            put_text(store, s, t);
            app.set.predictive = !app.set.predictive;
            app.text.clear();
            app.show_toast(if app.set.predictive { slow!("Predictive").as_bytes() } else { slow!("Multi-tap").as_bytes() }, slow!("hold * to switch").as_bytes(), false);
            app.screen = Screen::Compose(Compose { key: 0, pick: 0, punct: 0, ..s });
            return;
        }
        // TEXT-12: ◀▶ move the cursor in the message; a word being predicted goes in first.
        if s.field == 1 && matches!((e.key, e.press), (Key::Left | Key::Right, Short | Repeat)) {
            let mut t = text_of(store, s);
            if app.set.predictive && !accept(app, &mut s, &mut t) {
                return;
            }
            put_text(store, s, t);
            let n = full_of(store, s).len() as u8;
            s.tail = if e.key == Key::Left { (s.tail + 1).min(n) } else { s.tail.saturating_sub(1) };
            app.screen = Screen::Compose(Compose { key: 0, pick: 0, punct: 0, ..s });
            return;
        }
        if s.field == 1 && app.set.predictive && s.pred_key(app, e, store) {
            return;
        }
        if let Screen::Compose(c) = app.screen {
            s = c.resolved(store); // pred_key may have stored the accepted word
        }
        let data = store.data();
        let mut d = data.draft();
        let now = (app.now / 64) as u16;
        let to = s.field == 0;
        // the field being typed in, as one buffer
        let mut cur: Buf<67> = if to { Buf::from(d.to.as_bytes()) } else { text_of(store, s) };
        let max = if to { TO_MAX } else { room(s) };
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
                } else if !to {
                    refuse_full(app);
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
                } else if !to && s.tail > 0 {
                    // at the start with text after the cursor: nothing before it to delete
                } else if to || s.quick_edit_on() {
                    return app.back(); // the draft stays with the APRS task
                } else {
                    s.field = 0;
                    s.case = 1;
                }
                s.key = 0;
            }
            (Key::Down | Key::Up, Short) if !s.quick_edit_on() => {
                s.field ^= 1;
                (s.key, s.tail) = (0, 0);
                s.case = if s.field == 0 { 1 } else { 0 };
            }
            (Key::Menu, Short) if to => {
                s.field = 1;
                (s.key, s.tail) = (0, 0);
                s.case = 0;
            }
            (Key::Menu, Short) if s.quick_edit_on() => {
                app.back();
                return app.show_toast(slow!("Quick message").as_bytes(), b"kept", false);
            }
            (Key::Menu, Short) => return send(app, store),
            _ => return,
        }
        if to {
            d.to.set(cur.as_bytes());
            data.request(Op::Draft(d));
        } else {
            put_text(store, s, cur);
        }
        app.screen = Screen::Compose(s);
    }

    fn knob(self, app: &mut App, d: i32, store: &impl ChannelStore) {
        let s = self.resolved(store);
        if s.field == 1 && app.set.predictive {
            s.pred_knob(app, d, store);
        }
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
        let pred = app.set.predictive;
        let keys = app.text;
        let head = text_of(cx.store, self);
        let full = full_of(cx.store, self);
        let (word, matched, drop) = if pred && !keys.is_empty() { pending(keys.as_bytes(), head.as_bytes(), self) } else { (Buf::new(), true, 0) };
        // a phrase's short form shows in place of the words it replaces (TEXT-4)
        let text = Buf::<67>::from(&head.as_bytes()[..head.len() - drop]);
        let quick_list = pred && self.field == 1 && full.is_empty() && keys.is_empty() && !self.quick_edit_on();
        if quick_list != self.list {
            app.screen = Screen::Compose(Compose { list: quick_list, ..self });
        }
        let pending_tap = self.key != 0;
        // the recipient, or which quick message this is
        if app.dirty(R_TOP, hash(&(d.to, self.field, pending_tap && self.field == 0, self.edit))) {
            if self.quick_edit_on() {
                lcd.fill(0, 24, 240, 60, BG);
                let mut b = Buf::<40>::new();
                let _ = uwrite!(b, "Quick message {}", self.edit + 1);
                lcd.text4(8, 36, b.as_bytes(), &UI_12B, TEXT, BG);
                lcd.text4(8, 56, slow!("# in the list: the factory text again").as_bytes(), &UI_12, SUB, BG);
            } else {
                field_box(lcd, 24, 34, b"To", d.to.as_bytes(), self.field == 0, pending_tap && self.field == 0, slow!("callsign-SSID").as_bytes());
            }
        }
        if app.dirty(R_ROW, hash(&(full, self.tail, word, self.field, pending_tap && self.field == 1, quick_list, quick_list.then_some(self.quick)))) {
            let (y, h) = (86, 120);
            lcd.fill(0, y, 240, h + 24, BG);
            lcd.text4(8, y + 12, if quick_list { slow!("Quick messages").as_bytes() } else { b"Message".as_slice() }, &UI_12, SUB, BG);
            let mut c = Buf::<12>::new();
            let _ = uwrite!(c, "{}/{}", full.len() - drop + word.len(), TEXT_MAX); // TEXT-5
            let cw = text4_width(c.as_bytes(), &UI_12);
            lcd.text4(232 - cw, y + 12, c.as_bytes(), &UI_12, SUB, BG);
            let focus = self.field == 1;
            let bg = if focus { FOCUS } else { SURFACE };
            lcd.fill_r(8, y + 18, 224, h, 2, bg);
            lcd.frame_r(8, y + 18, 224, h, 2, if focus { BLUE } else { LINE });
            if quick_list {
                quick_rows(cx.store, lcd, y + 20, self.quick as usize, bg);
            } else {
                let mut all = Buf::<84>::from(text.as_bytes());
                all.push(word.as_bytes());
                let at = all.len();
                all.push(&full.as_bytes()[head.len().min(full.len())..]);
                wrap_in(lcd, 14, y + 22, 212, y + 18 + h - 4, all.as_bytes(), bg);
                if focus {
                    let under = if !word.is_empty() { text4_width(word.as_bytes(), &UI_12) } else if pending_tap { 7 } else { 0 };
                    caret(lcd, all.as_bytes(), at, 14, y + 22, 212, under, if matched { BLUE } else { AMBER });
                }
            }
        }
        let callsign = cx.store.data().callsign();
        let strip = (pred, self.field, keys, self.pick, self.pcase, self.case, callsign, quick_list, head);
        if app.dirty(R_NOTE, hash(&strip)) {
            let y = 236;
            lcd.fill(0, y, 240, 60, BG);
            if pred && self.field == 1 && !keys.is_empty() {
                candidate_strip(lcd, y + 2, keys.as_bytes(), head.as_bytes(), self);
            } else {
                let mut b = Buf::<40>::new();
                if pred && self.field == 1 {
                    let _ = uwrite!(b, "Words: {}", PCASES[self.pcase as usize % 3]);
                } else {
                    let _ = uwrite!(b, "Typing: {}", ["abc", "ABC", "123"][self.case as usize % 3]);
                }
                lcd.text4(8, y + 13, b.as_bytes(), &UI_12, SUB, BG);
            }
            match callsign {
                Some(c) if !self.quick_edit_on() => {
                    let mut f = Buf::<40>::new();
                    let _ = uwrite!(f, "From {}", Txt(c.as_bytes()));
                    lcd.text4(8, y + 37, f.as_bytes(), &UI_12, SUB, BG);
                }
                None if !self.quick_edit_on() => {
                    lcd.text4(8, y + 37, slow!("No callsign: set one to send").as_bytes(), &UI_12, AMBER, BG);
                }
                _ => {}
            }
            let help: &[u8] = if pred && self.field == 1 { slow!("1 marks   hold a digit: digit").as_bytes() } else { slow!("1 . , ? !   0 space   # space").as_bytes() };
            lcd.text4(8, y + 55, help, &UI_12, DIM, BG);
        }
    }

    fn hints(self, app: &App) -> Hints {
        let pred = app.set.predictive && self.field == 1;
        if self.field == 0 {
            Hints::new(&[("OK", "Next"), ("*", "Case"), ("BACK", "Delete")])
        } else if pred && !app.text.is_empty() {
            Hints::new(&[("Knob", "Word"), ("#", "Space"), ("Hold *", "Spell")]) // TEXT-11
        } else if pred && self.list {
            Hints::new(&[("Knob", "Pick"), ("OK", "Insert"), ("BACK", "To")]) // TEXT-6
        } else if self.quick_edit_on() {
            Hints::new(&[("OK", "Keep"), ("Hold *", if pred { "Spell" } else { "Words" }), ("BACK", "Delete")])
        } else if pred {
            Hints::new(&[("OK", "Send"), ("#", "Space"), ("Hold *", "Spell")])
        } else {
            Hints::new(&[("OK", "Send"), ("*", "Case"), ("Hold *", "Words")])
        }
    }
}

/// The quick messages on an empty message (TEXT-6): the highlighted one in blue, scrolled to keep it in view.
fn quick_rows(store: &impl ChannelStore, lcd: &mut impl Panel, y: i32, sel: usize, bg: u16) {
    const VIS: usize = 6;
    let top = sel.saturating_sub(VIS - 1).min(crate::words::QUICK - VIS);
    for (k, i) in (top..crate::words::QUICK).take(VIS).enumerate() {
        let ry = y + 2 + k as i32 * 19;
        let on = i == sel;
        let (fg, rb) = if on { (BLUE_FG, BLUE) } else { (TEXT, bg) };
        if on {
            lcd.fill_r(10, ry, 220, 19, 1, BLUE);
        }
        let mut n = Buf::<4>::new();
        let _ = uwrite!(n, "{}", i + 1);
        lcd.text4(16, ry + 14, n.as_bytes(), &UI_12, if on { BLUE_FG } else { SUB }, rb);
        lcd.text4(32, ry + 14, w::fit(quick_text(store, i).as_bytes(), &UI_12, 192), &UI_12, fg, rb);
    }
}

/// The candidate strip (TEXT-11): three at a time, the picked one selected, and where it is in the list; or
/// "no word" (TEXT-3).
fn candidate_strip(lcd: &mut impl Panel, y: i32, keys: &[u8], head: &[u8], s: Compose) {
    let n = candidates(keys, head);
    if n == 0 {
        lcd.text4(8, y + 11, slow!("No word: hold * to spell it").as_bytes(), &UI_12, AMBER, BG);
        return;
    }
    let from = s.pick as usize / 3 * 3;
    let mut words = [Buf::<16>::new(); 3];
    let mut k = 0;
    for (i, w) in words.iter_mut().enumerate() {
        if let Some((c, caps, _)) = candidate(keys, head, from + i) {
            *w = cased(c.as_bytes(), caps, s.pcase);
            k += 1;
        }
    }
    let items: [&[u8]; 3] = [words[0].as_bytes(), words[1].as_bytes(), words[2].as_bytes()];
    w::chips_sel(lcd, 8, y, 184, &items[..k], Some(s.pick as usize - from), BG);
    let mut c = Buf::<12>::new();
    let _ = uwrite!(c, "{}/{}", s.pick as usize + 1, n);
    let cw = text4_width(c.as_bytes(), &UI_12);
    lcd.text4(232 - cw, y + 13, c.as_bytes(), &UI_12, SUB, BG);
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
    lcd.fill_r(8, y + 18, 224, h, 2, bg);
    lcd.frame_r(8, y + 18, 224, h, 2, if focus { BLUE } else { LINE });
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

/// The caret at `at` in the wrapped text (TEXT-12; the end when nothing is after the cursor): an underline under
/// the `under` pixels before it (a letter still being tapped, or the predicted word in progress), else a bar.
fn caret(lcd: &mut impl Panel, text: &[u8], at: usize, x: i32, y: i32, w_: i32, under: i32, c: u16) {
    // the line the cursor is on, and its text up to the cursor
    let (mut rest, mut start, mut line) = (text, 0, 0);
    let mut last: &[u8] = b"";
    while !rest.is_empty() {
        let cut = cut_line(rest, w_);
        last = &text[start..at.clamp(start, start + cut)];
        if at < start + cut || rest.len() == cut {
            break;
        }
        (rest, start, line) = (&rest[cut..], start + cut, line + 1);
    }
    let lw = text4_width(last, &UI_12);
    let base = y + 13 + line * 17;
    if under > 0 && !last.is_empty() {
        lcd.fill(x + lw - under.min(lw), base + 2, under.min(lw), 2, c);
    } else {
        lcd.fill(x + lw + 1, base - 12, 1, 15, BLUE);
    }
}

// ---- quick messages (TEXT-7) ----

/// The nine quick messages, from the Messages screen's options: OK edits one, # puts the factory text back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QuickMsgs {
    pub cur: Cursor,
}

impl View for QuickMsgs {
    fn name(self) -> &'static str {
        "Quick messages"
    }

    fn title(self, _: &App, b: &mut Buf<10>) {
        b.set(b"Quick");
    }

    fn key(self, app: &mut App, e: KeyEvent, store: &impl ChannelStore) {
        use Press::*;
        let i = self.cur.sel as usize;
        match (e.key, e.press) {
            (Key::Up | Key::Down, Short | Repeat) => self.knob(app, if e.key == Key::Up { -1 } else { 1 }, store),
            (Key::Menu, Short) => {
                app.text.clear();
                app.open_screen(Screen::Compose(Compose::quick_edit(i as u8)));
            }
            (Key::Hash, Short) => {
                store.data().request(Op::Quick(i as u8, None));
                app.show_toast(slow!("Factory text").as_bytes(), crate::words::quick(i), false);
            }
            (Key::Exit, Short) => app.back(),
            _ => {}
        }
    }

    fn knob(self, app: &mut App, d: i32, _: &impl ChannelStore) {
        let cur = self.cur.step(d, crate::words::QUICK as u16, ROWS2, &ALL);
        app.screen = Screen::QuickMsgs(QuickMsgs { cur });
    }

    fn render(self, app: &mut App, lcd: &mut impl Panel, cx: &Cx<impl ChannelStore>) {
        app.page(lcd, 0u8);
        if !app.gate((self, cx.store.data().version())) {
            return;
        }
        app.list_head(lcd, slow!("Shown on an empty message").as_bytes(), b"");
        let rows = self.cur.rows(crate::words::QUICK as u16, ROWS2, &ALL, |r, i| {
            let mut n = Buf::<4>::new();
            let _ = uwrite!(n, "{}", i + 1);
            let own = cx.store.data().quick(i as usize).is_some();
            let q = quick_text(cx.store, i as usize);
            let r2 = Row2 {
                tag: n.as_bytes(),
                title: q.as_bytes(),
                sub: if own { b"yours" } else { b"factory" },
                selected: i == self.cur.sel,
                ..Row2::default()
            };
            app.row2(lcd, r, &r2);
        });
        app.rows_end(lcd, rows, ROWS2, ROW2_H);
    }

    fn hints(self, _: &App) -> Hints {
        Hints::new(&[("OK", "Edit"), ("#", "Factory"), ("BACK", "Back")])
    }
}
