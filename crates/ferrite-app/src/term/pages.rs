//! The full-screen VT100 pages (TERM-11 to TERM-20; the `vt100` cargo feature, decision 346): what the
//! firmware does around the program in the template part ([`super::tpl`]): the keys decoded and a value typed, the
//! rows drawn and sent only when they changed, a change to several channels stepped on the poll. Without the
//! feature, or without the part, the terminal is a plain command line (`term.rs`, TERM-47).

use super::*;
pub(super) use crate::chan::{Channel, ZoneId, FLAG_APRS, FLAG_HIDDEN, FLAG_RX_ONLY};
pub(super) use crate::chfield::ChField as F;
pub(super) use crate::slot::Item;
use tpl::{NPAGES, V_BUSY, V_CHAIN, V_HELP, V_PAGE};

/// The channel editor's fields, in order (the radio's editor: EDIT-2, CHAN-4), with their rules in [`crate::chfield`].
pub(super) const FIELDS: [F; 16] = [
    F::Name,
    F::Zone,
    F::Rx,
    F::It(Item::Mode),
    F::It(Item::RxTone),
    F::It(Item::Tones),
    F::It(Item::Offset),
    F::Tx,
    F::It(Item::Power),
    F::It(Item::Squelch),
    F::It(Item::BusyLock),
    F::Scan,
    F::Flag(FLAG_APRS),
    F::Flag(FLAG_RX_ONLY),
    F::Flag(FLAG_HIDDEN),
    F::Home,
];

/// The channel list's "every channel" filter.
pub(super) const ALL: ZoneId = 0xFFFD;
/// The program's pages, by number (`pages.vt` lists them in this order).
const PAGES: [Page; 10] = [Page::Splash, Page::Main, Page::Channels, Page::Edit, Page::Zones, Page::Settings, Page::Monitor, Page::Mail, Page::Tnc, Page::About];

pub(super) fn zone_name<S: ChannelStore>(z: ZoneId, st: &S) -> Buf<16> {
    if z == ALL {
        return Buf::from(b"All channels");
    }
    st.zone_label(z, b"No zone", b"?")
}

pub(super) fn list_len<S: ChannelStore>(z: ZoneId, st: &S) -> u16 {
    if z == ALL { st.count() } else { st.zone_len(z) }
}

pub(super) fn chan_at<S: ChannelStore>(z: ZoneId, i: u16, st: &S) -> Option<Channel> {
    if z == ALL { st.by_number(i) } else { st.zone_at(z, i) }
}

// ---- Writing a row ----

pub(super) struct Cx<'a, S> {
    pub(super) app: &'a mut App,
    pub(super) store: &'a S,
    pub(super) dev: &'a Device,
    pub(super) live: &'a LiveAll,
}

/// Where a row's bytes go: the hash that tells whether it changed, the sink, or nowhere (a key's run).
pub(super) trait Out {
    fn out(&mut self, b: &[u8]);
}

/// FNV-1a over the row's bytes, attributes included.
pub(super) struct Hash(u32);

impl Out for Hash {
    fn out(&mut self, b: &[u8]) {
        for &c in b {
            self.0 = (self.0 ^ c as u32).wrapping_mul(0x0100_0193);
        }
    }
}

pub(super) struct SinkOut<'a, T: Sink>(&'a mut T);

impl<T: Sink> Out for SinkOut<'_, T> {
    fn out(&mut self, b: &[u8]) {
        self.0.put(b);
    }
}

struct Null;

impl Out for Null {
    fn out(&mut self, _: &[u8]) {}
}

/// One row: text clipped to the width, columns counted (escape sequences, the program's attributes, take none).
/// While capturing, what is written goes to a message instead (the program's notes, and what it compares with a
/// typed line).
pub(super) struct W<'a> {
    o: &'a mut dyn Out,
    col: u16,
    cols: u16,
    /// Capturing: what was written, and the column and width to go back to.
    capt: Option<cmd::Msg>,
    was: (u16, u16),
}

impl<'a> W<'a> {
    fn new(o: &'a mut dyn Out, cols: u8) -> Self {
        W { o, col: 0, cols: cols as u16, capt: None, was: (0, 0) }
    }

    pub(super) fn put(&mut self, b: &[u8]) {
        match &mut self.capt {
            Some(m) => m.push(b),
            None => self.o.out(b),
        }
    }

    /// Capturing on, or off: what was written since it went on.
    #[inline(never)]
    pub(super) fn cap(&mut self, on: bool) -> cmd::Msg {
        let m = match self.capt.take() {
            Some(m) => {
                (self.col, self.cols) = self.was;
                m
            }
            None => Buf::new(),
        };
        if on {
            self.was = (self.col, self.cols);
            (self.col, self.cols) = (0, u16::MAX);
            self.capt = Some(Buf::new());
        }
        m
    }

