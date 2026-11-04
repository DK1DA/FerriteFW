//! The full-screen VT100 pages (TERM-11 to TERM-20; the `vt100` cargo feature, decision 342): the page state's
//! keys, the rows drawn from the templates ([`super::tpl`]), the channel editor's and the settings' changes.
//! Without the feature the terminal is a plain command line (`term.rs`).

use super::*;
pub(super) use crate::chan::{Channel, FLAG_APRS, FLAG_HIDDEN, FLAG_RX_ONLY, NO_ZONE, ZoneId};
pub(super) use crate::chfield::{ChField as F, Ctx, Op};
pub(super) use crate::screens::groups::{self, SUBS};
pub(super) use crate::slot::Item;

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
/// What the main menu's entries open (their names are the template's).
pub(super) const MAIN: [Page; 7] = [Page::Channels, Page::Zones, Page::Settings, Page::Monitor, Page::Mail, Page::Tnc, Page::About];


pub(super) fn item(it: Item, c: &Channel, w: &mut W) {
    let mut v = Buf::<16>::new();
    it.text(&c.cfg, false, &mut v);
    w.s(v.as_bytes());
}


/// "12 s ago", "3 min ago", "2 h ago".
pub(super) fn ago(s: u32, b: &mut Buf<16>) {
    match s {
        crate::data::AGO_UNKNOWN => put!(b, "earlier"), // INBOX-6.1
        0..=59 => put!(b, "{} s ago", s),
        60..=3599 => put!(b, "{} min ago", s / 60),
        _ => put!(b, "{} h ago", s / 3600),
    };
}

/// The zones list's i-th entry: every channel, each zone, then no zone.
pub(super) fn zone_of<S: ChannelStore>(i: u16, st: &S) -> ZoneId {
    match i {
        0 => ALL,
        _ if i <= st.zones() => i - 1,
        _ => NO_ZONE,
    }
}

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
    pub(super) app: &'a App,
    pub(super) store: &'a S,
    pub(super) dev: &'a Device,
    pub(super) live: &'a LiveAll,
    /// The template pack, read for this redraw (TERM-47, UPD-21).
    pub(super) pk: &'a [u8],
}

/// Where a row's bytes go: the hash that tells whether it changed, or the sink.
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


/// One row: text clipped to the width, columns counted (attributes take none), reverse video and bold only
/// (TERM-1: plain VT100); with `colour` (TERM-18, setting 0 9 9 3), ANSI colours in their place: the top line
/// and the selection white on blue, headings cyan, warnings yellow.
pub(super) struct W<'a> {
    pub(super) o: &'a mut dyn Out,
    pub(super) col: u16,
    pub(super) cols: u16,
    pub(super) rev: bool,
    pub(super) colour: bool,
}

impl W<'_> {
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
            self.o.out(&[c]);
            self.col += 1;
            i += skip;
        }
    }

    /// A number in decimal.
    pub(super) fn n(&mut self, n: u32) {
        self.s(digits(n, &mut [0; 10]));
    }

    pub(super) fn digit(&mut self, d: u8) {
        self.s(&[b'0' + d % 10]);
    }

    /// Spaces up to column `c`.
    pub(super) fn at(&mut self, c: u16) {
        while self.col < c.min(self.cols) {
            self.o.out(b" ");
            self.col += 1;
        }
    }

    /// Spaces to the end of the row (a reverse-video bar or selection runs the whole width).
    pub(super) fn fill(&mut self) {
        self.at(self.cols);
    }

    pub(super) fn rev(&mut self) {
        self.o.out(if self.colour { b"\x1b[0;97;44m" } else { b"\x1b[7m" });
        self.rev = true;
    }

    /// What the attributes go back to after bold or a warning.
    pub(super) fn back(&mut self) {
        self.o.out(match (self.rev, self.colour) {
            (true, true) => b"\x1b[0;97;44m",
            (true, false) => b"\x1b[0m\x1b[7m",
            (false, _) => b"\x1b[0m",
        });
    }

    /// A warning: bold, or yellow in colour.
    pub(super) fn warn(&mut self, b: &[u8]) {
        self.won();
        self.s(b);
        self.back();
    }

    /// A warning's attributes on.
    pub(super) fn won(&mut self) {
        if !self.colour {
            return self.bon();
        }
        self.o.out(if self.rev { b"\x1b[1;93;44m" } else { b"\x1b[1;33m" });
    }

    /// Bold on (cyan in colour, heading colours on the selection).
    pub(super) fn bon(&mut self) {
        self.o.out(match (self.rev, self.colour) {
            (true, true) => b"\x1b[1;97;44m",
            (false, true) => b"\x1b[1;36m",
            _ => b"\x1b[1m",
        });
    }

    pub(super) fn plain(&mut self) {
        self.o.out(b"\x1b[0m");
        self.rev = false;
    }

    pub(super) fn bold(&mut self, b: &[u8]) {
        self.bon();
        self.s(b);
        // VT100 has no "bold off": all off, then reverse (or the colours) again if it was on.
        self.back();
    }
}



