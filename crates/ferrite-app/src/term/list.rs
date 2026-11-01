//! The channel list's tools (TERM-14): sorting by any column, find, marks, and changes to several channels at
//! once (move to a zone, copy, renumber, delete), plus a new channel. Also who else is editing (TERM-17).
//!
//! Nothing here holds the channel list in RAM. A sorted page is filled by reading every channel for each row
//! (the next one after the row above, in the sort order) and kept until the view moves or the list changes;
//! the store's own number order needs no page. Marks are a bit per channel number. A change to several
//! channels runs one channel write per poll, after the store has taken the last one, as a load does
//! (TERM-13).

use super::{cmd::Msg, list_len, Page, Term};
use crate::chan::{Channel, ChannelStore, MAX_CHANNELS, NO_ZONE, ZoneId};
use crate::screens::Screen;
use crate::App;
use core::cmp::Ordering;
use ferrite_core::freq::mhz;
use ferrite_ui::text::{Buf, Bytes};

/// The sortable columns, as the list shows them (TERM-14): 0 is the store's own number order.
pub(super) const SORTS: [&str; 8] = ["number", "name", "RX", "TX", "mode", "tone", "power", "flags"];
/// Rows a sorted page holds at most: the tallest terminal less the list's other rows.
pub(super) const VIS: usize = 56;
/// "From the start" for a sorted page's first channel.
pub(super) const START: u16 = u16::MAX;
/// Bytes of marks: one bit per channel number.
pub(super) const MARK_BYTES: usize = MAX_CHANNELS as usize / 8;

/// What the list is asking for (the typed line, or y/n).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ask {
    None,
    Find,
    Move,
    Copy,
    Renumber,
    /// Delete: y goes ahead.
    Delete,
}