    /// Text, printable ASCII; anything else shows as `?` ("·" as a dot).
    pub(super) fn s(&mut self, b: &[u8]) {
        let mut i = 0;
        while i < b.len() && self.col < self.cols {
            let c = b[i];
            let (c, skip) = match c {
                0x20..=0x7E => (c, 1),
                0xC2 if b.get(i + 1) == Some(&0xB7) => (b'.', 2), // U+00B7, the UI's middle dot
                0xB7 => (b'.', 1),
                _ => (b'?', 1),
            };
            self.put(&[c]);
            self.col += 1;
            i += skip;
        }
    }

    /// A number in decimal.
    pub(super) fn n(&mut self, n: u32) {
        self.s(digits(n, &mut [0; 10]));
    }

    /// Spaces up to column `c`.
    pub(super) fn at(&mut self, c: u16) {
        while self.col < c.min(self.cols) {
            self.put(b" ");
            self.col += 1;
        }
    }
}

impl Term {
    /// What it shows now ([`Page::Off`] until it has been started).
    pub fn page(&self) -> Page {
        if self.vt && self.page != Page::Off { PAGES[self.pg.v[V_PAGE] as usize % PAGES.len()] } else { self.page }
    }

    /// A key on the page (after the console, the command line, Ctrl-C and Ctrl-L): a value being typed takes every
    /// key until Enter or Esc; the program has the rest.
    pub(super) fn page_key<S: ChannelStore>(&mut self, app: &mut App, k: K, store: &S, dev: &Device, live: &LiveAll) {
        let p = &mut self.pg;
        let code = if p.typing {
            match k {
                K::Enter => 0xA0,
                K::Esc => 0xA1,
                K::Bs => {
                    p.line = Buf::from(&p.line.as_bytes()[..p.line.len().saturating_sub(1)]);
                    return;
                }
                K::Ch(c) if p.line.len() < p.lmax as usize => return p.line.push(&[c]),
                _ => return,
            }
        } else {
            match k {
                K::Ch(c) => c.to_ascii_lowercase(),
                K::Up => 0x80,
                K::Down => 0x81,
                K::Left => 0x82,
                K::Right => 0x83,
                K::PgUp => 0x84,
                K::PgDn => 0x85,
                K::Home => 0x86,
                K::End => 0x87,
                K::Enter => 0x88,
                K::Esc => 0x89,
                K::Bs => 0x8B,
                K::Pf(n) => 0x90 + n,
                _ => return,
            }
        };
        p.typing &= code < 0xA0;
        self.run(tpl::E_KEY, code as i32, &mut Cx { app, store, dev, live });
    }

    /// Runs one of the lib's entries with the lib and this page read from the part; then, when it asked for
    /// another segment (`!chain`: the channel list's tools), the same entry there.
    #[inline(never)]
    fn run<S: ChannelStore>(&mut self, entry: usize, arg: i32, cx: &mut Cx<S>) {
        let mut buf = [0u8; tpl::PACK_MAX];
        let mut seg = self.pg.v[V_PAGE];
        for _ in 0..2 {
            self.pg.v[V_CHAIN] = -1;
            let Some((code, lib)) = tpl::load(&mut buf, seg) else { return };
            self.vm(code, lib, entry, arg, cx, &mut W::new(&mut Null, self.cols));
            seg = self.pg.v[V_CHAIN];
            if seg < 0 {
                return;
            }
        }
    }

    /// Several channels changing (TERM-14): the program's poll step, one write each time the store has taken the last.
    pub(super) fn bulk_on(&self) -> bool {
        self.pg.v[V_BUSY] != 0
    }

    /// Whether the help shows (TERM-16).
    #[cfg(test)]
    pub(super) fn help(&self) -> bool {
        self.pg.v[V_HELP] != 0
    }

    /// The pages' part of a poll: the screen cleared, a change to several channels stepped; false when the sink is
    /// full.
    pub(super) fn page_poll<S: ChannelStore>(&mut self, app: &mut App, store: &S, dev: &Device, live: &LiveAll, out: &mut impl Sink) -> bool {
        if self.clear && self.con == Con::Off {
            if out.room() < 16 {
                return false;
            }
            out.put(b"\x1b[0m\x1b[2J");
            self.clear = false;
        }
        let cx = &mut Cx { app, store, dev, live };
        if self.bulk_on() && !cx.app.write_pending() && !store.data().busy() {
            self.run(tpl::E_POLL, 0, cx);
        }
        true
    }