impl Term {
    /// A key on the page (after the console, the command line, Ctrl-C and Ctrl-L).
    pub(super) fn page_key<S: ChannelStore>(&mut self, app: &mut App, k: K, store: &S) {
        // A value being typed takes every key until Enter or Esc.
        if let Some(mut l) = self.line {
            match k {
                K::Enter => {
                    self.line = None;
                    if self.ask == list::Ask::None {
                        self.commit(app, l.as_bytes());
                    } else {
                        self.answer(app, l.as_bytes(), store);
                    }
                }
                K::Esc => (self.line, self.ask) = (None, list::Ask::None),
                K::Bs => {
                    let n = l.len().saturating_sub(1);
                    let t = Buf::<24>::from(&l.as_bytes()[..n]);
                    self.line = Some(t);
                }
                K::Ch(c) if l.len() < self.line_max() => {
                    l.push(&[c]);
                    self.line = Some(l);
                }
                _ => {}
            }
            return;
        }
        if self.help {
            if matches!(k, K::Esc | K::Ch(b'?') | K::Pf(1) | K::Enter) {
                self.help = false;
            }
            return;
        }
        if matches!(k, K::Ch(b'?') | K::Pf(1)) {
            self.help = true;
            return;
        }
        if k == K::Ch(b':') && self.page != Page::Off {
            (self.cmd, self.clen) = (true, 0);
            return;
        }
        match self.page {
            Page::Off => {}
            Page::Channels => {
                if !self.list_key(app, k, store) {
                    self.tpl_key(app, k, store);
                    self.keep_in_view();
                }
            }
            Page::Edit if self.edit.is_none() => self.go(Page::Channels),
            Page::Settings if self.g != 0 && groups::items(self.g, self.sub).is_empty() => self.settings_up(),
            _ => {
                self.tpl_key(app, k, store);
            }
        }
    }

    /// The pages' part of a poll: the screen cleared, a change to several channels stepped, a sorted page
    /// filled, the radio's copy of a channel it is editing; false when the sink is full.
    pub(super) fn page_poll<S: ChannelStore>(&mut self, app: &mut App, store: &S, out: &mut impl Sink) -> bool {
        if self.clear && self.con == Con::Off {
            if out.room() < 16 {
                return false;
            }
            out.put(b"\x1b[0m\x1b[2J");
            self.clear = false;
        }
        // Several channels changing (TERM-14): one write each time the store has taken the last.
        if self.bulk.is_some() && !app.write_pending() && !store.data().busy() {
            self.bulk_step(app, store);
            if self.bulk.is_none() {
                self.chan_sel = self.chan_sel.min(list_len(self.zone, store).saturating_sub(1));
            }
        }
        if self.page == Page::Channels && self.sort != 0 && (!self.vis_ok || self.vis_len != list_len(self.zone, store)) {
            self.fill(store);
        }
        // TERM-17: a channel open in the radio's editor shows the radio's copy.
        if let Some(c) = self.edit.filter(|c| self.page == Page::Edit && radio_channel(app) == Some(c.num)) {
            self.edit = store.get(c.num).or(Some(c));
        }
        true
    }

    /// Each row whose text changed, while the sink has room.
    pub(super) fn draw<S: ChannelStore>(&mut self, app: &App, store: &S, dev: &Device, live: &LiveAll, out: &mut impl Sink) {
        if self.dirty == 0 {
            return;
        }
        // the pack once a redraw (about once a second, and per key), never kept
        let mut buf = [0u8; tpl::PACK_MAX];
        let Some(pk) = tpl::load(&mut buf) else { return };
        let cx = Cx { app, store, dev, live, pk };
        for r in 0..self.rows {
            if self.dirty & (1 << r) == 0 {
                continue;
            }
            let mut h = Hash(0x811C_9DC5);
            let colour = app.set.term_colour; // TERM-18, 0 9 9 3
            self.row(r, &cx, &mut W { o: &mut h, col: 0, cols: self.cols as u16, rev: false, colour });
            let h = h.0 | 1;
            if h != self.hash[r as usize] {
                if out.room() < ROW_MAX {
                    return;
                }
                let mut p = Buf::<12>::new();
                put!(p, "\x1b[{};1H", r as u16 + 1);
                out.put(p.as_bytes());
                let mut s = SinkOut(out);
                self.row(r, &cx, &mut W { o: &mut s, col: 0, cols: self.cols as u16, rev: false, colour });
                out.put(b"\x1b[0m\x1b[K");
                self.hash[r as usize] = h;
            }
            self.dirty &= !(1 << r);
        }
    }