/// A change to several channels (or the selected one), one write at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Op {
    Delete,
    /// To this zone ([`NO_ZONE`]: none).
    Move(ZoneId),
    /// To free numbers from this one up.
    Copy(u16),
    /// To consecutive numbers from this one, in number order.
    Renumber(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Bulk {
    pub op: Op,
    /// No marks: just this channel.
    pub single: Option<u16>,
    /// The next channel number to look at (marks), the renumbering's phase (0 those moving up, highest first;
    /// 1 those moving down, lowest first), done so far and in all, and where the next copy may go.
    pub at: u16,
    pub phase: u8,
    pub done: u16,
    pub total: u16,
    pub free: u16,
}

/// A sort column's order for two channels; the channel number breaks ties, so the order is total.
pub(super) fn order(col: u8, a: &Channel, b: &Channel) -> Ordering {
    let lower = |c: &Channel| {
        let mut n = [0u8; 16];
        for (d, s) in n.iter_mut().zip(c.name()) {
            *d = s.to_ascii_lowercase();
        }
        n
    };
    let o = match col {
        1 => lower(a).cmp(&lower(b)),
        2 => a.rx.cmp(&b.rx),
        3 => a.cfg.tx_of(a.rx).cmp(&b.cfg.tx_of(b.rx)),
        4 => (a.cfg.mode, a.cfg.narrow).cmp(&(b.cfg.mode, b.cfg.narrow)),
        5 => (a.cfg.tx_tone, a.cfg.rx_tone).cmp(&(b.cfg.tx_tone, b.cfg.rx_tone)),
        6 => a.cfg.power.cmp(&b.cfg.power),
        7 => a.flags.cmp(&b.flags),
        _ => Ordering::Equal,
    };
    o.then(a.num.cmp(&b.num))
}

/// The numbers of a typed answer: 1-1000 as shown, back to 0-999.
fn number(s: &[u8]) -> Option<u16> {
    if s.is_empty() || s.len() > 4 || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n = s.iter().fold(0u32, |n, &c| n * 10 + (c - b'0') as u32);
    (1..=MAX_CHANNELS as u32).contains(&n).then(|| (n - 1) as u16)
}

/// Whether a channel's name holds `t` (any case), or its frequency starts with it.
fn matches(c: &Channel, t: &[u8]) -> bool {
    let name = c.name();
    let in_name = name.len() >= t.len() && name.windows(t.len()).any(|w| w.eq_ignore_ascii_case(t));
    in_name || super::mhz5(c.rx).as_bytes().starts_with(t)
}

/// The channel open in one of the radio's editors (TERM-17): the active slot's channel while its channel
/// editor, or an editor opened from it, is on the radio's screen.
pub(crate) fn radio_channel(app: &App) -> Option<u16> {
    let from_editor = app.back.iter().flatten().any(|s| matches!(s, Screen::ChEdit(_)));
    let open = match app.screen {
        Screen::ChEdit(_) => true,
        Screen::Value(_) | Screen::Tones(_) | Screen::Text(_) | Screen::Entry(_) => from_editor,
        _ => false,
    };
    let s = &app.slots[app.active as usize];
    s.ch.filter(|_| open && s.on_channel()).map(|c| c.num)
}

/// The setting highlighted in the radio's menu (TERM-17): the knob changes it there at once (MENU-16).
pub(super) fn radio_setting(app: &App) -> Option<([u8; 3], usize)> {
    match app.screen {
        Screen::Group(g) if g.sub == 0 => Some(([g.g, g.row + 1, 0], 2)),
        Screen::Group(g) => Some(([g.g, g.sub, g.row + 1], 3)),
        _ => None,
    }
}

impl Term {
    // ---- Marks ----

    pub(super) fn marked(&self, num: u16) -> bool {
        self.marks.get(num as usize / 8).is_some_and(|b| b >> (num % 8) & 1 != 0)
    }

    fn set_mark(&mut self, num: u16, on: bool) {
        if let Some(b) = self.marks.get_mut(num as usize / 8) {
            if on {
                *b |= 1 << (num % 8);
            } else {
                *b &= !(1 << (num % 8));
            }
        }
    }

    pub(super) fn marks_n(&self) -> u16 {
        self.marks.iter().map(|b| b.count_ones() as u16).sum()
    }

    /// The first marked number at or after `from`.
    fn next_mark(&self, from: u16) -> Option<u16> {
        (from..MAX_CHANNELS).find(|&n| self.marked(n))
    }

    /// The last marked number below `below`.
    fn prev_mark(&self, below: u16) -> Option<u16> {
        (0..below.min(MAX_CHANNELS)).rev().find(|&n| self.marked(n))
    }

    // ---- The list in the sort order ----

    fn in_list(&self, c: &Channel) -> bool {
        self.zone == super::ALL || c.zone == self.zone
    }

    /// The channel after `after` in the sort order (the first when None): one pass over every channel.
    fn after<S: ChannelStore>(&self, after: Option<&Channel>, st: &S) -> Option<Channel> {
        let mut best: Option<Channel> = None;
        for n in 0..MAX_CHANNELS {
            let Some(c) = st.get(n).filter(|c| self.in_list(c)) else { continue };
            if after.is_none_or(|a| order(self.sort, &c, a) == Ordering::Greater) && best.as_ref().is_none_or(|b| order(self.sort, &c, b) == Ordering::Less) {
                best = Some(c);
            }
        }
        best
    }

    /// The channel before `before` in the sort order (the last when None).
    fn before<S: ChannelStore>(&self, before: Option<&Channel>, st: &S) -> Option<Channel> {
        let mut best: Option<Channel> = None;
        for n in 0..MAX_CHANNELS {
            let Some(c) = st.get(n).filter(|c| self.in_list(c)) else { continue };
            if before.is_none_or(|b| order(self.sort, &c, b) == Ordering::Less) && best.as_ref().is_none_or(|x| order(self.sort, &c, x) == Ordering::Greater) {
                best = Some(c);
            }
        }
        best
    }

    /// Fills the sorted page from its first channel (TERM-14).
    pub(super) fn fill<S: ChannelStore>(&mut self, st: &S) {
        let pg = (self.page_rows() as usize).min(VIS);
        let mut prev = if self.first == START { None } else { st.get(self.first).filter(|c| self.in_list(c)) };
        let mut n = 0;
        if let Some(c) = prev {
            self.vis[0] = c.num;
            n = 1;
        }
        while n < pg {
            let Some(c) = self.after(prev.as_ref(), st) else { break };
            self.vis[n] = c.num;
            prev = Some(c);
            n += 1;
        }
        self.vis_n = n as u8;
        self.srow = self.srow.min(self.vis_n.saturating_sub(1));
        (self.vis_ok, self.vis_len) = (true, list_len(self.zone, st));
    }

    /// The selected channel.
    pub(super) fn selected<S: ChannelStore>(&self, st: &S) -> Option<Channel> {
        if self.sort == 0 {
            return super::chan_at(self.zone, self.chan_sel, st);
        }
        self.vis[..self.vis_n as usize].get(self.srow as usize).and_then(|&n| st.get(n))
    }

    /// The channel on row `i` of the page (0 the first shown).
    pub(super) fn row_chan<S: ChannelStore>(&self, i: u16, st: &S) -> Option<Channel> {
        if self.sort == 0 {
            return super::chan_at(self.zone, self.chan_top + i, st);
        }
        self.vis[..self.vis_n as usize].get(i as usize).and_then(|&n| st.get(n))
    }

    /// Whether row `i` of the page is the selected one.
    pub(super) fn row_selected(&self, i: u16) -> bool {
        if self.sort == 0 { self.chan_top + i == self.chan_sel } else { i == self.srow as u16 }
    }

    /// The view starts over (a new sort, zone or list).
    pub(super) fn list_reset(&mut self) {
        (self.chan_sel, self.chan_top, self.srow, self.first, self.vis_ok) = (0, 0, 0, START, false);
    }

    /// Arrows, pages and the ends in a sorted list: the page moves by its first channel.
    fn sorted_move<S: ChannelStore>(&mut self, k: super::K, st: &S) {
        use super::K;
        if !self.vis_ok {
            self.fill(st);
        }
        let n = self.vis_n;
        let pg = self.page_rows();
        let at = |t: &Term, i: u8| st.get(t.vis[i as usize]);
        match k {
            K::Down if self.srow + 1 < n => self.srow += 1,
            K::Down => {
                if n > 0 && self.after(at(self, n - 1).as_ref(), st).is_some() {
                    self.first = self.vis[1.min(n as usize - 1)];
                    if n == 1 {
                        self.first = self.after(at(self, 0).as_ref(), st).map_or(START, |c| c.num);
                    }
                    self.vis_ok = false;
                }
            }
            K::Up if self.srow > 0 => self.srow -= 1,
            K::Up => {
                if let Some(p) = self.before(at(self, 0).as_ref(), st) {
                    (self.first, self.vis_ok) = (p.num, false);
                }
            }
            K::PgDn => match self.after(n.checked_sub(1).and_then(|l| at(self, l)).as_ref(), st) {
                Some(c) if n > 0 => (self.first, self.vis_ok) = (c.num, false),
                _ => self.srow = n.saturating_sub(1),
            },
            K::PgUp => {
                let mut p = at(self, 0);
                let mut moved = false;
                for _ in 0..pg {
                    match self.before(p.as_ref(), st) {
                        Some(c) => (p, moved) = (Some(c), true),
                        None => break,
                    }
                }
                if moved {
                    (self.first, self.vis_ok) = (p.map_or(START, |c| c.num), false);
                } else {
                    self.srow = 0;
                }
            }
            K::Home => (self.first, self.srow, self.vis_ok) = (START, 0, false),
            K::End => {
                let mut p = self.before(None, st);
                for _ in 1..pg {
                    match self.before(p.as_ref(), st) {
                        Some(c) => p = Some(c),
                        None => break,
                    }
                }
                (self.first, self.srow, self.vis_ok) = (p.map_or(START, |c| c.num), pg.min(VIS as u16) as u8, false);
            }
            _ => {}
        }
        if !self.vis_ok {
            self.fill(st);
        }
    }

    // ---- Keys ----

    /// The list's own keys (TERM-14); true when the key was one of them.
    pub(super) fn list_key<S: ChannelStore>(&mut self, app: &mut App, k: super::K, st: &S) -> bool {
        use super::K;
        if self.ask == Ask::Delete {
            self.ask = Ask::None;
            if matches!(k, K::Ch(b'y' | b'Y')) {
                self.begin(app, Op::Delete, st);
            } else {
                self.tell("Nothing deleted".as_bytes(), false);
            }
            return true;
        }
        if self.bulk.is_some() {
            if matches!(k, K::Esc | K::CtrlC) {
                self.bulk = None;
                self.tell("Stopped: what was done stays done".as_bytes(), true);
            }
            return true; // keys wait while several channels change
        }
        match k {
            K::Up | K::Down | K::PgUp | K::PgDn | K::Home | K::End if self.sort != 0 => self.sorted_move(k, st),
            K::Ch(b's' | b'S') => {
                self.sort = (self.sort + 1) % SORTS.len() as u8;
                self.list_reset();
            }
            K::Ch(b' ') => {
                if let Some(c) = self.selected(st) {
                    self.set_mark(c.num, !self.marked(c.num));
                    self.list_key(app, K::Down, st);
                    if self.sort == 0 {
                        self.chan_sel = (self.chan_sel + 1).min(list_len(self.zone, st).saturating_sub(1));
                        self.keep_in_view();
                    }
                }
            }
            K::Ch(b'a' | b'A') => {
                for n in 0..MAX_CHANNELS {
                    if st.get(n).is_some_and(|c| self.in_list(&c)) {
                        self.set_mark(n, true);
                    }
                }
            }
            K::Ch(b'u' | b'U') => self.marks = [0; MARK_BYTES],
            K::Ch(b'/') => self.prompt(Ask::Find),
            K::Ch(b'm' | b'M') => self.prompt(Ask::Move),
            K::Ch(b'c' | b'C') => self.prompt(Ask::Copy),
            K::Ch(b'r' | b'R') => self.prompt(Ask::Renumber),
            K::Ch(b'd' | b'D') => {
                if self.targets(st).0 > 0 {
                    self.ask = Ask::Delete;
                }
            }
            K::Ch(b'n' | b'N') => self.new_channel(app, st),
            _ => return false,
        }
        true
    }

    fn keep_in_view(&mut self) {
        let pg = self.page_rows();
        if self.chan_sel < self.chan_top {
            self.chan_top = self.chan_sel;
        } else if self.chan_sel >= self.chan_top + pg {
            self.chan_top = self.chan_sel + 1 - pg;
        }
    }

    /// How many channels a change applies to: the marked ones, else the selected one (and which).
    fn targets<S: ChannelStore>(&self, st: &S) -> (u16, Option<u16>) {
        match self.marks_n() {
            0 => self.selected(st).map_or((0, None), |c| (1, Some(c.num))),
            n => (n, None),
        }
    }

    fn prompt(&mut self, a: Ask) {
        self.ask = a;
        self.line = Some(Buf::new());
    }

    /// A message on the bottom row until the next key.
    fn tell(&mut self, s: &[u8], warn: bool) {
        self.note.set(s);
        (self.warn, self.bnote) = (warn, true);
        self.all_dirty();
    }

    /// What the prompt row says, for the line being typed.
    pub(super) fn ask_text<S: ChannelStore>(&self, st: &S, out: &mut Msg) {
        let (n, one) = self.targets(st);
        let what = |out: &mut Msg| match one {
            Some(c) => {
                put!(out, "channel {}", c as u32 + 1);
            }
            None => {
                put!(out, "{} marked channels", n);
            }
        };
        match self.ask {
            Ask::Find => out.push("Find (name or MHz): ".as_bytes()),
            Ask::Move => {
                out.push(b"Move ");
                what(out);
                out.push(" to zone (number, name, or none): ".as_bytes());
            }
            Ask::Copy => {
                out.push(b"Copy ");
                what(out);
                out.push(" to free numbers from (Enter: first free): ".as_bytes());
            }
            Ask::Renumber => {
                out.push("Renumber ".as_bytes());
                what(out);
                out.push(" from number: ".as_bytes());
            }
            Ask::Delete => {
                out.push(b"Delete ");
                what(out);
                out.push("? y deletes, any other key keeps".as_bytes());
            }
            Ask::None => {}
        }
    }

    /// The typed answer to a prompt (Enter).
    pub(super) fn answer<S: ChannelStore>(&mut self, app: &mut App, s: &[u8], st: &S) {
        let a = core::mem::replace(&mut self.ask, Ask::None);
        let s = super::cmd_trim(s);
        match a {
            Ask::Find => self.find_next(s, st),
            Ask::Move => {
                let z = if s.eq_ignore_ascii_case(b"none") || s == b"0" {
                    Some(NO_ZONE)
                } else if let Some(n) = number(s).filter(|&n| n < st.zones()) {
                    Some(n)
                } else {
                    (0..st.zones()).find(|&z| st.zone_name(z).is_some_and(|nm| nm.as_bytes().eq_ignore_ascii_case(s)))
                };
                match z {
                    Some(z) => self.begin(app, Op::Move(z), st),
                    None => self.tell("No such zone: a number, a name, or none".as_bytes(), true),
                }
            }
            Ask::Copy => {
                let from = if s.is_empty() { Some(0) } else { number(s) };
                match from {
                    Some(f) => self.begin(app, Op::Copy(f), st),
                    None => self.tell("Copy to: a channel number from 1 to 1000".as_bytes(), true),
                }
            }
            Ask::Renumber => match number(s) {
                Some(f) => self.begin(app, Op::Renumber(f), st),
                None => self.tell("Renumber from: a channel number from 1 to 1000".as_bytes(), true),
            },
            Ask::Delete | Ask::None => {}
        }
    }

    /// Find: from the row after the selection, in the list's order, wrapping.
    fn find_next<S: ChannelStore>(&mut self, s: &[u8], st: &S) {
        if !s.is_empty() {
            self.find = Buf::from(&s[..s.len().min(16)]);
        }
        let t = self.find;
        if t.is_empty() {
            return self.tell("Find what? Part of a name or MHz".as_bytes(), true);
        }
        if self.sort == 0 {
            let n = list_len(self.zone, st);
            for k in 1..=n {
                let i = (self.chan_sel + k) % n.max(1);
                if super::chan_at(self.zone, i, st).is_some_and(|c| matches(&c, t.as_bytes())) {
                    self.chan_sel = i;
                    self.keep_in_view();
                    return;
                }
            }
        } else {
            let start = self.selected(st);
            let mut c = self.after(start.as_ref(), st).or_else(|| self.after(None, st));
            let mut seen = 0;
            while let Some(x) = c {
                if matches(&x, t.as_bytes()) {
                    (self.first, self.srow) = (x.num, 0);
                    return self.fill(st);
                }
                seen += 1;
                if seen > MAX_CHANNELS {
                    break;
                }
                c = self.after(Some(&x), st).or_else(|| if start.is_some() { self.after(None, st) } else { None });
                if c.map(|y| y.num) == start.map(|y| y.num) {
                    if start.is_some_and(|y| matches(&y, t.as_bytes())) {
                        return;
                    }
                    break;
                }
            }
        }
        let mut m = Msg::new();
        put!(m, "No channel here matches \"{}\"", Bytes(t.as_bytes()));
        self.tell(m.as_bytes(), true);
    }

    /// A new channel at the first free number, in this list's zone, on the active slot's frequency, and its
    /// editor (TERM-14).
    fn new_channel<S: ChannelStore>(&mut self, app: &mut App, st: &S) {
        let Some(num) = st.free_num(0) else { return self.tell("All 1000 channels are in use".as_bytes(), true) };
        let s = &app.slots[app.active as usize];
        let rx = s.freq().clamp(mhz(64), mhz(1300)); // CHAN-22
        let zone = if self.zone == super::ALL { NO_ZONE } else { self.zone };
        let c = Channel { num, zone, rx, name: [0; 16], cfg: *s.cfg(), flags: 0, tones: 0 };
        if let Err(r) = crate::chfield::check_edit(&c) {
            let mut m = Msg::new();
            r.line(&mut m);
            return self.tell(m.as_bytes(), true); // the slot's TX wouldn't fit a channel (CHAN-22, TX-37)
        }
        app.put_channel(c);
        (self.edit, self.field) = (Some(c), 0);
        self.vis_ok = false;
        self.go(Page::Edit);
        let mut m = Msg::new();
        put!(m, "New channel {}: Enter types its name", num as u32 + 1);
        self.say(m.as_bytes(), false);
    }

    // ---- Several channels ----

    /// Checks and starts a change; it runs from [`Term::bulk_step`].
    fn begin<S: ChannelStore>(&mut self, app: &App, op: Op, st: &S) {
        let (total, single) = self.targets(st);
        if total == 0 {
            return;
        }
        // TERM-17: not a channel open in the radio's editor.
        if let Some(r) = radio_channel(app).filter(|&r| single == Some(r) || single.is_none() && self.marked(r)) {
            let mut m = Msg::new();
            put!(m, "Channel {} is open in the radio's editor", r as u32 + 1);
            return self.tell(m.as_bytes(), true);
        }
        if let Op::Renumber(from) = op {
            if let Err(m) = self.renumber_check(from, total, single, st) {
                return self.tell(m.as_bytes(), true);
            }
        }
        let at = match op {
            Op::Renumber(_) => MAX_CHANNELS,
            _ => 0,
        };
        let free = if let Op::Copy(f) = op { f } else { 0 };
        self.bulk = Some(Bulk { op, single, at, phase: 0, done: 0, total, free });
    }

    /// The number a channel gets when renumbering from `from`: the marked ones keep their order.
    fn renumber_to(&self, from: u16, num: u16, single: Option<u16>) -> u16 {
        if single.is_some() {
            return from;
        }
        from + (0..num).filter(|&n| self.marked(n)).count() as u16
    }

    /// Every new number must be free, or held by a channel that moves too; and inside 1-1000.
    fn renumber_check<S: ChannelStore>(&self, from: u16, total: u16, single: Option<u16>, st: &S) -> Result<(), Msg> {
        if from + total > MAX_CHANNELS {
            let mut m = Msg::new();
            put!(m, "{} channels from {} would pass 1000", total, from as u32 + 1);
            return Err(m);
        }
        let mut n = single.or_else(|| self.next_mark(0));
        while let Some(x) = n {
            let t = self.renumber_to(from, x, single);
            let moving = |y: u16| single == Some(y) || single.is_none() && self.marked(y);
            if t != x && st.get(t).is_some() && !moving(t) {
                let mut m = Msg::new();
                put!(m, "Number {} is taken by a channel that isn't moving", t as u32 + 1);
                return Err(m);
            }
            n = if single.is_some() { None } else { self.next_mark(x + 1) };
        }
        Ok(())
    }

    /// One write of a running change, when the store has taken the last one (called from the poll).
    pub(super) fn bulk_step<S: ChannelStore>(&mut self, app: &mut App, st: &S) {
        let Some(mut b) = self.bulk else { return };
        // The next channel: the single one once, else the next mark (renumbering: moving up highest first,
        // then moving down lowest first, so no channel lands on one that hasn't moved yet).
        let next = match (b.single, b.op) {
            (Some(n), _) => (b.done == 0).then_some(n),
            (None, Op::Renumber(from)) => loop {
                let n = if b.phase == 0 { self.prev_mark(b.at) } else { self.next_mark(b.at) };
                match n {
                    None if b.phase == 0 => (b.phase, b.at) = (1, 0),
                    None => break None,
                    Some(x) => {
                        b.at = if b.phase == 0 { x } else { x + 1 };
                        let t = self.renumber_to(from, x, None);
                        if (b.phase == 0 && t > x) || (b.phase == 1 && t < x) {
                            break Some(x);
                        }
                        if t == x && b.phase == 1 {
                            b.done += 1; // already where it goes
                        }
                    }
                }
            },
            (None, _) => {
                let n = self.next_mark(b.at);
                if let Some(x) = n {
                    b.at = x + 1;
                }
                n
            }
        };
        let Some(x) = next else { return self.bulk_done(b) };
        let Some(c) = st.get(x) else {
            self.bulk = Some(b); // gone meanwhile: nothing to do for it
            return;
        };
        match b.op {
            Op::Delete => app.delete_channel(x, st),
            Op::Move(z) if c.zone != z && !st.zone_room(z, x) => {
                // CHAN-35: the zone filled up: the rest stay where they are
                let mut m = Msg::new();
                put!(m, "Zone full (256 channels): moved {} of {}", b.done, b.total);
                self.bulk = None;
                self.vis_ok = false;
                return self.tell(m.as_bytes(), true);
            }
            Op::Move(z) if c.zone != z => app.put_channel(Channel { zone: z, ..c }),
            Op::Move(_) => {}
            Op::Copy(_) => match (b.free..MAX_CHANNELS).find(|&n| st.get(n).is_none()) {
                Some(t) => {
                    app.put_channel(Channel { num: t, ..c });
                    b.free = t + 1;
                }
                None => {
                    let mut m = Msg::new();
                    put!(m, "No free numbers left: copied {} of {}", b.done, b.total);
                    self.bulk = None;
                    self.marks = [0; MARK_BYTES];
                    self.vis_ok = false;
                    return self.tell(m.as_bytes(), true);
                }
            },
            Op::Renumber(from) => app.renumber_channel(c, self.renumber_to(from, x, b.single)),
        }
        b.done += 1;
        self.bulk = Some(b);
        let mut m = Msg::new();
        put!(m, "{} of {}...", b.done, b.total);
        self.tell(m.as_bytes(), false);
    }

    fn bulk_done(&mut self, b: Bulk) {
        self.bulk = None;
        self.marks = [0; MARK_BYTES];
        self.vis_ok = false;
        let what = match b.op {
            Op::Delete => "deleted",
            Op::Move(_) => "moved",
            Op::Copy(_) => "copied",
            Op::Renumber(_) => "renumbered",
        };
        let mut m = Msg::new();
        put!(m, "{} {} {}", b.total, if b.total == 1 { "channel" } else { "channels" }, what);
        self.tell(m.as_bytes(), false);
    }
}