    /// Each row whose text changed, while the sink has room: the program draws it once into a hash, and again
    /// into the sink when the hash isn't what was sent.
    pub(super) fn draw<S: ChannelStore>(&mut self, app: &mut App, store: &S, dev: &Device, live: &LiveAll, out: &mut impl Sink) {
        if self.dirty == 0 {
            return;
        }
        // the lib and the page once a redraw (about once a second, and per key), never kept
        let mut buf = [0u8; tpl::PACK_MAX];
        // while the help shows, the page's help segment instead of the page
        let seg = self.pg.v[V_PAGE] + if self.pg.v[V_HELP] != 0 { NPAGES } else { 0 };
        let Some((code, lib)) = tpl::load(&mut buf, seg) else { return };
        let cx = &mut Cx { app, store, dev, live };
        self.vm(code, lib, tpl::E_PREP, 0, cx, &mut W::new(&mut Null, self.cols));
        for r in 0..self.rows {
            if self.dirty & (1 << r) == 0 {
                continue;
            }
            let mut h = Hash(0x811C_9DC5);
            self.vm(code, lib, tpl::E_FRAME, r as i32, cx, &mut W::new(&mut h, self.cols));
            let h = h.0 | 1;
            if h != self.hash[r as usize] {
                if out.room() < ROW_MAX {
                    return;
                }
                let mut p = Buf::<12>::new();
                put!(p, "\x1b[{};1H", r as u16 + 1);
                out.put(p.as_bytes());
                self.vm(code, lib, tpl::E_FRAME, r as i32, cx, &mut W::new(&mut SinkOut(out), self.cols));
                out.put(b"\x1b[0m\x1b[K");
                self.hash[r as usize] = h;
            }
            self.dirty &= !(1 << r);
        }
    }
}

/// What the main menu's entries open (tests).
#[cfg(test)]
pub(super) const MAIN: [Page; 7] = [Page::Channels, Page::Zones, Page::Settings, Page::Monitor, Page::Mail, Page::Tnc, Page::About];
#[cfg(test)]
pub(super) use crate::chan::NO_ZONE;

/// The program's state as the tests read and set it.
#[cfg(test)]
impl Term {
    fn var(&self, v: usize) -> i32 {
        self.pg.v[v] as i32
    }

    pub(super) fn main_sel(&self) -> u8 {
        self.var(tpl::V_MSEL) as u8
    }

    pub(super) fn chan_sel(&self) -> u16 {
        self.var(tpl::V_CSEL) as u16
    }

    pub(super) fn field(&self) -> u8 {
        self.var(tpl::V_FIELD) as u8
    }

    pub(super) fn set_field(&mut self, f: u8) {
        self.pg.v[tpl::V_FIELD] = f as i16;
    }

    pub(super) fn set_sel(&self) -> u8 {
        self.var(tpl::V_SSEL) as u8
    }

    pub(super) fn g(&self) -> u8 {
        self.var(tpl::V_G) as u8
    }

    pub(super) fn sub(&self) -> u8 {
        self.var(tpl::V_SUB) as u8
    }

    pub(super) fn set_settings(&mut self, g: u8, sub: u8, sel: u8) {
        (self.pg.v[tpl::V_G], self.pg.v[tpl::V_SUB], self.pg.v[tpl::V_SSEL]) = (g as i16, sub as i16, sel as i16);
    }

    pub(super) fn zone(&self) -> ZoneId {
        self.var(tpl::V_ZONE) as ZoneId
    }

    pub(super) fn zone_sel(&self) -> u16 {
        self.var(tpl::V_ZSEL) as u16
    }

    pub(super) fn edit(&self) -> Option<Channel> {
        self.pg.edit
    }

    pub(super) fn set_edit(&mut self, c: Option<Channel>) {
        self.pg.edit = c;
    }

    /// Rows the channel list shows at once.
    pub(super) fn page_rows(&self) -> u16 {
        (self.rows as u16).saturating_sub(8).max(1)
    }

    /// The channel list's selected channel (the program's `selch`).
    pub(super) fn selected<S: ChannelStore>(&self, st: &S) -> Option<Channel> {
        if self.var(tpl::V_SORT) == 0 {
            return chan_at(self.zone(), self.chan_sel(), st);
        }
        let (srow, n) = (self.var(tpl::V_SROW) as usize, self.var(tpl::V_VISN) as usize);
        self.pg.vis[..n].get(srow).and_then(|&x| st.get(x))
    }

    /// Another page, as the program's `go` does.
    pub(super) fn go(&mut self, p: Page) {
        self.pg.v[tpl::V_PAGE] = PAGES.iter().position(|x| *x == p).unwrap() as i16;
        self.pg.v[tpl::V_HELP] = 0;
        self.pg.typing = false;
        self.note.clear();
        (self.warn, self.bnote) = (false, false);
    }

    /// A word of this page's program, run with `arg` on the stack: what it wrote.
    pub(super) fn call<S: ChannelStore>(&mut self, word: usize, arg: i32, app: &mut App, store: &S, dev: &Device, live: &LiveAll) -> std::vec::Vec<u8> {
        struct V(std::vec::Vec<u8>);
        impl Out for V {
            fn out(&mut self, b: &[u8]) {
                self.0.extend_from_slice(b);
            }
        }
        let mut buf = [0u8; tpl::PACK_MAX];
        let (code, lib) = tpl::load(&mut buf, self.pg.v[V_PAGE]).unwrap();
        let mut v = V(std::vec::Vec::new());
        self.vm(code, lib, word, arg, &mut Cx { app, store, dev, live }, &mut W::new(&mut v, self.cols));
        v.0
    }
}
