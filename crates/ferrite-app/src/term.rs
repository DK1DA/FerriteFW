//! The terminal and mini CPS over the cable (manual chapter 37, TERM-1 to TERM-20; Term* mock-ups): a VT100
//! interface any terminal program can use at 80 x 24 or more. A third renderer of the radio's state after the
//! screen and the web app: the main menu, channels (list and editor), zones, the settings with the radio's own
//! numbers and validation (TERM-6, TERM-7), a monitor of the event log and live activity, and About.
//!
//! No screen buffer (TERM-19): every row is drawn on demand from the App and the store, and only a hash of each
//! row is kept, so a row is sent again only when what it shows changed (the clock, a slot opening, an edit made
//! on the radio: TERM-10). The state is a few hundred bytes. It never blocks (TERM-4): bytes in through
//! [`Term::feed`], rows out through [`Term::poll`] while the [`Sink`] has room; a row that doesn't fit waits for
//! the next poll.
//!
//! Slice 1 (Oct 4, 2026): the pages. Slice 2: the command line (`:`, TERM-12) and dump/load (TERM-13), in
//! [`cmd`]; a command that answers in several lines (a dump, a load's report, help) uses the console, which
//! streams lines and then waits for Enter. Not yet: marking or colour (TERM-14, 18).

use crate::chan::{Channel, ChannelStore, FLAG_APRS, FLAG_HIDDEN, FLAG_RX_ONLY, NO_ZONE, ZoneId};
pub(crate) use crate::chfield::parse_mhz;
use crate::chfield::{ChField as F, Ctx, Op};
use crate::screens::groups::{self, SUBS};
use crate::settings::CABLE_PORTS;
use crate::slot::Item;
use crate::App;
use ferrite_core::freq::Freq10;
use ferrite_core::msg::{Device, LiveAll};
use ferrite_core::VERSION;
use ferrite_ui::text::{Buf, Zero};

/// Text from a template (in slow flash) and its arguments into a buffer, through the terminal's one formatter
/// ([`cmd::fmt_into`]): no `uwrite!` expansion per call site.
macro_rules! put {
    ($o:expr, $t:literal) => {{
        use $crate::term::cmd::Fmt as _;
        $o.fill($t, &[])
    }};
    ($o:expr, $t:literal, $($a:expr),+ $(,)?) => {{
        use $crate::term::cmd::Fmt as _;
        $o.fill($t, &[$($crate::term::cmd::Arg::from($a)),+])
    }};
}

pub mod cmd;
pub mod debug;
pub(crate) mod list;
mod tpl;

/// Where the terminal's output goes: the cable's transmit buffer.
pub trait Sink {
    /// Bytes that can be put now without blocking.
    fn room(&self) -> usize;
    fn put(&mut self, b: &[u8]);
}

/// Terminal size limits (TERM-1, TERM-15): 80 x 24 at least; larger terminals use what they have, up to these.
pub const MIN_COLS: u8 = 80;
pub const MAX_COLS: u8 = 200;
pub const MIN_ROWS: u8 = 24;
pub const MAX_ROWS: u8 = 64;
/// A lone ESC is the Esc key once nothing follows it for this long (arrows and PF keys start with ESC).
const ESC_MS: u32 = 60;
/// Every row is checked again this often: the clock, live activity, edits made on the radio (TERM-10).
const REFRESH_MS: u32 = 1000;
/// The most a row can take on the wire: its text plus the cursor move and attributes.
const ROW_MAX: usize = MAX_COLS as usize + 48;

/// What the terminal shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Not started: in *auto* the cable waits for a typed Enter (TERM-2).
    Off,
    Splash,
    Main,
    Channels,
    Edit,
    Zones,
    Settings,
    Monitor,
    Mail,
    Tnc,
    About,
}

/// A decoded key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum K {
    Ch(u8),
    Up,
    Down,
    Left,
    Right,
    PgUp,
    PgDn,
    Home,
    End,
    Enter,
    Esc,
    Bs,
    CtrlC,
    CtrlL,
    Pf(u8),
}

/// Where an escape sequence is: none, after ESC, after ESC [, after ESC O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seq {
    None,
    Started,
    Csi,
    Ss3,
}

/// The console (several lines of answer: a dump, a load, help) instead of the pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Con {
    Off,
    /// Clear the screen first.
    Start(Body),
    Run(Body),
    /// Esc or Ctrl-C: say so, then wait.
    Stop,
    /// Over: Enter or Esc goes back to the page.
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    List(cmd::List, u16),
    Dump(cmd::DumpPos),
    /// Lines read, applied and refused; `end` 1 when it ended, 2 when it was stopped.
    Load { line: u16, ok: u16, bad: u16, end: u8 },
    /// A debug command waiting on, or printing, the firmware's answers (TERM-21).
    Debug(debug::Job),
}

fn cmd_trim(s: &[u8]) -> &[u8] {
    let a = s.iter().position(|c| *c != b' ').unwrap_or(s.len());
    let b = s.iter().rposition(|c| *c != b' ').map_or(a, |k| k + 1);
    &s[a..b]
}

