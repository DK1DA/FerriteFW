//! The full-screen pages as templates and their interpreter (TERM-11, TERM-16; decision 342). The pages' text,
//! layout, list rows, hints, help and keys are data: the template pack, compiled by `build.rs` from `pages.vt`.
//! The firmware keeps only what reads the radio (the sources, conditions, counts) and what a key does (the
//! actions). The pack is in MCU flash for now, in the byte format the SPI flash would hold it:
//!
//! ```text
//! pack    "FVT" 1 (format 1) | u8 pages | u8 globals | u16 global string[globals] | u16 page[pages]
//! page    u16 hints string | u16 help string | u16 keys | rules | 0
//! rule    u8 kind (1 row N, 2 bot K, 3 list N COUNT, 4 tail K) | u8 N or K | u8 COUNT | u8 len | ops[len]
//! ops     10 S len text | 11 AT col | 12 SRC id | 13 BON | 14 WON | 15 BACK | 16 SEL | 17 SELFILL | 18 FILL
//!         19 PLAIN | 1A DIGIT | 1B PICK len text | 1C PICKN id len text | 1D CTX id
//!         20 IF cond | 21 IFNOT cond | 22 IFI n | 23 ELSE | 24 FI | 25 RET
//! keys    (u8 key, u8 action, u16 arg)... | 0; key: printable ASCII (letters lower case) or 80 up, 81 down,
//!         82 left, 83 right, 84 PgUp, 85 PgDn, 86 Home, 87 End, 88 Enter, 89 Esc, 8A a digit 1-9;
//!         arg: a page (go) or a string, a line command (cmd)
//! string  u16 len | bytes
//! ```
//!
//! Numbers are little-endian; offsets are from the pack's start. Every name's number is `pages.vt`'s.

use super::{cmd, digits, item, list, list_len, mhz5, piece, zone_name, zone_of, Cx, Page, Term, ALL, FIELDS, K, W};
use crate::chan::{Channel, ChannelStore, FLAG_APRS, FLAG_HIDDEN, FLAG_PRIORITY, FLAG_RX_ONLY, FLAG_SKIP};
use crate::chfield::ChField as F;
use crate::screens::advanced::GROUPS;
use crate::screens::groups::{self, SUBS};
use crate::slot::{mode_chip, tone_text, Item};
use crate::App;
use ferrite_core::freq::Freq10;
use ferrite_core::gpspower::GpsPhase;
use ferrite_core::msg::{GpsState, SLOTS};
use ferrite_ui::text::{Buf, Zero};
use ferrite_ui::widgets as w;

#[allow(dead_code)]
mod pack {
    include!(concat!(env!("OUT_DIR"), "/vt_pack.rs"));
}
use pack::*;

/// What a channel source shows with no channel (never drawn: the template checks `ch` first).
const BLANK: Channel = Channel { num: 0, zone: crate::chan::NO_ZONE, rx: 0, name: [0; 16], cfg: crate::slot::DEFAULT_CFG, flags: 0, tones: 0 };

/// Page `pg`'s binding for key `code` (TERM-11): its action, argument, and a line command's bytes (TERM-12) copied
/// out, so the pack's buffer is gone before the action runs.
#[inline(never)]
fn binding(pg: u8, code: u8) -> Option<(u8, usize, [u8; 64], usize)> {
    let mut buf = [0u8; PACK_MAX];
    let pk = load(&mut buf)?;
    let mut p = u16_at(pk, page_at(pk, pg) + 4);
    while p + 4 <= pk.len() && pk[p] != 0 {
        let (key, act, arg) = (pk[p], pk[p + 1], u16_at(pk, p + 2));
        p += 4;
        if key == code || key == 0x8A && (b'1'..=b'9').contains(&code) {
            let s = string(pk, arg);
            let mut cmd = [0u8; 64];
            let n = s.len().min(cmd.len());
            cmd[..n].copy_from_slice(&s[..n]);
            return Some((act, arg, cmd, n));
        }
    }
    None
}