    pub(super) fn open_main(&mut self) {
        let p = MAIN[self.main_sel as usize % MAIN.len()];
        match p {
            Page::Channels => {
                (self.zone, self.from_zones) = (ALL, false);
                self.list_reset();
            }
            Page::Settings => (self.g, self.sub, self.set_sel) = (0, 0, 0),
            _ => {}
        }
        self.go(p);
    }

    /// Rows the channel list shows at once.
    pub(super) fn page_rows(&self) -> u16 {
        (self.rows as u16).saturating_sub(8).max(1)
    }

    pub(super) fn line_max(&self) -> usize {
        if self.ask != list::Ask::None {
            return 16;
        }
        match FIELDS[self.field as usize % FIELDS.len()] {
            F::Name => 16,
            _ => 10,
        }
    }

    /// ◀▶ on a field: the same steps and checks as the radio's channel editor (TERM-6, TERM-7).
    pub(super) fn change<S: ChannelStore>(&mut self, app: &mut App, c: Channel, f: F, d: i32, store: &S) {
        match f {
            F::Name => return self.say("Enter types the name (16 characters)".as_bytes(), false),
            F::Tx if !c.cfg.split => return self.say("TX: Enter types a TX frequency (split)".as_bytes(), false),
            _ => {}
        }
        self.apply(app, c, f, Op::Step(d), store.zones(), &|z| store.zone_room(z, c.num));
        if f == F::It(Item::Offset) && self.edit.is_some_and(|n| n.cfg.split && !c.cfg.split) {
            // Split on today's TX, as the radio's knob: the TX row, typing another
            self.field = FIELDS.iter().position(|x| *x == F::Tx).unwrap_or(0) as u8;
            self.line = Some(Buf::new());
            self.say("Split: type the TX frequency, then Enter".as_bytes(), false);
        }
    }

    /// # on a field: its default (MENU-18).
    pub(super) fn default(&mut self, app: &mut App, c: Channel, f: F) {
        self.apply(app, c, f, Op::Reset, 0, &crate::chfield::any_room);
    }

    /// A typed value: the name, or the RX or split TX frequency in MHz.
    pub(super) fn commit(&mut self, app: &mut App, s: &[u8]) {
        let Some(c) = self.edit else { return };
        let f = FIELDS[self.field as usize % FIELDS.len()];
        self.apply(app, c, f, Op::Typed(s), 0, &crate::chfield::any_room);
    }

    /// A field's change through its rules: saved, Home set, or the refusal said.
    pub(super) fn apply(&mut self, app: &mut App, c: Channel, f: F, op: Op, zones: ZoneId, room: &dyn Fn(ZoneId) -> bool) {
        let (mut n, mut home) = (c, app.set.home_ch);
        if let Err(r) = f.change(&mut n, &mut home, op, Ctx { zones, region: app.set.region(), room }) {
            let mut m = Buf::<96>::new();
            r.line(&mut m);
            return self.say(m.as_bytes(), true);
        }
        if home != app.set.home_ch {
            app.set.home_ch = home;
            let on = home == Some(c.num);
            return self.say(if on { "Home channel: hold 0 on home jumps here".as_bytes() } else { "No longer the home channel".as_bytes() }, false);
        }
        self.save(app, n);
    }

    pub(super) fn save(&mut self, app: &mut App, n: Channel) {
        if radio_channel(app) == Some(n.num) {
            // TERM-17: the radio's editor has it; its copy wins and shows here.
            return self.say("Open in the radio's editor: change it there".as_bytes(), true);
        }
        if Some(n) != self.edit {
            app.put_channel(n);
            self.edit = Some(n);
            self.say(b"Saved", false);
        }
    }

    pub(super) fn settings_up(&mut self) {
        if self.sub != 0 {
            (self.set_sel, self.sub) = (self.sub - 1, 0);
        } else if self.g != 0 {
            (self.set_sel, self.g) = (self.g - 1, 0);
        } else {
            return self.go(Page::Main);
        }
        self.note.clear();
    }

    /// The highlighted item's path after MENU 0 (the radio's numbers, TERM-6).
    pub(super) fn path(&self, row: u8) -> ([u8; 3], usize) {
        if self.sub == 0 { ([self.g, row + 1, 0], 2) } else { ([self.g, self.sub, row + 1], 3) }
    }

    pub(super) fn settings_open(&mut self, app: &mut App) {
        self.note.clear();
        if self.g == 0 {
            (self.g, self.set_sel) = (self.set_sel + 1, 0);
            return;
        }
        if self.sub == 0 && SUBS.iter().any(|(q, _)| *q == (self.g, self.set_sel + 1)) {
            (self.sub, self.set_sel) = (self.set_sel + 1, 0);
            return;
        }
        self.settings_change(app, 1);
    }