/// The channel editor's fields, in order (the radio's editor: EDIT-2, CHAN-4), with their rules in [`crate::chfield`].
const FIELDS: [F; 16] = [
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
const ALL: ZoneId = 0xFFFD;
/// What the main menu's entries open (their names are the template's).
const MAIN: [Page; 7] = [Page::Channels, Page::Zones, Page::Settings, Page::Monitor, Page::Mail, Page::Tnc, Page::About];

pub struct Term {
    page: Page,
    help: bool,
    /// Ctrl-C: say goodbye on the next poll and wait for Enter again.
    bye: bool,
    main_sel: u8,
    /// Channel list: the filter (a zone, [`NO_ZONE`] or [`ALL`]), selection and first row shown.
    zone: ZoneId,
    chan_sel: u16,
    chan_top: u16,
    from_zones: bool,
    zone_sel: u16,
    /// Settings: group (0 = the list of groups), third-level list (0 = none), selection.
    g: u8,
    sub: u8,
    set_sel: u8,
    /// The channel being edited: the terminal's own copy, written through the App on every change.
    edit: Option<Channel>,
    field: u8,
    /// A value being typed (the name, a frequency), when Some.
    line: Option<Buf<24>>,
    /// The note under the page: what a change did or why it was refused.
    note: cmd::Msg,
    warn: bool,
    /// The note is a command's answer: the bottom row shows it until the next key.
    bnote: bool,
    /// The command line (`:`, TERM-12) and `load`'s line (TERM-13) share one buffer; `clast` is the last
    /// command's length while it is still there (Up brings it back).
    cbuf: [u8; cmd::LINE_MAX],
    clen: u8,
    clast: u8,
    /// The command line is open on the bottom row.
    cmd: bool,
    /// A whole line waits in the buffer for the next poll to run it (after the store has caught up).
    pending: bool,
    /// `sat add` (SAT-3): the load's lines are a TLE or OMM CSV, and what the paste holds between them.
    sat: Option<cmd::SatPaste>,
    /// A load's line was longer than the buffer; the line before was ended by CR (its LF is no new line).
    long: bool,
    cr: bool,
    con: Con,
    cols: u8,
    rows: u8,
    /// Rows to check, and the hash of what each row last sent (0 = never sent).
    dirty: u64,
    hash: [u32; MAX_ROWS as usize],
    clear: bool,
    ask_size: bool,
    esc: Seq,
    esc_at: u32,
    num: [u16; 2],
    nn: u8,
    last: u32,
    /// The time of the last poll (ms): debug answers time out against it.
    now: u32,
    /// The channel list's tools (TERM-14, `list`): the sort column (0: number order), marks (a bit per channel
    /// number), a sorted page (channel numbers, its row count, whether it is current, the list's length when it
    /// was filled), the selected row in it and its first channel, what is being asked, the last thing found,
    /// and a change to several channels in progress.
    sort: u8,
    marks: [u8; list::MARK_BYTES],
    vis: [u16; list::VIS],
    vis_n: u8,
    vis_ok: bool,
    vis_len: u16,
    srow: u8,
    first: u16,
    ask: list::Ask,
    find: Buf<16>,
    bulk: Option<list::Bulk>,
    /// Bluetooth (TERM-25): auto start only (the cable port setting is the cable's). While the radio's PIN lock is
    /// on, a Bluetooth terminal asks for the PIN first and does nothing else until it's right (LINK-30, LINK-31):
    /// whether it's needed, given, and the digits typed so far.
    bt: bool,
    pin_need: bool,
    pin_ok: bool,
    pin: [u8; 6],
    pin_n: u8,
}

impl Default for Term {
    fn default() -> Self {
        Self::new()
    }
}

impl Term {
    pub const fn new() -> Self {
        Term {
            page: Page::Off,
            help: false,
            bye: false,
            main_sel: 0,
            zone: ALL,
            chan_sel: 0,
            chan_top: 0,
            from_zones: false,
            zone_sel: 0,
            g: 0,
            sub: 0,
            set_sel: 0,
            edit: None,
            field: 0,
            line: None,
            note: Buf::new(),
            warn: false,
            bnote: false,
            cbuf: [0; cmd::LINE_MAX],
            clen: 0,
            clast: 0,
            cmd: false,
            pending: false,
            sat: None,
            long: false,
            cr: false,
            con: Con::Off,
            cols: MIN_COLS,
            rows: MIN_ROWS,
            dirty: 0,
            hash: [0; MAX_ROWS as usize],
            clear: false,
            ask_size: false,
            esc: Seq::None,
            esc_at: 0,
            num: [0; 2],
            nn: 0,
            last: 0,
            now: 0,
            sort: 0,
            marks: [0; list::MARK_BYTES],
            vis: [0; list::VIS],
            vis_n: 0,
            vis_ok: false,
            vis_len: 0,
            srow: 0,
            first: list::START,
            ask: list::Ask::None,
            bt: false,
            pin_need: false,
            pin_ok: false,
            pin: [0; 6],
            pin_n: 0,
            find: Buf::new(),
            bulk: None,
        }
    }

    /// What it shows now ([`Page::Off`] until it has been started).
    pub fn page(&self) -> Page {
        self.page
    }

    /// The size in use: columns, rows.
    pub fn size(&self) -> (u8, u8) {
        (self.cols, self.rows)
    }

    /// The terminal's size from outside (the web simulator's pane); a cursor position report does the same.
    pub fn set_size(&mut self, cols: u16, rows: u16) {
        self.cols = cols.clamp(MIN_COLS as u16, MAX_COLS as u16) as u8;
        self.rows = rows.clamp(MIN_ROWS as u16, MAX_ROWS as u16) as u8;
        self.redraw();
    }

    /// Rows are still waiting for room in the sink, a line waits to run, or the console is streaming: poll
    /// again soon.
    pub fn busy(&self) -> bool {
        self.page != Page::Off && (self.dirty != 0 || self.clear || self.bye || self.pending || self.bulk.is_some() || matches!(self.con, Con::Start(_) | Con::Run(_)))
    }

    /// It takes another byte now. False while a whole line waits to run (a load waits for the store between
    /// lines): leave the bytes where they are, the cable's flow control holds the sender (TERM-4, TERM-13).
    pub fn ready(&self) -> bool {
        !self.pending
    }

    /// The terminal is open (Enter started it and Ctrl-C hasn't ended it).
    pub fn on(&self) -> bool {
        self.page != Page::Off
    }

    /// Which port this terminal runs on: Bluetooth starts on Enter whatever the cable port setting says (TERM-25).
    pub fn set_bluetooth(&mut self, bt: bool) {
        self.bt = bt;
    }

    /// LINK-30: this terminal must be given the radio's PIN before anything else (Bluetooth with the PIN lock on).
    /// Set each poll; the PIN given lasts for this terminal (a new connection opens a new one).
    pub fn set_pin_needed(&mut self, need: bool) {
        self.pin_need = need;
    }

    /// Open and still waiting for the PIN (LINK-30).
    fn pin_gate(&self) -> bool {
        self.page != Page::Off && self.pin_need && !self.pin_ok
    }

    /// LINK-31: one byte typed at the PIN prompt: digits (shown as *), Backspace, Enter checks it against the
    /// radio's PIN with its counter and wait, Ctrl-C leaves. Nothing else is read.
    fn pin_byte(&mut self, app: &mut App, b: u8) {
        match b {
            b'0'..=b'9' if (self.pin_n as usize) < self.pin.len() => {
                self.pin[self.pin_n as usize] = b;
                self.pin_n += 1;
            }
            0x08 | 0x7F => self.pin_n = self.pin_n.saturating_sub(1),
            b'\r' | b'\n' if self.pin_n > 0 => {
                let n = core::mem::take(&mut self.pin_n) as usize;
                match app.remote_pin(&self.pin[..n]) {
                    crate::screens::pin::RemotePin::Ok => {
                        self.pin_ok = true;
                        self.note.clear();
                        self.redraw();
                        return;
                    }
                    crate::screens::pin::RemotePin::Wrong => self.say("Wrong PIN".as_bytes(), true),
                    crate::screens::pin::RemotePin::Wait(w) => {
                        let mut b = Buf::<40>::new();
                        put!(b, "Too many tries: wait {} s", w);
                        self.say(b.as_bytes(), true);
                    }
                }
                self.pin = [0; 6];
            }
            0x03 => self.bye = true,
            _ => {}
        }
        self.all_dirty();
    }

    /// Everything again: clear the screen, forget what each row showed (Ctrl-L, TERM-16).
    fn redraw(&mut self) {
        self.clear = true;
        self.hash = [0; MAX_ROWS as usize];
        self.all_dirty();
    }

    fn all_dirty(&mut self) {
        self.dirty = if self.rows >= 64 { u64::MAX } else { (1u64 << self.rows) - 1 };
    }

    fn start(&mut self) {
        *self = Term { cols: self.cols, rows: self.rows, bt: self.bt, pin_need: self.pin_need, ..Term::new() };
        self.page = Page::Splash;
        self.ask_size = true;
        self.redraw();
    }

    fn go(&mut self, p: Page) {
        self.page = p;
        self.help = false;
        self.line = None;
        self.note.clear();
        (self.warn, self.bnote) = (false, false);
    }

    fn say(&mut self, s: &[u8], warn: bool) {
        self.note.set(s);
        (self.warn, self.bnote) = (warn, false);
    }

    // ---- Input ----

    /// One byte from the cable, `now` in ms. Call the App's `pop_write` after each byte: a channel edit writes
    /// one channel per key.
    pub fn feed<S: ChannelStore>(&mut self, app: &mut App, b: u8, now: u32, store: &S) {
        if self.page == Page::Off {
            // TERM-2: auto starts on a typed Enter; terminal on any byte; link and KISS never. COBS link frames
            // (the 0x00 delimiter) select the link protocol in milestone 3.
            let go = match if self.bt { 0 } else { app.set.cable_port } {
                0 => b == b'\r' || b == b'\n',
                2 => true,
                _ => false,
            };
            if go {
                self.start();
            }
            return;
        }
        if self.pin_gate() {
            return self.pin_byte(app, b);
        }
        if let Con::Run(Body::Load { .. }) = self.con {
            return self.load_byte(b);
        }
        let k = match (self.esc, b) {
            (Seq::None, 0x1B) => {
                (self.esc, self.esc_at) = (Seq::Started, now);
                return;
            }
            (Seq::Started, b'[') => {
                (self.esc, self.num, self.nn) = (Seq::Csi, [0; 2], 0);
                return;
            }
            (Seq::Started, b'O') => {
                self.esc = Seq::Ss3;
                return;
            }
            (Seq::Started, _) => {
                // ESC then something else: the Esc key, then that byte.
                self.esc = Seq::None;
                self.key(app, K::Esc, store);
                return self.feed(app, b, now, store);
            }
            (Seq::Csi, b'0'..=b'9') => {
                let i = (self.nn as usize).min(1);
                self.num[i] = self.num[i].saturating_mul(10).saturating_add((b - b'0') as u16);
                return;
            }
            (Seq::Csi, b';') => {
                self.nn = self.nn.saturating_add(1);
                return;
            }
            (Seq::Csi, _) => {
                self.esc = Seq::None;
                match b {
                    b'A' => K::Up,
                    b'B' => K::Down,
                    b'C' => K::Right,
                    b'D' => K::Left,
                    b'H' => K::Home,
                    b'F' => K::End,
                    b'R' => {
                        // The cursor position report: the terminal's size (TERM-15).
                        let (r, c) = (self.num[0], self.num[1]);
                        if r > 0 && c > 0 {
                            self.set_size(c, r);
                        }
                        return;
                    }
                    b'~' => match self.num[0] {
                        1 | 7 => K::Home,
                        4 | 8 => K::End,
                        5 => K::PgUp,
                        6 => K::PgDn,
                        11..=14 => K::Pf((self.num[0] - 10) as u8),
                        _ => return,
                    },
                    _ => return,
                }
            }
            (Seq::Ss3, _) => {
                self.esc = Seq::None;
                match b {
                    b'P'..=b'S' => K::Pf(b - b'P' + 1),
                    b'A' => K::Up,
                    b'B' => K::Down,
                    b'C' => K::Right,
                    b'D' => K::Left,
                    b'H' => K::Home,
                    b'F' => K::End,
                    b'M' => K::Enter,
                    _ => return,
                }
            }
            (Seq::None, b'\r') => K::Enter,
            (Seq::None, b'\n') => return, // CR LF terminals: the CR was the Enter
            (Seq::None, 0x08 | 0x7F) => K::Bs,
            (Seq::None, 0x03) => K::CtrlC,
            (Seq::None, 0x0C) => K::CtrlL,
            (Seq::None, 0x20..=0x7E) => K::Ch(b),
            _ => return,
        };
        self.key(app, k, store);
    }

    /// A byte of a load (TERM-13): lines, not keys. Ctrl-C stops, Ctrl-D or a line `end` finishes.
    fn load_byte(&mut self, b: u8) {
        let Con::Run(Body::Load { line, ok, bad, .. }) = self.con else { return };
        let end = |t: &mut Term, how: u8| t.con = Con::Run(Body::Load { line, ok, bad, end: how });
        let was_cr = core::mem::replace(&mut self.cr, b == b'\r');
        match b {
            0x03 => end(self, 2),
            0x04 => end(self, 1),
            b'\n' if was_cr => {}
            b'\r' | b'\n' => {
                if cmd_trim(&self.cbuf[..self.clen as usize]) == b"end" {
                    self.clen = 0;
                    end(self, 1);
                } else {
                    self.pending = true;
                }
            }
            b'\t' | 0x20..=0x7E => {
                if (self.clen as usize) < cmd::LINE_MAX {
                    self.cbuf[self.clen as usize] = if b == b'\t' { b' ' } else { b };
                    self.clen += 1;
                } else {
                    self.long = true;
                }
            }
            _ => {}
        }
    }

    /// Keys on the console: Enter or Esc leaves it when it is done; Esc or Ctrl-C stops a list or dump.
    fn con_key(&mut self, k: K) {
        match (self.con, k) {
            (Con::Done, K::Enter | K::Esc | K::CtrlC) => {
                self.con = Con::Off;
                self.redraw();
            }
            (Con::Start(_) | Con::Run(_), K::Esc | K::CtrlC) => self.con = Con::Stop,
            (_, K::CtrlL) => {}
            _ => {}
        }
    }

    /// Keys on the command line.
    fn cmd_key(&mut self, k: K) {
        match k {
            K::Enter => {
                self.cmd = false;
                if self.clen > 0 {
                    (self.pending, self.clast) = (true, self.clen);
                }
            }
            K::Esc => self.cmd = false,
            K::Bs => self.clen = self.clen.saturating_sub(1),
            K::Up if self.clen == 0 => self.clen = self.clast,
            K::Ch(c) if (self.clen as usize) < cmd::LINE_MAX => {
                if self.clen < self.clast {
                    self.clast = 0; // typing over the last command: Up has nothing to bring back
                }
                self.cbuf[self.clen as usize] = c;
                self.clen += 1;
            }
            _ => {}
        }
    }

    fn key<S: ChannelStore>(&mut self, app: &mut App, k: K, store: &S) {
        self.all_dirty();
        if self.con != Con::Off {
            return self.con_key(k);
        }
        self.bnote = false;
        if self.cmd && k != K::CtrlC {
            return self.cmd_key(k);
        }
        match k {
            K::CtrlC => {
                self.bye = true;
                return;
            }
            K::CtrlL => return self.redraw(),
            _ => {}
        }
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

    fn open_main(&mut self) {
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
    fn page_rows(&self) -> u16 {
        (self.rows as u16).saturating_sub(8).max(1)
    }

    fn line_max(&self) -> usize {
        if self.ask != list::Ask::None {
            return 16;
        }
        match FIELDS[self.field as usize % FIELDS.len()] {
            F::Name => 16,
            _ => 10,
        }
    }

    /// ◀▶ on a field: the same steps and checks as the radio's channel editor (TERM-6, TERM-7).
    fn change<S: ChannelStore>(&mut self, app: &mut App, c: Channel, f: F, d: i32, store: &S) {
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
    fn default(&mut self, app: &mut App, c: Channel, f: F) {
        self.apply(app, c, f, Op::Reset, 0, &crate::chfield::any_room);
    }

    /// A typed value: the name, or the RX or split TX frequency in MHz.
    fn commit(&mut self, app: &mut App, s: &[u8]) {
        let Some(c) = self.edit else { return };
        let f = FIELDS[self.field as usize % FIELDS.len()];
        self.apply(app, c, f, Op::Typed(s), 0, &crate::chfield::any_room);
    }

    /// A field's change through its rules: saved, Home set, or the refusal said.
    fn apply(&mut self, app: &mut App, c: Channel, f: F, op: Op, zones: ZoneId, room: &dyn Fn(ZoneId) -> bool) {
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

    fn save(&mut self, app: &mut App, n: Channel) {
        if list::radio_channel(app) == Some(n.num) {
            // TERM-17: the radio's editor has it; its copy wins and shows here.
            return self.say("Open in the radio's editor: change it there".as_bytes(), true);
        }
        if Some(n) != self.edit {
            app.put_channel(n);
            self.edit = Some(n);
            self.say(b"Saved", false);
        }
    }

    fn settings_up(&mut self) {
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
    fn path(&self, row: u8) -> ([u8; 3], usize) {
        if self.sub == 0 { ([self.g, row + 1, 0], 2) } else { ([self.g, self.sub, row + 1], 3) }
    }

    fn settings_open(&mut self, app: &mut App) {
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

    fn settings_change(&mut self, app: &mut App, d: i32) {
        let (p, pl) = self.path(self.set_sel);
        if list::radio_setting(app) == Some((p, pl)) {
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

    // ---- Output ----

    /// A debug command's next request for the firmware (TERM-21), once: memory is read by the caller, the
    /// BK4829 and SI4732 requests go to the Radio task. Answer with [`Term::debug_answer`].
    pub fn debug_req(&mut self) -> Option<ferrite_core::msg::DebugReq> {
        match &mut self.con {
            Con::Run(Body::Debug(j)) => j.request(self.now),
            _ => None,
        }
    }

    /// The firmware's answer to the last [`Term::debug_req`]; ignored once the command has been stopped.
    pub fn debug_answer(&mut self, a: ferrite_core::msg::DebugAns) {
        if let Con::Run(Body::Debug(j)) = &mut self.con {
            j.answer(a);
        }
    }

    /// Sends what changed while the sink has room: the goodbye, a clear, then each row whose text changed.
    #[inline(never)] // its own stack frame, not the caller's for its whole run (SBAR-23)
    pub fn poll<S: ChannelStore>(&mut self, app: &mut App, store: &S, dev: &Device, live: &LiveAll, now: u32, out: &mut impl Sink) {
        self.now = now;
        if self.esc == Seq::Started && now.wrapping_sub(self.esc_at) >= ESC_MS {
            self.esc = Seq::None;
            self.key(app, K::Esc, store);
        }
        if self.page == Page::Off {
            return;
        }
        if self.bye {
            if out.room() < 96 {
                return;
            }
            out.put(b"\x1b[0m\x1b[2J\x1b[H");
            out.put("Ferrite terminal closed. Press Enter to open it again.\r\n".as_bytes());
            *self = Term { cols: self.cols, rows: self.rows, ..Term::new() };
            return;
        }
        if now.wrapping_sub(self.last) >= REFRESH_MS {
            self.last = now;
            self.all_dirty();
        }
        if self.ask_size {
            if out.room() < 32 {
                return;
            }
            // Save the cursor, go far down right, ask where it is, come back (TERM-15).
            out.put(b"\x1b7\x1b[999;999H\x1b[6n\x1b8");
            self.ask_size = false;
        }
        if self.clear && self.con == Con::Off {
            if out.room() < 16 {
                return;
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
        if let Some(c) = self.edit.filter(|c| self.page == Page::Edit && list::radio_channel(app) == Some(c.num)) {
            self.edit = store.get(c.num).or(Some(c));
        }
        // A whole line: run it once the store has taken the last one's writes (TERM-13 never drops a line).
        if self.pending && !app.write_pending() && !store.data().busy() && out.room() >= 2 * ROW_MAX {
            self.pending = false;
            self.exec(app, store, dev, out);
        }
        if self.con != Con::Off {
            return self.console(app, store, dev, live, out);
        }
        let cx = Cx { app, store, dev, live };
        for r in 0..self.rows {
            if self.dirty & (1 << r) == 0 {
                continue;
            }
            let mut h = Hash(0x811C_9DC5);
            let colour = app.set.term_colour; // TERM-18, 0 9 9 4
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

    /// Runs the line in the buffer: a typed command, or a load's next line (its result and the count go out).
    fn exec<S: ChannelStore>(&mut self, app: &mut App, store: &S, dev: &Device, out: &mut impl Sink) {
        let n = self.clen as usize;
        self.clen = if self.con == Con::Off { self.clen } else { 0 };
        if let Con::Run(Body::Load { mut line, mut ok, mut bad, end }) = self.con {
            line += 1;
            let r = if core::mem::take(&mut self.long) {
                Err(cmd::Msg::from("too long: 255 characters at most".as_bytes()))
            } else if let Some(p) = self.sat.as_mut() {
                cmd::sat_line(store, p, &self.cbuf[..n])
            } else {
                cmd::run(app, store, dev, &self.cbuf[..n], true)
            };
            match r {
                Ok(cmd::Done::Nothing) => {}
                Ok(_) => ok += 1,
                Err(m) => {
                    bad += 1;
                    let mut l = cmd::Line::new();
                    put!(l, "\r\x1b[K  line {}: ", line);
                    l.push(m.as_bytes());
                    out.put(l.as_bytes());
                    out.put(b"\r\n");
                }
            }
            self.con = Con::Run(Body::Load { line, ok, bad, end });
            let mut l = Buf::<64>::new();
            put!(l, "\r\x1b[K  {} lines read: {} applied, {} refused", line, ok, bad);
            out.put(l.as_bytes());
            return;
        }
        let (m, warn) = match cmd::run(app, store, dev, &self.cbuf[..n], false) {
            Ok(cmd::Done::Nothing) => return,
            Ok(cmd::Done::Said(m)) => (m, false),
            Ok(cmd::Done::List(l)) => return self.con = Con::Start(Body::List(l, 0)),
            Ok(cmd::Done::Dump) => return self.con = Con::Start(Body::Dump(cmd::DumpPos::default())),
            Ok(cmd::Done::Debug(j)) => return self.con = Con::Start(Body::Debug(j)),
            Ok(d @ (cmd::Done::Load | cmd::Done::SatPaste)) => {
                // The buffer takes the load's lines from here: the command that started it is gone.
                (self.clen, self.clast, self.cr, self.long) = (0, 0, false, false);
                self.sat = matches!(d, cmd::Done::SatPaste).then(cmd::SatPaste::default);
                return self.con = Con::Start(Body::Load { line: 0, ok: 0, bad: 0, end: 0 });
            }
            Err(m) => (m, true),
        };
        (self.note, self.warn, self.bnote) = (m, warn, true);
        self.all_dirty();
    }

    /// The console: a clear screen, then lines while the sink has room; then "Enter returns".
    fn console<S: ChannelStore>(&mut self, app: &App, store: &S, dev: &Device, live: &LiveAll, out: &mut impl Sink) {
        const LINE_ROOM: usize = 300;
        match self.con {
            Con::Off | Con::Done => {}
            Con::Start(b) => {
                if out.room() < LINE_ROOM {
                    return;
                }
                out.put(b"\x1b[0m\x1b[2J\x1b[H");
                if let (Body::Load { .. }, Some(_)) = (b, self.sat) {
                    out.put("Paste satellites: a TLE (name, line 1, line 2) or OMM CSV lines. One with a catalogue number\r\nheld gets the new elements; another is added. It ends at \"end\" or Ctrl-D; Ctrl-C stops.\r\n\r\n".as_bytes());
                } else if let Body::Load { .. } = b {
                    out.put("Load: send the dump now. A bad line is skipped and named. It ends at \"end\" or Ctrl-D;\r\nCtrl-C stops. Use XON/XOFF flow control, or a short delay per line.\r\n\r\n".as_bytes());
                }
                self.con = Con::Run(b);
                self.console(app, store, dev, live, out);
            }
            Con::Run(Body::Load { line, ok, bad, end }) => {
                if end != 0 && !self.pending && out.room() >= LINE_ROOM {
                    let mut l = cmd::Line::new();
                    let how = if end == 2 { "Stopped" } else { "Done" };
                    self.sat = None;
                    put!(l, "\r\x1b[K  {}: {} lines read, {} applied, {} refused.\r\n", how, line, ok, bad);
                    out.put(l.as_bytes());
                    self.done(out);
                }
            }
            Con::Run(mut b) => {
                while out.room() >= LINE_ROOM {
                    let mut l = cmd::Line::new();
                    let more = match &mut b {
                        Body::List(what, i) => {
                            let m = cmd::list_line(app, store, dev, live, *what, *i, &mut l);
                            *i += 1;
                            m
                        }
                        Body::Dump(p) => cmd::dump_line(app, store, dev, p, &mut l),
                        Body::Load { .. } => false,
                        Body::Debug(j) => match j.line(self.now, &mut l) {
                            debug::Step::Line => true,
                            debug::Step::Wait => break,
                            debug::Step::Over => false,
                        },
                    };
                    if !more {
                        return self.done(out);
                    }
                    out.put(l.as_bytes());
                    out.put(b"\r\n");
                }
                self.con = Con::Run(b);
            }
            Con::Stop => {
                if out.room() >= LINE_ROOM {
                    out.put("\r\n  Stopped.\r\n".as_bytes());
                    self.pending = false;
                    self.sat = None;
                    self.done(out);
                }
            }
        }
    }

    fn done(&mut self, out: &mut impl Sink) {
        out.put("\r\n  Enter or Esc returns to the menu.".as_bytes());
        (self.con, self.clen) = (Con::Done, 0);
    }

    fn row<S: ChannelStore>(&self, r: u8, cx: &Cx<S>, w: &mut W) {
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
            return self.hints(w);
        }
        let b = r - 1;
        if self.help {
            return self.help_row(b, w);
        }
        self.tpl_row(b, cx, w);
    }

    /// The top line (TERM-11): firmware, callsign and model; the link in the middle; the clock at the right.
    fn top<S: ChannelStore>(&self, cx: &Cx<S>, w: &mut W) {
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
    fn hints(&self, w: &mut W) {
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
            tpl::global(tpl::KEYS_TYPE)
        } else if self.help {
            tpl::global(tpl::KEYS_HELP)
        } else {
            tpl::hints(self.tpl_page())
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

    fn help_row(&self, b: u8, w: &mut W) {
        let lines = tpl::help(self.tpl_page());
        if b == 1 {
            w.s(b" ");
            w.bold(b"Help");
        } else if let Some(l) = (b as usize).checked_sub(3).and_then(|i| piece(lines, b'\n', i)) {
            w.s(b"  ");
            w.s(l);
        }
    }

}

fn item(it: Item, c: &Channel, w: &mut W) {
    let mut v = Buf::<16>::new();
    it.text(&c.cfg, false, &mut v);
    w.s(v.as_bytes());
}

// ---- Helpers ----

/// Settings the terminal leaves to the radio: they open their own screens there (the PIN, Extend TX's warning)
/// or the cable may not change them (the region: TERM-7; Dry TX, TX-48: the PC never decides whether a key-up has
/// power).
fn blocked(p: &[u8]) -> bool {
    matches!(p, [9, 3] | [9, 3, 1] | [8, 8, 4] | [8, 8, 5] | [9, 4])
}

/// The `i`-th piece of `t` cut at each `sep` (a page's hints and help lines).
#[inline(never)]
fn piece(mut t: &[u8], sep: u8, i: usize) -> Option<&[u8]> {
    for _ in 0..i {
        t = &t[t.iter().position(|&c| c == sep)? + 1..];
    }
    Some(&t[..t.iter().position(|&c| c == sep).unwrap_or(t.len())])
}

/// "0.1.0" from "v0.1.0".
fn version() -> &'static [u8] {
    VERSION.strip_prefix('v').unwrap_or(VERSION).as_bytes()
}

/// "146.52000": MHz with five decimals (10 Hz steps), as the channel list shows it.
fn mhz5(f: Freq10) -> Buf<12> {
    let mut b = Buf::new();
    put!(b, "{}.{}", f / 100_000, Zero(f % 100_000, 5));
    b
}

/// "12 s ago", "3 min ago", "2 h ago".
fn ago(s: u32, b: &mut Buf<16>) {
    match s {
        crate::data::AGO_UNKNOWN => put!(b, "earlier"), // INBOX-6.1
        0..=59 => put!(b, "{} s ago", s),
        60..=3599 => put!(b, "{} min ago", s / 60),
        _ => put!(b, "{} h ago", s / 3600),
    };
}

/// The zones list's i-th entry: every channel, each zone, then no zone.
fn zone_of<S: ChannelStore>(i: u16, st: &S) -> ZoneId {
    match i {
        0 => ALL,
        _ if i <= st.zones() => i - 1,
        _ => NO_ZONE,
    }
}

fn zone_name<S: ChannelStore>(z: ZoneId, st: &S) -> Buf<16> {
    if z == ALL {
        return Buf::from(b"All channels");
    }
    st.zone_label(z, b"No zone", b"?")
}

fn list_len<S: ChannelStore>(z: ZoneId, st: &S) -> u16 {
    if z == ALL { st.count() } else { st.zone_len(z) }
}

fn chan_at<S: ChannelStore>(z: ZoneId, i: u16, st: &S) -> Option<Channel> {
    if z == ALL { st.by_number(i) } else { st.zone_at(z, i) }
}

/// The cable port setting's names (0 9 2 3), for the radio's menu too.
pub fn port_name(p: u8) -> &'static str {
    CABLE_PORTS[p as usize % CABLE_PORTS.len()]
}

// ---- Writing a row ----

struct Cx<'a, S> {
    app: &'a App,
    store: &'a S,
    dev: &'a Device,
    live: &'a LiveAll,
}

/// Where a row's bytes go: the hash that tells whether it changed, or the sink.
trait Out {
    fn out(&mut self, b: &[u8]);
}

/// FNV-1a over the row's bytes, attributes included.
struct Hash(u32);

impl Out for Hash {
    fn out(&mut self, b: &[u8]) {
        for &c in b {
            self.0 = (self.0 ^ c as u32).wrapping_mul(0x0100_0193);
        }
    }
}

struct SinkOut<'a, T: Sink>(&'a mut T);

impl<T: Sink> Out for SinkOut<'_, T> {
    fn out(&mut self, b: &[u8]) {
        self.0.put(b);
    }
}

/// `n` in decimal, written into the end of `d`: one formatter for every number the terminal shows.
#[inline(never)]
pub(crate) fn digits(mut n: u32, d: &mut [u8; 10]) -> &[u8] {
    let mut i = d.len();
    loop {
        i -= 1;
        d[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            return &d[i..];
        }
    }
}

/// One row: text clipped to the width, columns counted (attributes take none), reverse video and bold only
/// (TERM-1: plain VT100); with `colour` (TERM-18, setting 0 9 9 4), ANSI colours in their place: the top line
/// and the selection white on blue, headings cyan, warnings yellow.
struct W<'a> {
    o: &'a mut dyn Out,
    col: u16,
    cols: u16,
    rev: bool,
    colour: bool,
}

impl W<'_> {
    /// Text, printable ASCII; anything else shows as `?` ("·" as a dot).
    fn s(&mut self, b: &[u8]) {
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
    fn n(&mut self, n: u32) {
        self.s(digits(n, &mut [0; 10]));
    }

    fn digit(&mut self, d: u8) {
        self.s(&[b'0' + d % 10]);
    }

    /// Spaces up to column `c`.
    fn at(&mut self, c: u16) {
        while self.col < c.min(self.cols) {
            self.o.out(b" ");
            self.col += 1;
        }
    }

    /// Spaces to the end of the row (a reverse-video bar or selection runs the whole width).
    fn fill(&mut self) {
        self.at(self.cols);
    }

    fn rev(&mut self) {
        self.o.out(if self.colour { b"\x1b[0;97;44m" } else { b"\x1b[7m" });
        self.rev = true;
    }

    /// What the attributes go back to after bold or a warning.
    fn back(&mut self) {
        self.o.out(match (self.rev, self.colour) {
            (true, true) => b"\x1b[0;97;44m",
            (true, false) => b"\x1b[0m\x1b[7m",
            (false, _) => b"\x1b[0m",
        });
    }

    /// A warning: bold, or yellow in colour.
    fn warn(&mut self, b: &[u8]) {
        self.won();
        self.s(b);
        self.back();
    }

    /// A warning's attributes on.
    fn won(&mut self) {
        if !self.colour {
            return self.bon();
        }
        self.o.out(if self.rev { b"\x1b[1;93;44m" } else { b"\x1b[1;33m" });
    }

    /// Bold on (cyan in colour, heading colours on the selection).
    fn bon(&mut self) {
        self.o.out(match (self.rev, self.colour) {
            (true, true) => b"\x1b[1;97;44m",
            (false, true) => b"\x1b[1;36m",
            _ => b"\x1b[1m",
        });
    }

    fn plain(&mut self) {
        self.o.out(b"\x1b[0m");
        self.rev = false;
    }

    fn bold(&mut self, b: &[u8]) {
        self.bon();
        self.s(b);
        // VT100 has no "bold off": all off, then reverse (or the colours) again if it was on.
        self.back();
    }
}

#[cfg(test)]
mod tests;