/// The pack's largest size: the stack buffer it is read into (build.rs checks it fits).
pub(super) const PACK_MAX: usize = 4096;

/// The pack as built with this crate (host builds: the packer writes it; tests and the simulators without parts
/// read it).
#[cfg(not(target_os = "none"))]
pub fn body() -> &'static [u8] {
    &PACK
}

/// Whether the template part is there (TERM-47, UPD-21: without it the terminal is the plain command line).
pub(super) fn present() -> bool {
    (cfg!(not(target_os = "none")) && !ferrite_core::part::has_reader()) || ferrite_core::part::ok(&ferrite_core::part::TERM)
}

/// The pack: the template part read into `buf` (3.6 KB at SPI speed, about 2.5 ms; once a redraw or key, never
/// kept), or a host build's own with no reader set. None when the part isn't there.
#[inline(never)]
pub(super) fn load(buf: &mut [u8; PACK_MAX]) -> Option<&[u8]> {
    use ferrite_core::part::{self, TERM};
    #[cfg(not(target_os = "none"))]
    if !part::has_reader() {
        return Some(&PACK);
    }
    let n = (part::len(&TERM) as usize).min(PACK_MAX);
    (n > 6 && part::read_body(&TERM, 0, &mut buf[..n]) && buf[..3] == *b"FVT").then_some(&buf[..n])
}

fn u16_at(pk: &[u8], p: usize) -> usize {
    pk.get(p).map_or(0, |&b| b as usize) | pk.get(p + 1).map_or(0, |&b| b as usize) << 8
}

fn string(pk: &[u8], off: usize) -> &[u8] {
    pk.get(off + 2..off + 2 + u16_at(pk, off)).unwrap_or_default()
}

fn page_at(pk: &[u8], pg: u8) -> usize {
    u16_at(pk, 6 + 2 * pk[5] as usize + 2 * pg as usize)
}

/// A string the pages share (the hints while typing, or in help).
pub(super) fn global(pk: &[u8], id: u8) -> &[u8] {
    string(pk, u16_at(pk, 6 + 2 * id as usize))
}

pub(super) const KEYS_TYPE: u8 = STR_KEYS_TYPE;
pub(super) const KEYS_HELP: u8 = STR_KEYS_HELP;

/// The page's hints line (TERM-11) and help (TERM-16).
pub(super) fn hints(pk: &[u8], pg: u8) -> &[u8] {
    string(pk, u16_at(pk, page_at(pk, pg)))
}

pub(super) fn help(pk: &[u8], pg: u8) -> &[u8] {
    string(pk, u16_at(pk, page_at(pk, pg) + 2))
}

/// The bytes an op takes after its code.
fn arg_len(pk: &[u8], q: usize) -> usize {
    match pk[q] {
        0x10 | 0x1B => 1 + pk.get(q + 1).map_or(0, |&b| b as usize),
        0x1C => 2 + pk.get(q + 2).map_or(0, |&b| b as usize),
        0x11 | 0x12 | 0x1D | 0x20 | 0x21 | 0x22 => 1,
        _ => 0,
    }
}

/// From an IF (or ELSE) not taken: past the matching ELSE (`at_else`) or FI.
fn skip(pk: &[u8], mut q: usize, end: usize, at_else: bool) -> usize {
    let mut depth = 0;
    while q < end {
        let op = pk[q];
        let n = 1 + arg_len(pk, q);
        q += n;
        match op {
            0x20..=0x22 => depth += 1,
            0x23 if depth == 0 && at_else => return q,
            0x24 if depth == 0 => return q,
            0x24 => depth -= 1,
            _ => {}
        }
    }
    q
}

impl Term {
    /// The template for what shows (Settings has two: the groups, and a group's items).
    pub(super) fn tpl_page(&self) -> u8 {
        match self.page {
            Page::Off | Page::Splash => PAGE_SPLASH,
            Page::Main => PAGE_MAIN,
            Page::Channels => PAGE_CHANNELS,
            Page::Edit => PAGE_EDIT,
            Page::Zones => PAGE_ZONES,
            Page::Settings if self.g == 0 => PAGE_SETTINGS_TOP,
            Page::Settings => PAGE_SETTINGS_ITEMS,
            Page::Monitor => PAGE_MONITOR,
            Page::Mail => PAGE_MAIL,
            Page::Tnc => PAGE_TNC,
            Page::About => PAGE_ABOUT,
        }
    }