    pub(super) fn settings_change(&mut self, app: &mut App, d: i32) {
        let (p, pl) = self.path(self.set_sel);
        if radio_setting(app) == Some((p, pl)) {
            return self.say("Open on the radio's menu: change it there".as_bytes(), true); // TERM-17
        }
        let p = &p[..pl];
        if blocked(p) {
            return self.say("Change this one on the radio".as_bytes(), true);
        }
        if let Some(why) = groups::greyed(app, p) {
            return self.say(why.as_bytes(), true);
        }
        if groups::change(app, p, d) {
            self.say(b"Changed", false);
        } else {
            self.say("Not settable yet".as_bytes(), true);
        }
    }


    pub(super) fn row<S: ChannelStore>(&self, r: u8, cx: &Cx<S>, w: &mut W) {
        if r == 0 {
            return self.top(cx, w);
        }
        if self.pin_gate() {
            // LINK-30: the PIN prompt, alone.
            match r {
                2 => w.s("  This radio has a PIN lock. Type its PIN, then Enter.".as_bytes()),
                4 => {
                    w.s(b"  PIN: ");
                    for _ in 0..self.pin_n {
                        w.s(b"*");
                    }
                }
                6 if !self.note.is_empty() => w.s(self.note.as_bytes()),
                _ if r == self.rows - 1 => w.s(" Enter Check   Ctrl-C Leave".as_bytes()),
                _ => {}
            }
            return;
        }
        if r == self.rows - 1 {
            return self.hints(cx.pk, w);
        }
        let b = r - 1;
        if self.help {
            return self.help_row(cx.pk, b, w);
        }
        self.tpl_row(b, cx, w);
    }

    /// The top line (TERM-11): firmware, callsign and model; the link in the middle; the clock at the right.
    pub(super) fn top<S: ChannelStore>(&self, cx: &Cx<S>, w: &mut W) {
        w.rev();
        w.s(b" Ferrite ");
        w.s(version());
        if let Some(c) = cx.app.set.boot_callsign() {
            w.s(b"   ");
            w.s(c);
        }
        w.s(b"   RT-950 Pro");
        let link: &[u8] = if self.bt { b"bluetooth 9600" } else { b"cable 115200" };
        w.at((self.cols as u16).saturating_sub(link.len() as u16) / 2 + 8);
        w.s(link);
        let mut t = Buf::<8>::new();
        match cx.dev.clock_min {
            Some(m) => {
                put!(t, "{}:{}", Zero(m as u32 / 60 % 24, 2), Zero(m as u32 % 60, 2));
            }
            None => t.set(b"--:--"),
        }
        w.at(self.cols as u16 - 7);
        w.s(t.as_bytes());
        w.fill();
    }

    /// The bottom line: the keys for this page (TERM-11).
    pub(super) fn hints(&self, pk: &[u8], w: &mut W) {
        if self.cmd {
            // The command line: its tail, when it is wider than the row.
            let n = self.clen as usize;
            let keep = (self.cols as usize).saturating_sub(4);
            w.s(b" :");
            w.s(&self.cbuf[n.saturating_sub(keep)..n]);
            w.s(b"_");
            return;
        }
        if self.bnote {
            w.rev();
            w.s(b" ");
            if self.warn {
                w.warn(self.note.as_bytes());
            } else {
                w.s(self.note.as_bytes());
            }
            w.fill();
            return;
        }
        let keys = if self.line.is_some() {
            tpl::global(pk, tpl::KEYS_TYPE)
        } else if self.help {
            tpl::global(pk, tpl::KEYS_HELP)
        } else {
            tpl::hints(pk, self.tpl_page())
        };
        w.rev();
        w.s(b" ");
        let mut i = 0;
        while let Some(kl) = piece(keys, b'|', i) {
            w.bold(piece(kl, b'=', 0).unwrap_or_default());
            w.s(b" ");
            w.s(piece(kl, b'=', 1).unwrap_or_default());
            w.s(b"   ");
            i += 1;
        }
        w.fill();
    }

    pub(super) fn help_row(&self, pk: &[u8], b: u8, w: &mut W) {
        let lines = tpl::help(pk, self.tpl_page());
        if b == 1 {
            w.s(b" ");
            w.bold(b"Help");
        } else if let Some(l) = (b as usize).checked_sub(3).and_then(|i| piece(lines, b'\n', i)) {
            w.s(b"  ");
            w.s(l);
        }
    }


    pub(super) fn go(&mut self, p: Page) {
        self.page = p;
        self.help = false;
        self.line = None;
        self.note.clear();
        (self.warn, self.bnote) = (false, false);
    }

}