    /// Body row `b` of the page: the first rule that takes it.
    #[inline(never)]
    pub(super) fn tpl_row<S: ChannelStore>(&self, b: u8, cx: &Cx<S>, w: &mut W) {
        if self.page == Page::Off || self.page == Page::Edit && self.edit.is_none() {
            return;
        }
        let pk = cx.pk;
        let mut p = page_at(pk, self.tpl_page()) + 6;
        let body = self.rows as i32 - 2;
        let mut end = 0u16;
        loop {
            let kind = pk.get(p).copied().unwrap_or(0);
            if kind == 0 || p + 4 > pk.len() {
                return;
            }
            let (x, y, len) = (pk[p + 1], pk[p + 2], pk[p + 3] as usize);
            let ops = p + 4;
            p = ops + len;
            let i = match kind {
                1 => (b == x).then_some(0),
                2 => (b as i32 == body - x as i32).then_some(0),
                3 => {
                    end = x as u16 + self.count(y, cx);
                    (b >= x && (b as u16) < end).then(|| b as u16 - x as u16)
                }
                _ => (b as u16 == end + x as u16).then_some(0),
            };
            let Some(mut i) = i else { continue };
            if kind == 3 && y == COUNT_ZONES {
                // the zones list scrolls with its selection
                i += self.zone_sel.saturating_sub(self.page_rows() + 1);
                if i >= cx.store.zones() + 2 {
                    return;
                }
            }
            return self.run(ops, p, i, cx, w);
        }
    }

    fn run<S: ChannelStore>(&self, mut q: usize, end: usize, i: u16, cx: &Cx<S>, w: &mut W) {
        let (pk, mut ch) = (cx.pk, None);
        let end = end.min(pk.len());
        while q < end {
            let op = pk[q];
            let a = pk.get(q + 1).copied().unwrap_or(0);
            let next = (q + 1 + arg_len(pk, q)).min(end);
            match op {
                0x10 => w.s(pk.get(q + 2..next).unwrap_or_default()),
                0x11 => w.at(a as u16),
                0x12 => self.src(a, i, ch.as_ref(), cx, w),
                0x13 => w.bon(),
                0x14 => w.won(),
                0x15 => w.back(),
                0x16 if self.sel(i) => w.rev(),
                0x17 if self.sel(i) => w.fill(),
                0x18 => w.fill(),
                0x19 => w.plain(),
                0x1A => w.digit(i as u8 + 1),
                0x1B => w.s(piece(pk.get(q + 2..next).unwrap_or_default(), b'|', i as usize).unwrap_or_default()),
                0x1C => w.s(piece(pk.get(q + 3..next).unwrap_or_default(), b'|', self.pickn(a, cx) as usize).unwrap_or_default()),
                0x1D => {
                    ch = match a {
                        CTX_ROW => self.row_chan(i, cx.store),
                        CTX_SELECTED => self.selected(cx.store),
                        _ => self.edit,
                    }
                }
                0x20..=0x22 => {
                    let yes = match op {
                        0x22 => i == a as u16,
                        _ => self.cond(a, i, ch.as_ref(), cx) == (op == 0x20),
                    };
                    if !yes {
                        q = skip(pk, next, end, true);
                        continue;
                    }
                }
                0x23 => {
                    q = skip(pk, next, end, false);
                    continue;
                }
                0x25 => return,
                _ => {}
            }
            q = next;
        }
    }

    /// A list's length.
    fn count<S: ChannelStore>(&self, id: u8, cx: &Cx<S>) -> u16 {
        match id {
            COUNT_MAIN => super::MAIN.len() as u16,
            COUNT_SLOTS => SLOTS as u16,
            COUNT_PAGE_ROWS => self.page_rows(),
            COUNT_FIELDS => FIELDS.len() as u16,
            COUNT_ZONES => (cx.store.zones() + 2).min(self.page_rows() + 2),
            COUNT_GROUPS => GROUPS.len() as u16,
            COUNT_ITEMS => groups::items(self.g, self.sub).len() as u16,
            _ => 255,
        }
    }

    /// Whether list row `i` is the selected one.
    fn sel(&self, i: u16) -> bool {
        match self.page {
            Page::Main => i == self.main_sel as u16,
            Page::Channels => self.row_selected(i),
            Page::Edit => i == self.field as u16,
            Page::Zones => i == self.zone_sel,
            Page::Settings => i == self.set_sel as u16,
            _ => false,
        }
    }

    fn pickn<S: ChannelStore>(&self, id: u8, _cx: &Cx<S>) -> u8 {
        match id {
            PICKN_SORT => self.sort % list::SORTS.len() as u8,
            _ => match FIELDS[self.field as usize % FIELDS.len()] {
                F::Name => 0,
                F::Rx => 1,
                F::Tx => 2,
                _ => 3,
            },
        }
    }

    fn cond<S: ChannelStore>(&self, id: u8, i: u16, ch: Option<&Channel>, cx: &Cx<S>) -> bool {
        let st = cx.store;
        let c = ch.copied().unwrap_or(BLANK);
        match id {
            COND_MARKS => self.marks_n() > 0,
            COND_CH => ch.is_some(),
            COND_LIST_EMPTY => list_len(self.zone, st) == 0,
            COND_ASKING => self.ask != list::Ask::None,
            COND_TYPING => self.line.is_some(),
            COND_CH_HOME => cx.app.set.home_ch == Some(c.num),
            COND_SEL => self.sel(i),
            COND_NOTE => !self.note.is_empty() && !self.bnote,
            COND_SPLIT => c.cfg.split,
            COND_OFFSET => c.cfg.offset != 0,
            COND_ITEM_SUB => self.sub == 0 && SUBS.iter().any(|(q, _)| *q == (self.g, i as u8 + 1)),
            COND_HAS_ITEM => (self.set_sel as usize) < groups::items(self.g, self.sub).len(),
            COND_SUB => self.sub != 0,
            COND_SSB => cx.dev.selftest.ssb_patch,
            _ => cx.dev.tx_built,
        }
    }

    /// What a source shows.
    #[inline(never)]
    fn src<S: ChannelStore>(&self, id: u8, i: u16, ch: Option<&Channel>, cx: &Cx<S>, w: &mut W) {
        let (app, st) = (cx.app, cx.store);
        let c = ch.copied().unwrap_or(BLANK);
        let mut v = Buf::<24>::new();
        match id {
            SRC_VERSION => w.s(super::version()),
            SRC_CALLSIGN => w.s(app.set.boot_callsign().unwrap_or(b"No callsign set")),
            SRC_COUNT => w.n(st.count() as u32),
            SRC_ZONES => w.n(st.zones() as u32),
            SRC_REGION => w.s(app.set.region_name().as_bytes()),
            SRC_CAL => w.s(groups::cal_text(cx.dev.selftest.cal).as_bytes()),
            SRC_SLOT => slot_line(i as u8, cx, w),
            SRC_STATUS => status(cx, w),
            SRC_ZONE_NAME => w.s(zone_name(self.zone, st).as_bytes()),
            SRC_LIST_LEN => w.n(list_len(self.zone, st) as u32),
            SRC_MARKS_N => w.n(self.marks_n() as u32),
            SRC_CH_NUM => {
                let mut d = [0; 10];
                let num = digits(c.num as u32 + 1, &mut d);
                w.at(4u16.saturating_sub(num.len() as u16));
                w.s(num);
                if self.marked(c.num) {
                    w.s(b"*"); // TERM-14: marked
                }
            }
            SRC_CH_NAME => w.s(c.name()),
            SRC_CH_RX => w.s(mhz5(c.rx).as_bytes()),
            SRC_CH_OFFSET => item(Item::Offset, &c, w),
            SRC_CH_MODE => {
                let mut v = Buf::<16>::new();
                mode_chip(&c.cfg, &mut v);
                w.s(v.as_bytes());
            }
            SRC_CH_TONE => {
                let mut v = Buf::<16>::new();
                tone_text(c.cfg.tx_tone, &mut v);
                w.s(if v.is_empty() { b"-" } else { v.as_bytes() });
            }
            SRC_CH_POWER => item(Item::Power, &c, w),
            SRC_CH_FLAGS => flags(&c, app, w),
            SRC_ASK_TEXT => {
                let mut q = cmd::Msg::new();
                self.ask_text(st, &mut q);
                w.s(q.as_bytes());
            }
            SRC_LINE => w.s(self.line.unwrap_or_default().as_bytes()),
            SRC_CH_N1 => w.n(c.num as u32 + 1),
            SRC_CH_ZONE => w.s(zone_name(c.zone, st).as_bytes()),
            SRC_FIELD_LABEL => w.s(FIELDS[i as usize].label(true).as_bytes()),
            SRC_FIELD_VALUE => {
                let f = FIELDS[i as usize];
                match (f, self.line.filter(|_| self.sel(i))) {
                    (_, Some(l)) => {
                        w.s(l.as_bytes());
                        w.s(b"_");
                    }
                    (F::Zone, _) => w.s(zone_name(c.zone, st).as_bytes()),
                    (F::Rx, _) => {
                        w.s(mhz5(c.rx).as_bytes());
                        w.s(b" MHz");
                    }
                    (F::Tx, _) => {
                        w.s(mhz5(c.cfg.tx_of(c.rx)).as_bytes());
                        w.s(if c.cfg.split { b" MHz, split" } else { b" MHz" });
                    }
                    (_, _) => {
                        let mut v = Buf::<16>::new();
                        f.text(&c, app.set.home_ch, true, &mut v);
                        w.s(v.as_bytes());
                    }
                }
            }
            SRC_SPLIT_TX => w.s(mhz5(c.cfg.offset.max(0) as Freq10).as_bytes()),
            SRC_OFF_TX => w.s(mhz5((c.rx as i64 + c.cfg.offset as i64).max(0) as Freq10).as_bytes()),
            SRC_NOTE => {
                if self.warn {
                    w.warn(self.note.as_bytes());
                } else {
                    w.s(self.note.as_bytes());
                }
            }
            SRC_ZROW_NAME => w.s(zone_name(zone_of(i, st), st).as_bytes()),
            SRC_ZROW_LEN => w.n(list_len(zone_of(i, st), st) as u32),
            SRC_GROUP_NAME => w.s(GROUPS[i as usize].0.as_bytes()),
            SRC_GROUP_DESC => w.s(crate::text::load(GROUPS[i as usize].1).as_bytes()),
            SRC_SET_G => w.n(self.g as u32),
            SRC_GROUP_NAME_G => w.s(GROUPS[(self.g as usize).clamp(1, 9) - 1].0.as_bytes()),
            SRC_SET_SUB => w.n(self.sub as u32),
            SRC_SUB_NAME => w.s(groups::items(self.g, 0).get(self.sub as usize - 1).map_or("", |e| e.name()).as_bytes()),
            SRC_ITEM_NAME => w.s(groups::items(self.g, self.sub)[i as usize].name().as_bytes()),
            SRC_ITEM_VALUE => {
                let (p, pl) = self.path(i as u8);
                let e = groups::items(self.g, self.sub)[i as usize];
                let dot = match groups::wired(app, cx.dev, &p[..pl], &mut v) {
                    Some(d) => d,
                    None => {
                        match &p[..pl] {
                            [2, 1] => {
                                v.push(digits(st.count() as u32, &mut [0; 10]));
                                v.push(b" channels");
                            }
                            _ => v.set(e.value().as_bytes()),
                        }
                        false
                    }
                };
                if let Some(why) = groups::greyed(app, &p[..pl]) {
                    v.set(why.as_bytes());
                }
                w.s(if dot { b"* " } else { b"  " });
                w.s(v.as_bytes());
            }
            SRC_ITEM_SEL_NAME => w.s(groups::items(self.g, self.sub)[self.set_sel as usize].name().as_bytes()),
            SRC_ITEM_SEL_HELP => w.s(groups::items(self.g, self.sub)[self.set_sel as usize].help().map(crate::text::load).as_ref().map_or(&b""[..], |t| t.as_bytes())),
            SRC_PATH_SEL => {
                let (p, pl) = self.path(self.set_sel);
                for x in &p[..pl] {
                    w.s(b" ");
                    w.n(*x as u32);
                }
            }
            _ => self.monitor(i, cx, w),
        }
    }

    /// The monitor's line `k`: the newest events, oldest first, then the slots receiving now.
    fn monitor<S: ChannelStore>(&self, k: u16, cx: &Cx<S>, w: &mut W) {
        let body = self.rows - 2;
        let space = body.saturating_sub(4) as u16;
        let open: [bool; SLOTS] = core::array::from_fn(|i| cx.live.slots[i].squelch_open);
        let nopen = open.iter().filter(|&&o| o).count() as u16;
        let d = cx.store.data();
        let nev = d.events().min(space.saturating_sub(nopen));
        if k < nev {
            // Newest at the bottom: line k shows event nev - 1 - k (event 0 is the newest).
            let Some(e) = d.event(nev - 1 - k) else { return };
            let mut v = Buf::<16>::new();
            super::ago(e.ago_s, &mut v);
            w.at(1);
            w.s(v.as_bytes());
            w.at(11);
            w.s(e.src.name().as_bytes());
            w.at(20);
            w.s(e.name.as_bytes());
            w.at(32);
            w.s(e.text.as_bytes());
        } else if let Some(s) = (0..SLOTS).filter(|&i| open[i]).nth((k - nev) as usize) {
            let l = cx.live.slots[s];
            w.at(1);
            w.s(b"now");
            w.at(11);
            w.s(&[w::LETTERS[s]]);
            w.s(b" ");
            w.s(w::freq3(l.freq).as_bytes());
            w.at(20);
            w.s(b"RX");
            w.at(32);
            w.s(w::s_text(l.rssi).as_bytes());
        } else if k == 0 {
            w.s("  Nothing yet.".as_bytes());
        }
    }

    // ---- Keys ----

    /// A key on the page, by the template's bindings: true when one took it.
    pub(super) fn tpl_key<S: ChannelStore>(&mut self, app: &mut App, k: K, store: &S) -> bool {
        let code = match k {
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
            _ => return false,
        };
        match binding(self.tpl_page(), code) {
            Some((act, arg, cmd, n)) => {
                self.act(app, act, arg, &cmd[..n], code, k, store);
                true
            }
            None => false,
        }
    }

    /// What a key does.
    fn act<S: ChannelStore>(&mut self, app: &mut App, act: u8, arg: usize, s: &[u8], code: u8, k: K, store: &S) {
        let n = self.sel_n(store);
        let cur = self.sel_get();
        match act {
            ACT_PREV => self.sel_set((cur + n - 1) % n.max(1)),
            ACT_NEXT => self.sel_set((cur + 1) % n.max(1)),
            ACT_FIRST => self.sel_set(0),
            ACT_LAST => self.sel_set(n.saturating_sub(1)),
            ACT_DIGITOPEN => {
                let d = (code - b'0') as u16;
                if d <= n {
                    self.sel_set(d - 1);
                    self.open(app, store);
                }
            }
            ACT_OPEN => self.open(app, store),
            ACT_BACK => self.back(),
            ACT_GO => self.go(match arg as u8 {
                PAGE_SPLASH => Page::Splash,
                _ => Page::Main,
            }),
            ACT_LEFT | ACT_RIGHT => {
                let d = if act == ACT_RIGHT { 1 } else { -1 };
                match self.page {
                    Page::Edit => {
                        if let Some(c) = self.edit {
                            let f = FIELDS[self.field as usize % FIELDS.len()];
                            self.change(app, c, f, d, store);
                        }
                    }
                    _ if self.g != 0 => self.settings_change(app, d),
                    _ => {}
                }
            }
            ACT_DEFAULT => match self.page {
                Page::Edit => {
                    if let Some(c) = self.edit {
                        self.default(app, c, FIELDS[self.field as usize % FIELDS.len()]);
                    }
                }
                _ if self.g != 0 => {
                    let (p, pl) = self.path(self.set_sel);
                    if !super::blocked(&p[..pl]) {
                        groups::reset(app, &p[..pl]);
                        self.say("Back to the default".as_bytes(), false);
                    }
                }
                _ => {}
            },
            ACT_CHNAV => self.chan_nav(k, store),
            ACT_ZONENEXT => {
                // The next zone, then no zone, then every channel.
                let z = store.zones();
                self.zone = match self.zone {
                    ALL if z > 0 => 0,
                    ALL => crate::chan::NO_ZONE,
                    crate::chan::NO_ZONE => ALL,
                    x if x + 1 < z => x + 1,
                    _ => crate::chan::NO_ZONE,
                };
                self.list_reset();
            }
            _ => {
                // a line command, as typed after `:` (TERM-12)
                self.cbuf[..s.len()].copy_from_slice(s);
                (self.clen, self.pending) = (s.len() as u8, true);
            }
        }
    }

    /// The selection's list length on this page.
    fn sel_n<S: ChannelStore>(&self, st: &S) -> u16 {
        match self.page {
            Page::Main => super::MAIN.len() as u16,
            Page::Edit => FIELDS.len() as u16,
            Page::Zones => st.zones() + 2,
            Page::Settings if self.g == 0 => GROUPS.len() as u16,
            Page::Settings => groups::items(self.g, self.sub).len() as u16,
            _ => 0,
        }
    }

    fn sel_get(&self) -> u16 {
        match self.page {
            Page::Main => self.main_sel as u16,
            Page::Edit => self.field as u16,
            Page::Zones => self.zone_sel,
            _ => self.set_sel as u16,
        }
    }

    fn sel_set(&mut self, v: u16) {
        match self.page {
            Page::Main => self.main_sel = v as u8,
            Page::Edit => self.field = v as u8,
            Page::Zones => self.zone_sel = v,
            _ => self.set_sel = v as u8,
        }
    }

    /// Enter on the page.
    fn open<S: ChannelStore>(&mut self, app: &mut App, store: &S) {
        match self.page {
            Page::Splash => self.go(Page::Main),
            Page::Main => self.open_main(),
            Page::Channels => {
                if let Some(c) = self.selected(store) {
                    (self.edit, self.field) = (Some(c), 0);
                    self.go(Page::Edit);
                }
            }
            Page::Edit => {
                let Some(c) = self.edit else { return };
                match FIELDS[self.field as usize % FIELDS.len()] {
                    F::Name => self.line = Some(Buf::from(c.name())),
                    F::Rx | F::Tx => self.line = Some(Buf::new()),
                    f => self.change(app, c, f, 1, store),
                }
            }
            Page::Zones => {
                self.zone = zone_of(self.zone_sel, store);
                self.from_zones = true;
                self.list_reset();
                self.go(Page::Channels);
            }
            _ => self.settings_open(app),
        }
    }

    /// Esc on the page.
    fn back(&mut self) {
        match self.page {
            Page::Channels => self.go(if self.from_zones { Page::Zones } else { Page::Main }),
            Page::Edit => {
                self.edit = None;
                self.go(Page::Channels);
            }
            _ => self.settings_up(),
        }
    }

    /// Arrows, pages and the ends in the channel list (number order; a sorted list moves in `list`).
    fn chan_nav<S: ChannelStore>(&mut self, k: K, store: &S) {
        let last = list_len(self.zone, store).saturating_sub(1);
        let pg = self.page_rows();
        self.chan_sel = match k {
            K::Up => self.chan_sel.saturating_sub(1),
            K::Down => (self.chan_sel + 1).min(last),
            K::PgUp => self.chan_sel.saturating_sub(pg),
            K::PgDn => (self.chan_sel + pg).min(last),
            K::Home => 0,
            _ => last,
        };
    }
}

// ---- Sources shared by pages ----

/// "A  VFO  146.520 00  FM-N  T 100.0   RX S7   active".
fn slot_line<S: ChannelStore>(i: u8, cx: &Cx<S>, w: &mut W) {
    let s = cx.app.slot(i);
    let l = cx.live.slots[i as usize];
    w.at(2);
    w.s(&[w::LETTERS[i as usize]]);
    w.at(5);
    // The APRS slot shows what it's on, the APRS frequency, not the channel or VFO it keeps underneath (APRS-21).
    let aprs = cx.app.aprs_slot(i);
    match s.ch.filter(|_| s.on_channel() && !aprs) {
        _ if aprs => w.s(b"APRS"),
        Some(c) => {
            let mut v = Buf::<32>::new();
            put!(v, "CH {} ", Zero(c.num as u32 + 1, 3));
            let n = c.name();
            v.push(&n[..n.len().min(14)]); // "CH 012 " + 14 leaves a space before column 27
            w.s(v.as_bytes());
        }
        None => w.s(b"VFO"),
    }
    w.at(27);
    let f = if l.scanning { l.freq } else { cx.app.tuned_freq(i) };
    w.s(mhz5(f).as_bytes());
    if !aprs {
        w.at(39);
        let mut v = Buf::<16>::new();
        mode_chip(s.cfg(), &mut v);
        w.s(v.as_bytes());
        w.at(46);
        v.clear();
        tone_text(s.cfg().tx_tone, &mut v);
        w.s(v.as_bytes());
    }
    w.at(57);
    if cx.app.slot_off(i) {
        w.s(b"off");
    } else if l.squelch_open {
        w.s(b"RX ");
        w.s(w::s_text(l.rssi).as_bytes());
    } else if l.scanning {
        w.s(b"scanning");
    } else {
        w.s(b"idle");
    }
    if i == cx.app.active() {
        w.at(68);
        w.s(b"active");
    }
}

/// The main page's status line: battery, GPS (with the power manager's state, GPS-26), Bluetooth, Battery Save.
fn status<S: ChannelStore>(cx: &Cx<S>, w: &mut W) {
    let d = cx.dev;
    let mut v = Buf::<80>::new();
    let gps = match d.gps {
        GpsState::Off => "GPS off",
        GpsState::Searching => "GPS searching",
        GpsState::Fix => "GPS fix",
    };
    put!(v, "Battery {}.{} V   {}", d.battery_mv / 1000, d.battery_mv % 1000 / 100, gps);
    match d.gps_phase {
        GpsPhase::Standby(s) if d.gps != GpsState::Off => put!(v, ", standby {} s", s),
        GpsPhase::Waking if d.gps != GpsState::Off => put!(v, ", waking"),
        GpsPhase::Running | GpsPhase::Searching if d.gps != GpsState::Off => put!(v, ", awake"),
        _ => {}
    }
    put!(v, "   Bluetooth {}   Battery Save ", if d.bt { "on" } else { "off" });
    put!(v, "{}", cx.app.bsave);
    w.s(v.as_bytes());
}

fn flags(c: &Channel, app: &App, w: &mut W) {
    let mut first = true;
    for (on, s) in [
        (app.set.home_ch == Some(c.num), &b"home"[..]),
        (c.flags & FLAG_SKIP != 0, b"skip"),
        (c.flags & FLAG_APRS != 0, b"APRS"),
        (c.flags & FLAG_RX_ONLY != 0, b"RX"),
        (c.flags & FLAG_HIDDEN != 0, b"hidden"),
        (c.flags & FLAG_PRIORITY != 0, b"prio"),
    ] {
        if on {
            if !first {
                w.s(b" ");
            }
            first = false;
            w.s(s);
        }
    }
}
