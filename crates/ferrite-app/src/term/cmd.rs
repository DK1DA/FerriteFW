//! The command line (TERM-12) and dump/load (TERM-13). One command per line, the same whether typed after `:`
//! or read back by `load`: a dump is a script of `set`, `zone`, `preset`, `ch` and `home` lines, so whatever
//! `load` accepts can also be typed, and every line is checked as the radio's own editors check it (TERM-6,
//! TERM-7). A line is applied whole or not at all.
//!
//! Nothing is buffered whole: [`dump_line`] makes one line at a time from where [`DumpPos`] says it is.

use super::{blocked, mhz5, parse_mhz};
use crate::chan::{Channel, ChannelStore, Name, FLAG_APRS, FLAG_HIDDEN, FLAG_PRIORITY, FLAG_RX_ONLY, FLAG_SKIP, MAX_CHANNELS, NO_ZONE};
use crate::data::{Op, Pos, RangePreset, ZoneCfg};
use crate::screens::groups::{self, SUBS};
use crate::screens::tones::{CTCSS_MAX, CTCSS_MIN};
use crate::chfield::{self, ChField, Refusal};
use crate::slot::{Item, SLOT_MODES, DCS_FLAG, DCS_INV, DEFAULT_CFG, MODES, SQ_GLOBAL, STEPS, STEP_12K5};
use crate::App;
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::msg::Device;
use ferrite_core::VERSION;
use ferrite_ui::text::{Buf, Bytes};

/// The longest line read (TERM-12, TERM-13: a 256-byte line buffer).
pub const LINE_MAX: usize = 255;
/// A line written: a channel at its longest is under 200 characters.
pub type Line = Buf<256>;
/// A command's answer, or why it was refused.
pub type Msg = Buf<96>;

/// A message from its template (in slow flash) and arguments: every answer and refusal goes through the one
/// [`fmt`], so a call site is a pointer and its arguments rather than its own formatting code.
macro_rules! msg {
    ($t:literal) => {
        fmt($t, &[])
    };
    ($t:literal, $($a:expr),+ $(,)?) => {
        fmt($t, &[$(Arg::from($a)),+])
    };
}

/// A message's argument: a number, text as written, or bytes from the user (non-printing ones shown as `?`).
#[derive(Clone, Copy)]
pub(crate) enum Arg<'a> {
    N(u32),
    /// A number zero-padded to this many digits (`Zero`).
    Z(u32, u8),
    S(&'a str),
    B(&'a [u8]),
}

impl From<ferrite_ui::text::Zero> for Arg<'_> {
    fn from(z: ferrite_ui::text::Zero) -> Self {
        Arg::Z(z.0, z.1 as u8)
    }
}

/// Where formatted text goes: any buffer (a line, a message, a status field).
pub(crate) trait Out {
    fn put(&mut self, b: &[u8]);
}

impl<const N: usize> Out for Buf<N> {
    fn put(&mut self, b: &[u8]) {
        self.push(b);
    }
}

/// `put!`'s entry: a method, so a call site can name an owned buffer or a `&mut` one alike.
pub(crate) trait Fmt: Out + Sized {
    fn fill(&mut self, t: &str, args: &[Arg]) {
        fmt_into(self, t, args)
    }
}

impl<T: Out> Fmt for T {}

macro_rules! arg_num {
    ($($t:ty),*) => {$(
        impl From<$t> for Arg<'_> {
            fn from(n: $t) -> Self {
                Arg::N(n as u32)
            }
        }
    )*};
}
arg_num!(u8, u16, u32, usize);

impl<'a> From<&'a str> for Arg<'a> {
    fn from(s: &'a str) -> Self {
        Arg::S(s)
    }
}

impl<'a> From<Bytes<'a>> for Arg<'a> {
    fn from(b: Bytes<'a>) -> Self {
        Arg::B(b.0)
    }
}

/// `pre`, then `n` in decimal.
fn num(out: &mut Line, pre: &[u8], n: u32) {
    out.push(pre);
    out.push(super::digits(n, &mut [0; 10]));
}

/// Fills each `{}` in `t` with the next argument, into a message.
fn fmt(t: &str, args: &[Arg]) -> Msg {
    let mut m = Msg::new();
    fmt_into(&mut m, t, args);
    m
}

/// Fills each `{}` in `t` with the next argument, into `m`: the one formatter of the terminal, so a call site
/// ([`msg!`], `put!`) is a template pointer and its arguments rather than formatting code of its own.
#[inline(never)]
pub(crate) fn fmt_into(m: &mut dyn Out, t: &str, args: &[Arg]) {
    let (t, mut k, mut i) = (t.as_bytes(), 0, 0);
    while i < t.len() {
        if t[i] == b'{' && t.get(i + 1) == Some(&b'}') {
            match args.get(k) {
                Some(Arg::N(n)) => m.put(super::digits(*n, &mut [0; 10])),
                Some(Arg::Z(n, w)) => {
                    let d = super::digits(*n, &mut [0; 10]).len();
                    for _ in d..*w as usize {
                        m.put(b"0");
                    }
                    m.put(super::digits(*n, &mut [0; 10]));
                }
                Some(Arg::S(s)) => m.put(s.as_bytes()),
                Some(Arg::B(b)) => {
                    for &c in *b {
                        m.put(&[if (0x20..0x7F).contains(&c) { c } else { b'?' }]);
                    }
                }
                None => {}
            }
            k += 1;
            i += 2;
        } else {
            // a run of plain text in one call
            let j = t[i + 1..].iter().position(|&c| c == b'{').map_or(t.len(), |p| i + 1 + p);
            m.put(&t[i..j]);
            i = j;
        }
    }
}

/// What a line did.
pub enum Done {
    /// A blank line or a note (`#`).
    Nothing,
    /// One line of answer: the bottom row shows it; `load` counts the line as applied.
    Said(Msg),
    /// Several lines: the console.
    List(List),
    Dump,
    Load,
    /// A debug command: its lines come from the firmware's answers (TERM-21).
    Debug(super::debug::Job),
}

/// What the console lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum List {
    Help,
    /// The settings not at their defaults.
    Changed,
    /// One channel, as its `ch` line.
    Show(u16),
    /// The three slots, one `slot` line each (TERM-35).
    Slots,
    /// What the screen shows (`ui`, LINK-19): [`crate::uistate`].
    Ui,
    /// The key names `key` and the link's `KEY` take (`ui keys`).
    UiKeys,
    /// The compact status line (`ui status`, the link's `UI` what=2, LINK-19).
    UiStatus,
    /// The readback with the screen as drawn after it (LINK-19.5, the link's `UI` what=3): the link only.
    UiTexts,
}

// ---- Words ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tok<'a> {
    Word(&'a [u8]),
    /// Inside the quotes, escapes still in.
    Quoted(&'a [u8]),
}

/// A line's words: separated by spaces, `"..."` quoted (with `\"` and `\\`), `#` outside quotes starting a note.
#[derive(Clone, Copy)]
struct Toks<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Toks<'a> {
    fn new(s: &'a [u8]) -> Self {
        Toks { s, i: 0 }
    }

    fn skip(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t') {
            self.i += 1;
        }
    }

    fn next(&mut self) -> Result<Option<Tok<'a>>, Msg> {
        self.skip();
        let s = self.s;
        if self.i >= s.len() || s[self.i] == b'#' {
            return Ok(None);
        }
        if s[self.i] == b'"' {
            let start = self.i + 1;
            let mut j = start;
            while j < s.len() && s[j] != b'"' {
                j += if s[j] == b'\\' { 2 } else { 1 };
            }
            if j >= s.len() {
                return Err(msg!("a name has no closing quote"));
            }
            self.i = j + 1;
            return Ok(Some(Tok::Quoted(&s[start..j])));
        }
        let start = self.i;
        while self.i < s.len() && !matches!(s[self.i], b' ' | b'\t') {
            self.i += 1;
        }
        Ok(Some(Tok::Word(&s[start..self.i])))
    }

    /// The next word, or what it says is missing.
    fn word(&mut self, what: &str) -> Result<&'a [u8], Msg> {
        match self.next()? {
            Some(Tok::Word(w)) => Ok(w),
            _ => Err(msg!("{}", what)),
        }
    }

    /// The rest of the line, spaces trimmed, without a trailing note.
    fn rest(&self) -> &'a [u8] {
        let r = &self.s[self.i.min(self.s.len())..];
        let r = match r.iter().position(|&c| c == b'#') {
            Some(k) => &r[..k],
            None => r,
        };
        trim(r)
    }
}

fn trim(s: &[u8]) -> &[u8] {
    let a = s.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(s.len());
    let b = s.iter().rposition(|c| !c.is_ascii_whitespace()).map_or(a, |k| k + 1);
    &s[a..b]
}

/// Same letters, case and spaces aside ("night red" is "Night red", "12.5khz" is "12.5 kHz").
fn same(a: &[u8], b: &[u8]) -> bool {
    let mut x = a.iter().filter(|c| **c != b' ');
    let mut y = b.iter().filter(|c| **c != b' ');
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(p), Some(q)) if p.eq_ignore_ascii_case(q) => {}
            _ => return false,
        }
    }
}

fn number(w: &[u8]) -> Option<u32> {
    if w.is_empty() || w.len() > 9 || !w.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(w.iter().fold(0, |n, &c| n * 10 + (c - b'0') as u32))
}

fn on_off(v: &[u8], what: &str) -> Result<bool, Msg> {
    if same(v, b"on") || same(v, b"yes") {
        Ok(true)
    } else if same(v, b"off") || same(v, b"no") {
        Ok(false)
    } else {
        Err(msg!("{}: on or off", what))
    }
}

/// A quoted name, unescaped, up to `N` characters of printable ASCII.
fn unquote<const N: usize>(q: &[u8], what: &str) -> Result<Buf<N>, Msg> {
    let mut b = Buf::<N>::new();
    let mut i = 0;
    while i < q.len() {
        let c = if q[i] == b'\\' && i + 1 < q.len() {
            i += 1;
            q[i]
        } else {
            q[i]
        };
        if !(0x20..0x7F).contains(&c) {
            return Err(msg!("{}: letters, digits and punctuation only", what));
        }
        if b.len() == N {
            return Err(msg!("{}: {} characters at most", what, N));
        }
        b.push(&[c]);
        i += 1;
    }
    Ok(b)
}

/// A name between quotes, `"` and `\` escaped.
fn quoted(name: &[u8], out: &mut Line) {
    out.push(b"\"");
    for &c in name {
        if c == b'"' || c == b'\\' {
            out.push(b"\\");
        }
        out.push(&[c]);
    }
    out.push(b"\"");
}

/// A line's start: `ch 12 "Name"`, `zone 2 "Repeaters"`, `preset 1 "2 m"` (`n` 0-based).
#[inline(never)]
fn head(what: &[u8], n: u16, name: &[u8], out: &mut Line) {
    num(out, what, n as u32 + 1);
    out.push(b" ");
    quoted(name, out);
}

/// A channel or zone number as written, 1-based, to the radio's 0-based one.
fn index(w: &[u8], max: u32, what: &str) -> Result<u32, Msg> {
    match number(w) {
        Some(n) if (1..=max).contains(&n) => Ok(n - 1),
        _ => Err(msg!("{}: a number from 1 to {}", what, max)),
    }
}

/// "146.52", "146.520 00" (one word here): MHz from 0.15 MHz (HF channels, CHAN-26) up to 1300 MHz.
fn freq(w: &[u8], what: &str) -> Result<Freq10, Msg> {
    match parse_mhz(w) {
        Some(f) if w.iter().all(|c| c.is_ascii_digit() || *c == b'.') && (crate::chan::CH_LO..=mhz(1300)).contains(&f) => Ok(f),
        _ => Err(msg!("{}: MHz from 0.15 to 1300, e.g. 146.52", what)),
    }
}

/// Degrees x 1e7 as written: "45.4215", "-75.6972" (up to seven decimals, trailing zeros left off).
fn deg_word(v: i32, out: &mut Line) {
    if v < 0 {
        out.push(b"-");
    }
    let a = v.unsigned_abs();
    num(out, b"", a / 10_000_000);
    let mut frac = a % 10_000_000;
    if frac != 0 {
        out.push(b".");
        let mut d = 1_000_000;
        while frac != 0 {
            out.push(&[b'0' + (frac / d) as u8]);
            frac %= d;
            d /= 10;
        }
    }
}

/// A position as `pos=` writes it (CHAN-2, TERM-9): "45.4215,-75.6972".
fn pos_word(p: Pos, out: &mut Line) {
    deg_word(p.lat, out);
    out.push(b",");
    deg_word(p.lon, out);
}

/// "-75.6972" to degrees x 1e7, within +-max degrees; up to seven decimals.
fn deg_val(w: &[u8], max: u32) -> Option<i32> {
    let (neg, w) = match w.strip_prefix(b"-") {
        Some(r) => (true, r),
        None => (false, w.strip_prefix(b"+").unwrap_or(w)),
    };
    let (int, frac) = match w.iter().position(|&c| c == b'.') {
        Some(i) => (&w[..i], &w[i + 1..]),
        None => (w, &b""[..]),
    };
    if int.is_empty() || int.len() > 3 || frac.len() > 7 || !frac.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let deg = number(int)?;
    let f = frac.iter().chain(core::iter::repeat_n(&b'0', 7 - frac.len())).fold(0u32, |n, &c| n * 10 + (c - b'0') as u32);
    let v = deg as u64 * 10_000_000 + f as u64;
    (v <= max as u64 * 10_000_000).then_some(if neg { -(v as i64) } else { v as i64 } as i32)
}

/// A position typed for `ch N pos=` or `set 0 6 9`: "45.4215,-75.6972" (degrees, north and east positive), a grid
/// square ("FN25dk", its centre: GPS-21), or "none" (Ok(None)).
fn pos_val(v: &[u8], what: &str) -> Result<Option<Pos>, Msg> {
    if same(v, b"none") {
        return Ok(None);
    }
    let bad = || msg!("{}: lat,lon in degrees (45.42,-75.69), a grid square, or none", what);
    if let Some(i) = v.iter().position(|&c| c == b',') {
        let lat = deg_val(&v[..i], 90).ok_or_else(bad)?;
        let lon = deg_val(&v[i + 1..], 180).ok_or_else(bad)?;
        return Ok(Some(Pos { lat, lon }));
    }
    crate::screens::lists::grid_pos(v).map(Some).ok_or_else(bad)
}

// ---- Values in a line ----

/// "off", "100.0", "D023N".
fn tone_word(t: u16, out: &mut Line) {
    if t == 0 {
        out.push(b"off");
    } else if t & DCS_FLAG == 0 {
        num(out, b"", t as u32 / 10);
        num(out, b".", t as u32 % 10);
    } else {
        let c = t & 0x1FF;
        out.push(&[b'D', b'0' + (c >> 6) as u8, b'0' + ((c >> 3) & 7) as u8, b'0' + (c & 7) as u8, if t & DCS_INV != 0 { b'I' } else { b'N' }]);
    }
}

/// " tone=100.0", or " txtone=100.0 rxtone=off" when they differ: the channel and slot lines' tones.
#[inline(never)]
fn tones(tx: u16, rx: u16, out: &mut Line) {
    if rx == tx {
        out.push(b" tone=");
    } else {
        out.push(b" txtone=");
        tone_word(tx, out);
        out.push(b" rxtone=");
    }
    tone_word(rx, out);
}

/// " mode=NFM".
#[inline(never)]
fn mode_word(m: u8, out: &mut Line) {
    out.push(b" mode=");
    out.push(MODES[m as usize % MODES.len()].as_bytes());
}

/// " power=high".
#[inline(never)]
fn power_word(p: u8, out: &mut Line) {
    out.push(b" power=");
    out.push(POWERS[p as usize % 3].as_bytes());
}

fn tone(w: &[u8], what: &str) -> Result<u16, Msg> {
    if same(w, b"off") || same(w, b"none") {
        return Ok(0);
    }
    let bad = || msg!("{}: off, a CTCSS tone (100.0) or a DCS code (D023N, D023I)", what);
    if let Some(d) = w.strip_prefix(b"D").or_else(|| w.strip_prefix(b"d")) {
        let (code, inv) = match d.last() {
            Some(b'N' | b'n') => (&d[..d.len() - 1], false),
            Some(b'I' | b'i') => (&d[..d.len() - 1], true),
            _ => (d, false),
        };
        if code.len() != 3 || !code.iter().all(|c| (b'0'..=b'7').contains(c)) {
            return Err(bad());
        }
        // TONE-15: any code D000-D777, standard or not.
        let c = code.iter().fold(0u16, |n, &k| n * 8 + (k - b'0') as u16);
        return Ok(DCS_FLAG | if inv { DCS_INV } else { 0 } | c);
    }
    let t = match parse_mhz(w.strip_prefix(b"T").unwrap_or(w)) {
        // parse_mhz reads "100.0" as 100.0 MHz in 10 Hz: tenths of a hertz are its 10 kHz digit.
        Some(f) if f % 10_000 == 0 => f / 10_000,
        _ => return Err(bad()),
    };
    // TONE-15: any tone from 60.0 to 260.0 Hz, standard or not.
    match u16::try_from(t).ok().filter(|t| (CTCSS_MIN..=CTCSS_MAX).contains(t)) {
        Some(c) => Ok(c),
        None => Err(msg!("{}: {}.{} Hz is outside 60.0 to 260.0 Hz", what, t / 10, t % 10)),
    }
}

/// `slot`'s tone argument (TONE-2.1): one word as [`tone`] reads it ("off", "100.0", "D023N"), or a mode and a
/// value: "ctcss 100.0", "dcs-n 023", "dcs-i 023" ("off" alone is the Off mode).
fn tone_arg(t: &mut Toks, what: &str) -> Result<u16, Msg> {
    use crate::screens::tones::MODE_NAMES;
    const HELP: &str = ": off, 100.0, D023N, or a mode and value (ctcss 100.0, dcs-n 023, dcs-i 023)";
    let missing = || msg!("{}{}", what, HELP);
    let w = t.word(what).map_err(|_| missing())?;
    let Some(m) = MODE_NAMES.iter().skip(1).position(|n| same(w, n.as_bytes())) else { return tone(w, what) };
    let v = t.word(what).map_err(|_| missing())?;
    let bad = || msg!("{}: {} takes {}", what, MODE_NAMES[m + 1], if m == 0 { "a tone, e.g. 100.0" } else { "an octal code, e.g. 023" });
    if m == 0 {
        if v.first().is_some_and(|c| c.eq_ignore_ascii_case(&b'D')) {
            return Err(bad());
        }
        return tone(v, what);
    }
    let code = v.strip_prefix(b"D").or_else(|| v.strip_prefix(b"d")).unwrap_or(v);
    if code.len() != 3 {
        return Err(bad());
    }
    let mut d = [b'D', 0, 0, 0, if m == 1 { b'N' } else { b'I' }];
    d[1..4].copy_from_slice(code);
    tone(&d, what).map_err(|_| bad())
}

/// " tx=146.52000" (split), or " offset=+0.60000".
#[inline(never)]
fn offset_kv(split: bool, o: i32, out: &mut Line) {
    if split {
        out.push(b" tx=");
        out.push(mhz5(o.max(0) as Freq10).as_bytes());
    } else {
        out.push(b" offset=");
        offset_word(o, out);
    }
}

/// "simplex", "+0.60000", "-5.00000".
fn offset_word(o: i32, out: &mut Line) {
    if o == 0 {
        out.push(b"simplex");
    } else {
        out.push(if o < 0 { b"-" } else { b"+" });
        out.push(mhz5(o.unsigned_abs()).as_bytes());
    }
}

fn offset(w: &[u8]) -> Result<i32, Msg> {
    if same(w, b"simplex") || w == b"0" {
        return Ok(0);
    }
    let (neg, m) = match w.first() {
        Some(b'-') => (true, &w[1..]),
        Some(b'+') => (false, &w[1..]),
        _ => (false, w),
    };
    match parse_mhz(m) {
        Some(f) if m.iter().all(|c| c.is_ascii_digit() || *c == b'.') && f < mhz(100) => Ok(if neg { -(f as i32) } else { f as i32 }),
        _ => Err(msg!("offset: simplex, or MHz with a sign, e.g. +5 or -0.6")),
    }
}

/// "12.5kHz": the step's name without its space.
fn step_word(s: u8, out: &mut Line) {
    for c in STEPS[s as usize % STEPS.len()].1.bytes().filter(|&c| c != b' ') {
        out.push(&[c]);
    }
}

fn step(w: &[u8]) -> Result<u8, Msg> {
    STEPS.iter().position(|s| same(w, s.1.as_bytes())).map(|i| i as u8).ok_or_else(|| msg!("step: one of the radio's steps, e.g. 12.5kHz or 25kHz"))
}

fn pick(w: &[u8], list: &[&str], what: &str) -> Result<u8, Msg> {
    match list.iter().position(|l| same(w, l.as_bytes())) {
        Some(i) => Ok(i as u8),
        None => {
            let mut m = msg!("{}: ", what);
            for (i, l) in list.iter().enumerate() {
                if i > 0 {
                    m.push(b", ");
                }
                m.push(l.as_bytes());
            }
            Err(m)
        }
    }
}

const POWERS: [&str; 3] = ["low", "mid", "high"];
const BUSY: [&str; 2] = ["off", "on"];
const BW: [&str; 2] = ["wide", "narrow"];
/// Channel fields by name, in the order a `ch` line writes them.
const FIELDS: [&str; 22] = [
    "name", "rx", "offset", "mode", "bw", "tone", "rxtone", "txtone", "power", "zone", "step", "squelch", "busy", "reverse", "skip", "aprs", "rxonly", "tx", "hidden",
    "priority", "endtone", "tail",
];
/// A channel's end tone and tail words (crate::chan::END_TONES, TAILS; decision 86).
const END_WORDS: [&str; 7] = ["global", "off", "beep", "up", "down", "chirp", "quindar"];
const TAIL_WORDS: [&str; 3] = ["global", "off", "on"];

// ---- Channels ----

/// Channel `c` as its `ch` line: number, name and RX, then the fields always written, then those not at their
/// defaults. Every field `load` reads back is here, so a dump and a load give the same channel.
fn ch_line<S: ChannelStore>(c: &Channel, st: &S, out: &mut Line) {
    let g = &c.cfg;
    head(b"ch ", c.num, c.name(), out);
    out.push(b" ");
    out.push(mhz5(c.rx).as_bytes());
    if g.split {
        // Split: TX is a frequency of its own (MENU 5's 4th choice)
        out.push(b" offset=split");
    }
    offset_kv(g.split, g.offset, out);
    mode_word(g.mode, out);
    tones(g.tx_tone, g.rx_tone, out);
    power_word(c.cfg.power, out);
    if c.zone != NO_ZONE && c.zone < st.zones() {
        num(out, b" zone=", c.zone as u32 + 1);
    }
    if g.step != DEFAULT_CFG.step {
        out.push(b" step=");
        step_word(g.step, out);
    }
    if g.squelch >= SQ_GLOBAL {
        out.push(b" squelch=global");
    } else if g.squelch != DEFAULT_CFG.squelch {
        num(out, b" squelch=", g.squelch as u32);
    }
    if g.busy != 0 {
        out.push(b" busy=");
        out.push(BUSY[g.busy.min(1) as usize].as_bytes());
    }
    if c.end_tone() != 0 {
        out.push(b" endtone=");
        out.push(END_WORDS[c.end_tone() as usize].as_bytes());
    }
    if c.tail() != 0 {
        out.push(b" tail=");
        out.push(TAIL_WORDS[c.tail() as usize].as_bytes());
    }
    for (on, w) in [
        (g.reverse, "reverse"),
        (c.scan() == 1, "skip"),
        (c.flags & FLAG_APRS != 0, "aprs"),
        (c.flags & FLAG_RX_ONLY != 0, "rxonly"),
        (c.flags & FLAG_HIDDEN != 0, "hidden"),
        (c.scan() == 2, "priority"),
    ] {
        if on {
            out.push(b" ");
            out.push(w.as_bytes());
        }
    }
    if let Some(p) = st.position(c.num) {
        out.push(b" pos=");
        pos_word(p, out);
    }
}

/// One field of a channel from `key` and its value (a bare flag word is "on").
fn field<S: ChannelStore>(c: &mut Channel, key: &[u8], v: Option<Tok>, st: &S) -> Result<(), Msg> {
    let k = FIELDS.iter().position(|f| same(key, f.as_bytes())).ok_or_else(|| msg!("{}: no such field; help lists them", Bytes(key)))?;
    let flag = |c: &mut Channel, bit: u8, v: Option<Tok>, what: &str| -> Result<(), Msg> {
        let on = match v {
            None => true,
            Some(Tok::Word(w)) => on_off(w, what)?,
            Some(Tok::Quoted(_)) => return Err(msg!("{}: on or off", what)),
        };
        // Skip and Priority exclude each other (decision 63): one on puts the channel there, off only leaves it
        let s = [FLAG_SKIP, FLAG_PRIORITY].iter().position(|&b| b == bit).map(|i| i as u8 + 1);
        match s {
            Some(s) if on => c.set_scan(s),
            Some(s) if c.scan() == s => c.set_scan(0),
            Some(_) => {}
            None => c.flags = if on { c.flags | bit } else { c.flags & !bit },
        }
        Ok(())
    };
    match k {
        13 => {
            c.cfg.reverse = match v {
                None => true,
                Some(Tok::Word(w)) => on_off(w, "reverse")?,
                _ => return Err(msg!("reverse: on or off")),
            };
            return Ok(());
        }
        14 => return flag(c, FLAG_SKIP, v, "skip"),
        15 => return flag(c, FLAG_APRS, v, "aprs"),
        16 => return flag(c, FLAG_RX_ONLY, v, "rxonly"),
        18 => return flag(c, FLAG_HIDDEN, v, "hidden"),
        19 => return flag(c, FLAG_PRIORITY, v, "priority"),
        _ => {}
    }
    let v = match v {
        Some(Tok::Quoted(q)) if k == 0 => {
            let n: Buf<16> = unquote(q, "name")?;
            c.set_name(n.as_bytes());
            return Ok(());
        }
        Some(Tok::Word(w)) => w,
        _ => return Err(msg!("{}: needs a value, e.g. {}=...", FIELDS[k], FIELDS[k])),
    };
    let g = &mut c.cfg;
    match k {
        0 => {
            let n: Buf<16> = unquote(v, "name")?;
            c.set_name(n.as_bytes());
        }
        1 => c.rx = freq(v, "rx")?,
        // "split" keeps the TX frequency a tx= field gives (before or after it on the line)
        2 if same(v, b"split") => {
            if !g.split {
                (g.split, g.offset) = (true, 0); // the TX comes from tx=, before or after it on the line
            }
        }
        2 => (g.offset, g.split) = (offset(v)?, false),
        17 => (g.split, g.offset) = (true, freq(v, "tx")? as i32),
        3 => {
            g.mode = pick(v, &MODES, "mode")?; // USB, LSB and CW: HF channels only (check)
            g.narrow = g.mode == 1; // VFO-20
        }
        // bw= from older dumps: FM and NFM follow it; AM stays AM (VFO-20)
        4 => {
            let narrow = pick(v, &BW, "bw")? == 1;
            if g.mode < 2 {
                g.mode = narrow as u8;
            }
            g.narrow = g.mode == 1;
        }
        5 => (g.tx_tone, g.rx_tone) = {
            let t = tone(v, "tone")?;
            (t, t)
        },
        6 => g.rx_tone = tone(v, "rxtone")?,
        7 => g.tx_tone = tone(v, "txtone")?,
        8 => g.power = pick(v, &POWERS, "power")?,
        9 => {
            c.zone = if same(v, b"none") {
                NO_ZONE
            } else {
                let z = index(v, 254, "zone")? as u8;
                if z >= st.zones() {
                    return Err(msg!("zone: zone {} doesn't exist; the radio has {}", z as u32 + 1, st.zones()));
                }
                z
            }
        }
        10 => g.step = step(v)?,
        11 if same(v, b"global") => g.squelch = SQ_GLOBAL,
        11 => match number(v) {
            Some(n) if n <= 9 => g.squelch = n as u8,
            _ => return Err(msg!("squelch: global, or 0 to 9")),
        },
        20 => c.set_end_tone(pick(v, &END_WORDS, "endtone")?),
        21 => c.set_tail(pick(v, &TAIL_WORDS, "tail")?),
        _ => g.busy = pick(v, &BUSY, "busy")?,
    }
    Ok(())
}

/// What the radio's channel editor would refuse (CHAN-22, TX-9, TERM-7): [`chfield::check`] for a whole channel.
pub(crate) fn check(c: &Channel) -> Result<(), Msg> {
    match chfield::check(c) {
        Err(Refusal::Relay | Refusal::TxRange) if c.cfg.split && c.cfg.offset <= 0 => Err(msg!("offset=split needs its TX frequency: tx=146.52")),
        r => r.map_err(refusal),
    }
}

/// A refusal headed by its field's key ("power: this channel is RX only").
fn refusal(r: Refusal) -> Msg {
    let key = match r.field() {
        Some(ChField::Rx) => "rx: ",
        Some(ChField::It(Item::Mode)) => "mode: ",
        Some(ChField::It(Item::Offset)) => "offset: ",
        Some(ChField::It(Item::Power)) => "power: ",
        Some(_) => "name: ",
        None => "",
    };
    msg!("{}{}", key, r.why())
}

/// Saves a channel (TERM-9) unless the store already holds it exactly: a load of the same dump writes nothing.
fn save<S: ChannelStore>(app: &mut App, st: &S, c: Channel) -> Msg {
    if st.by_num(c.num).map(|x| x.0) != Some(c) {
        app.put_channel(c);
    }
    let mut m = msg!("Channel {} saved", c.num as u32 + 1);
    if c.name().is_empty() {
        m.push(b" (no name)");
    }
    m
}

/// [`save`] with the channel's position set or cleared (CHAN-2: `pos=`); nothing written when both are as stored.
fn save_pos<S: ChannelStore>(app: &mut App, st: &S, c: Channel, pos: Option<Pos>) -> Msg {
    if st.by_num(c.num).map(|x| x.0) != Some(c) || st.position(c.num) != pos {
        app.put_channel_pos(c, pos);
    }
    let mut m = msg!("Channel {} saved", c.num as u32 + 1);
    if c.name().is_empty() {
        m.push(b" (no name)");
    }
    m
}

fn ch<S: ChannelStore>(app: &mut App, st: &S, t: &mut Toks) -> Result<Done, Msg> {
    let num = index(t.word("ch: which channel, e.g. ch 12")?, MAX_CHANNELS as u32, "ch")? as u16;
    let old = st.by_num(num).map(|x| x.0);
    let missing = || msg!("ch {}: no such channel", num as u32 + 1);
    if t.clone().next()?.is_some() && super::list::radio_channel(app) == Some(num) {
        return Err(msg!("Channel {} is open in the radio's editor", num as u32 + 1)); // TERM-17
    }
    match t.next()? {
        None => old.map(|_| Done::List(List::Show(num))).ok_or_else(missing),
        // A whole channel: name, RX, then fields; the rest at their defaults.
        Some(Tok::Quoted(q)) => {
            let name: Buf<16> = unquote(q, "name")?;
            let rx = freq(t.word("ch: the RX MHz after the name")?, "rx")?;
            let mut c = Channel { num, zone: NO_ZONE, rx, name: [0; 16], cfg: DEFAULT_CFG, flags: 0, tones: 0 };
            c.set_name(name.as_bytes());
            let mut pos = None;
            while let Some(tok) = t.next()? {
                let Tok::Word(w) = tok else { return Err(msg!("ch: a name in quotes goes right after the number")) };
                match w.iter().position(|&b| b == b'=') {
                    Some(e) if same(&w[..e], b"pos") => pos = Some(pos_val(&w[e + 1..], "pos")?),
                    Some(e) => field(&mut c, &w[..e], Some(Tok::Word(&w[e + 1..])), st)?,
                    None => field(&mut c, w, None, st)?,
                }
            }
            check(&c)?;
            // A line without pos= keeps the position the channel has (the web app's import sets it).
            Ok(Done::Said(match pos {
                Some(p) => save_pos(app, st, c, p),
                None => save(app, st, c),
            }))
        }
        Some(Tok::Word(w)) if same(w, b"delete") => {
            old.ok_or_else(missing)?;
            app.delete_channel(num, st);
            Ok(Done::Said(msg!("Channel {} deleted", num as u32 + 1)))
        }
        // One field: ch 12 power high, ch 12 name "Club".
        Some(Tok::Word(w)) => {
            let mut c = old.ok_or_else(missing)?;
            let (key, v) = match w.iter().position(|&b| b == b'=') {
                Some(e) => (&w[..e], Some(Tok::Word(&w[e + 1..]))),
                None => (w, t.next()?),
            };
            if same(key, b"pos") {
                let Some(Tok::Word(v)) = v else { return Err(msg!("pos: latitude,longitude, a grid square, or none")) };
                let p = pos_val(v, "pos")?;
                if t.next()?.is_some() {
                    return Err(msg!("ch: one field at a time"));
                }
                return Ok(Done::Said(save_pos(app, st, c, p)));
            }
            let was = c;
            field(&mut c, key, v, st)?;
            if t.next()?.is_some() {
                return Err(msg!("ch: one field at a time"));
            }
            chfield::check_change(&was, &c).map_err(refusal)?; // as the editors change a field
            Ok(Done::Said(save(app, st, c)))
        }
    }
}

// ---- Zones and range presets ----

/// "radio", "5s", "carrier", "stop".
fn resume_word(r: u8, out: &mut Line) {
    match r {
        0 => out.push(b"radio"),
        11 => out.push(b"carrier"),
        12 => out.push(b"stop"),
        s => {
            num(out, b"", s as u32);
            out.push(b"s");
        }
    }
}

/// The other way: "radio" (or "global") is 0.
fn resume_val(v: &[u8]) -> Result<u8, Msg> {
    Ok(match v {
        _ if same(v, b"radio") || same(v, b"global") => 0,
        _ if same(v, b"carrier") => 11,
        _ if same(v, b"stop") => 12,
        _ => secs(v, 10, "resume").map_err(|_| msg!("resume: radio, 1s to 10s, carrier or stop"))?,
    })
}

fn secs(w: &[u8], max: u32, what: &str) -> Result<u8, Msg> {
    match number(w.strip_suffix(b"s").unwrap_or(w)) {
        Some(n) if (1..=max).contains(&n) => Ok(n as u8),
        _ => Err(msg!("{}: radio, or 1s to {}s", what, max)),
    }
}

fn zone_line<S: ChannelStore>(z: u8, st: &S, out: &mut Line) {
    let c = st.data().zone_cfg(z);
    head(b"zone ", z as u16, st.zone_label(z, b"", b"").as_bytes(), out);
    let o = |b: bool| if b { "on" } else { "off" };
    for (k, b) in [(" scan=", c.scan_all), (" shown=", c.shown), (" distance=", c.by_distance)] {
        out.push(k.as_bytes());
        out.push(o(b).as_bytes());
    }
    out.push(b" resume=");
    resume_word(c.resume, out);
    out.push(b" dwell=");
    if c.dwell == 255 {
        out.push(b"radio");
    } else {
        num(out, b"", c.dwell as u32);
        out.push(b"s");
    }
    for (i, p) in c.pri.iter().enumerate() {
        if let Some(n) = p {
            num(out, b" pri", i as u32 + 1);
            num(out, b"=", *n as u32 + 1);
        }
    }
}

/// A `key=value` field: the key and the value.
type Kv<'a> = (&'a [u8], &'a [u8]);

/// The next `key=value` after a zone's or preset's name; `what` names the line ("zone"), `eg` an example field.
fn kv<'a>(t: &mut Toks<'a>, what: &str, eg: &str) -> Result<Option<Kv<'a>>, Msg> {
    match t.next()? {
        None => Ok(None),
        Some(Tok::Quoted(_)) => Err(msg!("{}: one name per {}", what, what)),
        Some(Tok::Word(w)) => {
            let e = w.iter().position(|&b| b == b'=').ok_or_else(|| msg!("{}: fields are key=value, e.g. {}", what, eg))?;
            Ok(Some((&w[..e], &w[e + 1..])))
        }
    }
}

/// Zones and presets are numbered in order: `n` (0-based) may replace one up to `have`, or add the next.
fn in_order(what: &str, n: u8, have: u8) -> Result<(), Msg> {
    if n > have {
        return Err(msg!("{} {}: {}s are made in order; the radio has {}, so the next is {} {}", what, n as u32 + 1, what, have, what, have as u32 + 1));
    }
    Ok(())
}

/// Hands a zone or preset to the store, unless it is already what the store holds.
fn put<S: ChannelStore>(st: &S, cap: &str, what: &str, n: u8, same_as_stored: bool, op: Op) -> Result<Done, Msg> {
    if same_as_stored {
        return Ok(Done::Said(msg!("{} {} unchanged", cap, n as u32 + 1)));
    }
    if !st.data().request(op) {
        return Err(msg!("{} {}: the radio is full or busy; try again", what, n as u32 + 1));
    }
    Ok(Done::Said(msg!("{} {} saved", cap, n as u32 + 1)))
}

const ZONE_KEYS: [&str; 7] = ["scan", "shown", "distance", "resume", "dwell", "pri1", "pri2"];

fn zone<S: ChannelStore>(st: &S, t: &mut Toks) -> Result<Done, Msg> {
    let z = index(t.word("zone: which zone, e.g. zone 2 \"Repeaters\"")?, 254, "zone")? as u8;
    let have = st.zones();
    in_order("zone", z, have)?;
    let Some(Tok::Quoted(q)) = t.next()? else {
        return Err(msg!("zone {}: its name in quotes, e.g. zone {} \"Repeaters\"", z as u32 + 1, z as u32 + 1));
    };
    let name: Name = unquote(q, "name")?;
    let mut c = if z < have { st.data().zone_cfg(z) } else { ZoneCfg { shown: true, dwell: 255, ..ZoneCfg::default() } };
    while let Some((k, v)) = kv(t, "zone", "scan=on")? {
        let i = ZONE_KEYS.iter().position(|f| same(k, f.as_bytes()));
        let i = i.ok_or_else(|| msg!("zone: no field {}; they are scan, shown, distance, resume, dwell, pri1, pri2", Bytes(k)))?;
        match i {
            0..=2 => *[&mut c.scan_all, &mut c.shown, &mut c.by_distance][i] = on_off(v, ZONE_KEYS[i])?,
            3 => {
                c.resume = resume_val(v)?;
            }
            4 => c.dwell = if same(v, b"radio") { 255 } else { secs(v, 254, "dwell")? },
            _ => c.pri[i - 5] = if same(v, b"none") { None } else { Some(index(v, MAX_CHANNELS as u32, "pri")? as u16) },
        }
    }
    let stored = z < have && st.zone_name(z) == Some(name) && st.data().zone_cfg(z) == c;
    put(st, "Zone", "zone", z, stored, Op::PutZone(z, name, c))
}

fn preset_line(i: u8, p: &RangePreset, out: &mut Line) {
    head(b"preset ", i as u16, p.name.as_bytes(), out);
    out.push(b" ");
    out.push(mhz5(p.start).as_bytes());
    out.push(b" ");
    out.push(mhz5(p.end).as_bytes());
    out.push(b" step=");
    step_word(p.step, out);
    mode_word(p.mode, out);
    if p.resume != 0 {
        out.push(b" resume=");
        resume_word(p.resume, out);
    }
}

fn preset<S: ChannelStore>(st: &S, t: &mut Toks) -> Result<Done, Msg> {
    let d = st.data();
    let i = index(t.word("preset: which one, e.g. preset 1 \"2 m\" 144 148")?, 16, "preset")? as u8;
    in_order("preset", i, d.presets())?;
    let Some(Tok::Quoted(q)) = t.next()? else { return Err(msg!("preset: its name in quotes, then start and end in MHz")) };
    let name: Buf<12> = unquote(q, "name")?;
    let start = freq(t.word("preset: the start in MHz after the name")?, "start")?;
    let end = freq(t.word("preset: the end in MHz after the start")?, "end")?;
    if end <= start {
        return Err(msg!("preset: the end must be above the start"));
    }
    let mut p = RangePreset { name, start, end, step: STEP_12K5, mode: 1, narrow: true, skips: 0, resume: 0 };
    while let Some((k, v)) = kv(t, "preset", "step=25kHz")? {
        if same(k, b"step") {
            p.step = step(v)?;
        } else if same(k, b"mode") {
            p.mode = pick(v, &MODES[..SLOT_MODES], "mode")?;
            p.narrow = p.mode == 1; // VFO-20
        } else if same(k, b"bw") {
            // older dumps: FM and NFM follow it
            let narrow = pick(v, &BW, "bw")? == 1;
            if p.mode < 2 {
                p.mode = narrow as u8;
            }
            p.narrow = p.mode == 1;
        } else if same(k, b"resume") {
            p.resume = resume_val(v)?;
        } else {
            return Err(msg!("preset: no field {}; they are step, mode, bw, resume", Bytes(k)));
        }
    }
    let stored = d.preset(i).is_some_and(|o| RangePreset { skips: o.skips, ..p } == o);
    put(st, "Preset", "preset", i, stored, Op::PutPreset(i, p))
}

// ---- Settings ----

/// The menu number after MENU (TERM-6): "0", the group, the item, and the third level where the item has one.
fn path(t: &mut Toks) -> Result<([u8; 3], usize), Msg> {
    let usage = || msg!("a menu number from MENU, e.g. 0 7 1 (Theme)");
    if t.word("")
        .ok()
        .filter(|w| *w == b"0")
        .is_none()
    {
        return Err(usage());
    }
    let mut p = [0u8; 3];
    let digit = |t: &mut Toks, n: usize| -> Result<u8, Msg> {
        match t.word("").ok().and_then(number) {
            Some(d) if (1..=n as u32).contains(&d) => Ok(d as u8),
            _ => Err(usage()),
        }
    };
    p[0] = digit(t, 9)?;
    p[1] = digit(t, groups::items(p[0], 0).len())?;
    if SUBS.iter().any(|(q, _)| *q == (p[0], p[1])) {
        // TX-48: 0 8 8 alone still names Extend TX, as before Dry TX joined it in a list (0 8 8 4, 0 8 8 5).
        let n = groups::items(p[0], p[1]).len() as u32;
        let mut peek = *t;
        if (p[0], p[1]) == (8, 9) && !peek.word("").ok().and_then(number).is_some_and(|d| (1..=n).contains(&d)) {
            return Ok((p, 2));
        }
        p[2] = digit(t, groups::items(p[0], p[1]).len())?;
        return Ok((p, 3));
    }
    Ok((p, 2))
}

/// The item's name ("Theme").
fn path_name(p: &[u8]) -> &'static str {
    match p {
        [g, x] => groups::items(*g, 0)[*x as usize - 1].name(),
        [g, s, x] => groups::items(*g, *s)[*x as usize - 1].name(),
        _ => "",
    }
}

/// "0 7 1 Theme".
fn path_text<const N: usize>(p: &[u8], out: &mut Buf<N>) {
    out.push(b"0");
    for x in p {
        out.push(b" ");
        out.push(super::digits(*x as u32, &mut [0; 10]));
    }
}

fn value(app: &App, dev: &Device, p: &[u8], b: &mut Buf<24>) -> bool {
    *b = Buf::new(); // some wired values push: start empty, or the knob loop never sees its start again
    match groups::wired(app, dev, p, b) {
        Some(d) => d,
        None => {
            let e = match p {
                [g, x] => groups::items(*g, 0)[*x as usize - 1],
                [g, s, x] => groups::items(*g, *s)[*x as usize - 1],
                _ => return false,
            };
            b.set(e.value().as_bytes());
            false
        }
    }
}

fn set(app: &mut App, dev: &Device, t: &mut Toks) -> Result<Done, Msg> {
    let (p, n) = path(t)?;
    let p = &p[..n];
    let name = path_name(p);
    if p == [8, 8, 5] && app.radio.tx_built {
        // TX-48.2: the PC may turn Dry TX on (it only takes power away), never off.
        if same(t.rest(), b"on") {
            app.set_dry_tx(true);
            return Ok(Done::Said(msg!("0 8 8 5 Dry TX: On")));
        }
        return Err(msg!("Dry TX goes off only on the radio"));
    }
    if blocked(p) {
        return Err(msg!("{} changes only on the radio", name));
    }
    if super::list::radio_setting(app).is_some_and(|(q, ql)| &q[..ql] == p) {
        return Err(msg!("{} is open on the radio's menu", name)); // TERM-17
    }
    if let Some(why) = groups::greyed(app, p) {
        return Err(msg!("{}: {}", name, why));
    }
    if p == [6, 9] || p == [5, 5, 5] {
        // GPS-3, GPS-21: the fixed position typed (its altitude kept): degrees, a grid square, or none.
        let want = t.rest();
        match pos_val(want, name)? {
            Some(pos) => (app.set.fixed, app.set.fixed_on) = (pos, true),
            None => app.set.fixed_on = false,
        }
        let mut m = Msg::new();
        path_text(p, &mut m);
        put!(m, " {}: ", name);
        let mut l = Line::new();
        if app.set.fixed_on {
            pos_word(app.set.fixed, &mut l);
        } else {
            l.push(b"None");
        }
        m.push(l.as_bytes());
        return Ok(Done::Said(m));
    }
    if !groups::is_settable(p) {
        return Err(msg!("{} can't be set yet", name));
    }
    let want = t.rest();
    if want.is_empty() {
        return Err(msg!("set needs a value, e.g. set 0 7 1 Dark"));
    }
    // Typed values (DEC-4's capcode, APRS-26's custom path): taken as written, "None" clears.
    match p {
        [4, 2, 3] => {
            let v = if same(want, b"none") {
                0
            } else if want.len() <= 7 && want.iter().all(u8::is_ascii_digit) {
                want.iter().fold(0u32, |a, &c| a * 10 + (c - b'0') as u32)
            } else {
                u32::MAX
            };
            if v > ferrite_core::decoders::CAPCODE_MAX {
                return Err(msg!("{}: a capcode up to 2097151, or None", name));
            }
            app.set.pocsag_cap = v;
        }
        [5, 9, 2] | [5, 5, 3] if same(want, b"none") => groups::reset(app, p),
        // APRS-50: the comment as written (a dump line's '#' starts its note, so none in it)
        [5, 5, 3] => {
            if want.len() > crate::aprs::COMMENT_LEN || !want.iter().all(|&c| (b' '..=b'~').contains(&c)) {
                return Err(msg!("{}: up to 40 characters (no #), or None", name));
            }
            app.set_aprs_comment(want);
        }
        // MENU-31: a frequency setting typed in MHz, with the entry box's checks
        [5, 4] | [4, 2, 2] => {
            let fs = if p == [5, 4] { crate::screens::entry::FreqSet::Aprs } else { crate::screens::entry::FreqSet::Pager };
            let f = ferrite_core::freq::parse(want).ok_or_else(|| msg!("{}: a frequency in MHz, e.g. 145.495", name))?;
            let now = if p == [5, 4] { app.aprs_freq() } else { app.set.pocsag_freq };
            // `get` and `dump` show 3 decimals: the value as shown is the value kept (439.987 stays 439.9875)
            if ferrite_ui::widgets::freq3(now).as_bytes() != want {
                if let Err(why) = fs.set_setting(app, f) {
                    return Err(msg!("{}: {}", name, why));
                }
            }
        }
        [5, 9, 2] => {
            let mut up = Buf::<24>::new();
            for &c in want {
                up.push(&[c.to_ascii_uppercase()]);
            }
            if !crate::aprs::path_ok(up.as_bytes()) {
                return Err(msg!("{}: one or two entries like WIDE1-1,WIDE2-1 (16 characters at most), or None", name));
            }
            app.set_aprs_custom_path(up.as_bytes());
        }
        _ => {}
    }
    if matches!(p, [4, 2, 3] | [5, 9, 2] | [5, 5, 3] | [5, 4] | [4, 2, 2]) {
        let mut v = Buf::<24>::new();
        value(app, dev, p, &mut v);
        let mut m = Msg::new();
        path_text(p, &mut m);
        put!(m, " {}: {}", name, Bytes(v.as_bytes()));
        return Ok(Done::Said(m));
    }
    if let Some(why) = groups::refusal(app, p) {
        let mut v = Buf::<24>::new();
        value(app, dev, p, &mut v);
        if !same(v.as_bytes(), want) {
            return Err(msg!("{}: {}", name, why)); // VOX-10: the radio's toast, not "one of Off"
        }
    }
    match walk(app, dev, p, name, want) {
        // Not one of the values as written: an unambiguous short form names the one meant (`set 0 7 7 1` for
        // "Level 1"); the walk again with that value, so it changes exactly as if typed in full.
        Walk::Short(v) => match walk(app, dev, p, name, v.as_bytes()) {
            Walk::Set(m) => Ok(Done::Said(m)),
            Walk::Short(_) => Err(msg!("{}: not changed", name)), // never: the value was just seen
            Walk::No(m) => Err(m),
        },
        Walk::Set(m) => Ok(Done::Said(m)),
        Walk::No(m) => Err(m),
    }
}

/// What [`walk`] found.
enum Walk {
    /// Set: what it says.
    Set(Msg),
    /// Not as written, but the one value a short form names.
    Short(Buf<24>),
    /// Neither: the "one of" message.
    No(Msg),
}

/// A short form of a setting's value (TERM-12): a bare number names the one value that has it as a whole word
/// ("1" for "Level 1", "10" for "10 s"), so a number still means that number; anything else is the start of a
/// value ("night" for "Night red"). Only when one value matches, checked by the caller.
fn loose(v: &[u8], want: &[u8]) -> bool {
    let numeric = want.iter().any(u8::is_ascii_digit) && want.iter().all(|c| c.is_ascii_digit() || b".+-".contains(c));
    if numeric {
        v.split(|c| *c == b' ').any(|w| w == want)
    } else {
        !want.is_empty() && v.len() >= want.len() && same(&v[..want.len()], want)
    }
}

/// Step through the values as the knob does until one reads as asked: up, and a full turn brings it back. A
/// value that stops at its ends (a level, 1-10) is walked up to its top, then down, then back to where it was.
/// Not found: the one value [`loose`] matches if exactly one does, or the "one of" message.
fn walk(app: &mut App, dev: &Device, p: &[u8], name: &str, want: &[u8]) -> Walk {
    let mut v = Buf::<24>::new();
    value(app, dev, p, &mut v);
    let start = v;
    let mut all = msg!("{}: one of ", name);
    let mut n = 0;
    let (mut hits, mut hit) = (0, Buf::<24>::new());
    let fail = |all: Msg, hits: u32, hit: Buf<24>| if hits == 1 { Walk::Short(hit) } else { Walk::No(all) };
    for d in [1, -1, 1] {
        if d == -1 {
            (all, n, hits) = (msg!("{}: one of ", name), 0, 0); // the whole range, top to bottom
        }
        for _ in 0..64 {
            if d == 1 && n > 0 && v == start {
                return fail(all, hits, hit); // back where it started
            }
            if same(v.as_bytes(), want) && n != usize::MAX {
                let mut m = Msg::new();
                path_text(p, &mut m);
                put!(m, " {}: {}", name, Bytes(v.as_bytes()));
                return Walk::Set(m);
            }
            if n != usize::MAX {
                if n > 0 {
                    all.push(b", ");
                }
                all.push(v.as_bytes());
                n += 1;
                if loose(v.as_bytes(), want) {
                    (hits, hit) = (hits + 1, v);
                }
            }
            let was = v;
            groups::change(app, p, d);
            value(app, dev, p, &mut v);
            if v == was {
                break; // an end
            }
        }
        if d == -1 {
            n = usize::MAX; // the last pass only goes back to the start
        }
    }
    fail(all, hits, hit)
}

fn get(app: &App, dev: &Device, t: &mut Toks) -> Result<Done, Msg> {
    if t.clone().next()?.is_none() {
        return Ok(Done::List(List::Changed));
    }
    let (p, n) = path(t)?;
    let p = &p[..n];
    let mut v = Buf::<24>::new();
    let changed = value(app, dev, p, &mut v);
    let mut m = Msg::new();
    path_text(p, &mut m);
    put!(m, " {}: {}", path_name(p), Bytes(v.as_bytes()));
    if v.as_bytes().is_empty() && !groups::is_settable(p) {
        m.push(b"a page on the radio, not a setting"); // TERM-12.1: e.g. 0 5 1, the heard list
    }
    if changed {
        m.push(b" (not the default)");
    }
    Ok(Done::Said(m))
}

/// A setting's `set` line, or a note for those that change only on the radio.
fn set_line(app: &App, dev: &Device, p: &[u8], out: &mut Line) {
    let mut v = Buf::<24>::new();
    value(app, dev, p, &mut v);
    out.push(b"set ");
    path_text(p, out);
    out.push(b" ");
    let c = app.aprs_cfg();
    match p {
        // APRS-50: the whole comment (the row shows its start)
        [5, 5, 3] if !c.comment_text().is_empty() => out.push(c.comment_text()),
        [5, 5, 3] => out.push(b"None"),
        _ => out.push(v.as_bytes()),
    }
    while out.len() < 32 {
        out.push(b" ");
    }
    out.push(b"  # ");
    out.push(path_name(p).as_bytes());
}

// ---- Running a line ----

/// Runs one line: typed after `:`, or read by `load` (`loading`: only the lines a dump holds).
/// Whether a command line changes something (TERM-25: an unapproved Bluetooth device may only read). Reading
/// is `get`, `help`, `dump`, `mem`/`mem32`, `gpio`, `gps`, `ch`, `zone` or `preset` with nothing more than a number
/// (they show it), a bare `home`, `rx` or `txtest`, and `bk settle` alone; everything else, `home N`, the BK4829 and SI4732 commands included, counts
/// as a write.
pub fn writes(line: &[u8]) -> bool {
    let mut w = line.split(|b| b.is_ascii_whitespace()).filter(|x| !x.is_empty());
    let Some(c) = w.next() else { return false };
    let rest = w.count();
    let reads = [&b"get"[..], b"help", b"?", b"dump", b"mem", b"mem32", b"gpio", b"mic", b"af"].iter().any(|r| same(c, r));
    // TERM-36: `gps` and `gps query` only read; `gps standby` puts the module to sleep.
    let second = line.split(|b| b.is_ascii_whitespace()).filter(|x| !x.is_empty()).nth(1);
    let reads = reads || same(c, b"gps") && second.is_none_or(|w| same(w, b"query"));
    let shows = [&b"ch"[..], b"zone", b"preset"].iter().any(|r| same(c, r)) && rest <= 1 || [&b"home"[..], b"rx", b"txtest"].iter().any(|r| same(c, r)) && rest == 0;
    // `bk settle` alone only shows the waits (TERM-31).
    let shows = shows || same(c, b"slot") && rest <= 1; // TERM-35: `slot`, `slot A`
    let shows = shows || same(c, b"xb") && rest == 0; // XB-20: `xb` alone shows the state
    let shows = shows || same(c, b"bk") && rest == 1 && second.is_some_and(|w| same(w, b"settle") || same(w, b"save") || cfg!(feature = "bench") && same(w, b"turns"));
    let shows = shows || same(c, b"lcd") && rest == 0; // TERM-40, TERM-41: `lcd` and `bk save` alone show
    !(reads || shows)
}

pub fn run<S: ChannelStore>(app: &mut App, st: &S, dev: &Device, line: &[u8], loading: bool) -> Result<Done, Msg> {
    let mut t = Toks::new(line);
    let c = match t.next()? {
        None => return Ok(Done::Nothing),
        Some(Tok::Word(c)) => c,
        Some(Tok::Quoted(_)) => return Err(msg!("a command first; help lists them")),
    };
    let only_typed = |what: &str| -> Result<Done, Msg> { Err(msg!("{} can't be in a load", what)) };
    match () {
        _ if same(c, b"set") => set(app, dev, &mut t),
        _ if same(c, b"ch") => ch(app, st, &mut t),
        _ if same(c, b"zone") => zone(st, &mut t),
        _ if same(c, b"preset") => preset(st, &mut t),
        _ if same(c, b"home") => {
            let w = t.word("home: a channel number, or none")?;
            if same(w, b"none") {
                app.set.home_ch = None;
                return Ok(Done::Said(msg!("No home channel")));
            }
            let n = index(w, MAX_CHANNELS as u32, "home")? as u16;
            if st.by_num(n).is_none() {
                return Err(msg!("home: channel {} doesn't exist", n as u32 + 1));
            }
            app.set.home_ch = Some(n);
            Ok(Done::Said(msg!("Home channel: {}", n as u32 + 1)))
        }
        _ if loading => only_typed(core::str::from_utf8(c).unwrap_or("that")),
        _ if same(c, b"get") => get(app, dev, &mut t),
        _ if same(c, b"slot") => slot(app, st, &mut t),
        _ if same(c, b"ui") => match t.next()? {
            None => Ok(Done::List(List::Ui)),
            Some(Tok::Word(w)) if same(w, b"keys") => Ok(Done::List(List::UiKeys)),
            Some(Tok::Word(w)) if same(w, b"status") => Ok(Done::List(List::UiStatus)),
            _ => Err(msg!("ui, ui keys or ui status")),
        },
        _ if same(c, b"key") => key(app, st, &mut t),
        _ if same(c, b"knob") => knob(app, st, &mut t),
        _ if same(c, b"xb") => xb(app, &mut t),
        _ if same(c, b"help") || c == b"?" => Ok(Done::List(List::Help)),
        _ if same(c, b"dump") => Ok(Done::Dump),
        _ if same(c, b"gps") => gps(&mut t),
        #[cfg(feature = "bench")]
        _ if same(c, b"mem") || same(c, b"mem32") => mem(&mut t, same(c, b"mem32")),
        #[cfg(feature = "bench")]
        _ if same(c, b"gpio") => Ok(Done::Debug(Job::new(Kind::Gpio))),
        #[cfg(feature = "bench")]
        _ if same(c, b"mic") => Ok(Done::Debug(Job::new(Kind::Mic(false)))),
        #[cfg(feature = "bench")]
        _ if same(c, b"af") => Ok(Done::Debug(Job::new(Kind::Mic(true)))),
        #[cfg(feature = "bench")]
        _ if same(c, b"txtest") => txtest(&mut t),
        #[cfg(feature = "bench")]
        _ if same(c, b"bk") => bk(&mut t),
        #[cfg(feature = "bench")]
        _ if same(c, b"si") => si(&mut t),
        #[cfg(feature = "bench")]
        _ if same(c, b"bt") => bt(&mut t),
        #[cfg(feature = "bench")]
        _ if same(c, b"rx") => rx(&mut t),
        #[cfg(feature = "bench")]
        _ if same(c, b"lcd") => lcd(&mut t),
        // TERM-38 (decision 151): a user build knows the bench commands by name only.
        #[cfg(not(feature = "bench"))]
        _ if BENCH_ONLY.iter().any(|b| same(c, b.as_bytes())) => Err(not_in_build(c)),
        _ if same(c, b"load") => Ok(Done::Load),
        _ if same(c, b"clear") => {
            if !same(t.word("clear channels N: deletes every channel")?, b"channels") {
                return Err(msg!("clear channels N: deletes every channel"));
            }
            let n = st.count();
            match t.next()?.and_then(|k| if let Tok::Word(w) = k { number(w) } else { None }) {
                Some(k) if k == n as u32 && n > 0 => {
                    app.erase_channels();
                    Ok(Done::Said(msg!("All {} channels deleted", n)))
                }
                _ if n == 0 => Err(msg!("There are no channels to delete")),
                _ => Err(msg!("This deletes all {} channels. To go ahead, type: clear channels {}", n, n)),
            }
        }
        _ => Err(msg!("{}: no such command; help lists them", Bytes(c))),
    }
}

// ---- Remote keys (LINK-19) ----

/// `key NAME [short|long|down|up]`: a keypad or side key, as the link's `KEY` presses it (LINK-19).
fn key<S: ChannelStore>(app: &mut App, st: &S, t: &mut Toks) -> Result<Done, Msg> {
    use crate::remote::Press;
    let usage = "key NAME [long|down|up]: ui keys lists the names";
    let name = t.word(usage)?;
    let Some(code) = crate::uistate::key_code(name) else { return Err(msg!("{}: no such key; ui keys lists them", Bytes(name))) };
    let how = match t.next()? {
        None => Press::Short,
        Some(Tok::Word(w)) if same(w, b"short") => Press::Short,
        Some(Tok::Word(w)) if same(w, b"long") => Press::Long,
        Some(Tok::Word(w)) if same(w, b"down") => Press::Down,
        Some(Tok::Word(w)) if same(w, b"up") => Press::Up,
        _ => return Err(msg!("{}", usage)),
    };
    let now = app.now;
    match app.remote_press(code, how, crate::remote::BY_TERM, now, st) {
        Ok(()) => Ok(Done::Said(msg!("key {} {}", crate::uistate::key_name(code), ["up", "down", "short", "long"][how as usize]))),
        Err(r) => Err(msg!("key: {}", r.text())),
    }
}

/// `knob +N|-N`: knob steps, as the link's `KEY` turns it (LINK-19).
fn knob<S: ChannelStore>(app: &mut App, st: &S, t: &mut Toks) -> Result<Done, Msg> {
    let usage = "knob +N or -N: 1 to 20 steps, + clockwise";
    let w = t.word(usage)?;
    let (neg, digits) = match w.split_first() {
        Some((b'-', d)) => (true, d),
        Some((b'+', d)) => (false, d),
        _ => (false, w),
    };
    let n = number(digits).filter(|&n| (1..=20).contains(&n)).ok_or_else(|| msg!("{}", usage))? as i8;
    let now = app.now;
    match app.remote_knob(if neg { -n } else { n }, now, st) {
        Ok(()) => Ok(Done::Said(msg!("knob {}{}", if neg { "-" } else { "+" }, n as u32))),
        Err(r) => Err(msg!("knob: {}", r.text())),
    }
}

// ---- The crossband repeater (XB-20) ----

/// `xb`, `xb start`, `xb stop` (XB-20, decision 161): the crossband repeater's state; started as its # starts it
/// (XB-2's checks, its screen opened from home if it isn't up); stopped as a key stops it. Who ran it is logged
/// (XB-20.6: the terminal, or the link's port).
fn xb(app: &mut App, t: &mut Toks) -> Result<Done, Msg> {
    use crate::screens::xb;
    use crate::{Dest, Screen};
    let by = xb::BY_CMD | app.cmd_by;
    let sides = |app: &App| {
        let c = app.set.xb;
        let mut m = Msg::new();
        for (i, s) in [c.side1, c.side2].into_iter().enumerate() {
            put!(m, "{}{} {}", if i == 0 { "" } else { " <-> " }, ["A", "B", "C"][s as usize % 3], Bytes(mhz5(app.slot(s).freq()).as_bytes()));
        }
        m
    };
    match t.next()? {
        None => {
            let m = sides(app);
            if xb::running(app) {
                Ok(Done::Said(msg!("Crossband: running, {}, started by {}", Bytes(m.as_bytes()), xb::by_word(app.radio.xb_by))))
            } else {
                Ok(Done::Said(msg!("Crossband: off ({})", Bytes(m.as_bytes()))))
            }
        }
        Some(Tok::Word(w)) if same(w, b"start") => {
            if xb::running(app) {
                return Err(msg!("xb start: already running; xb stop stops it"));
            }
            if app.radio.keyed.is_some() || app.remote_keyed() {
                return Err(msg!("xb start: transmitting: send it after the unkey"));
            }
            if app.screen().modal() {
                return Err(msg!("xb start: not now: the radio shows {}", app.screen().name()));
            }
            if app.set.chan_only {
                return Err(msg!("xb start: channel-only mode (PIN-5): no repeater"));
            }
            if !matches!(app.screen(), Screen::Crossband(_)) {
                app.go_home(); // XB-20.4: its screen up while it runs, home under it
                app.open(Dest::Crossband);
            }
            match xb::try_start(app, by) {
                Ok(()) => Ok(Done::Said(msg!("Crossband started: {}", Bytes(sides(app).as_bytes())))),
                Err(why) => Err(msg!("xb start: can't start: {}", Bytes(why.as_bytes()))),
            }
        }
        Some(Tok::Word(w)) if same(w, b"stop") => {
            if !xb::running(app) {
                return Err(msg!("xb stop: the repeater isn't running"));
            }
            xb::stop(app, by, xb::XB_CMD);
            Ok(Done::Said(msg!("Crossband stopped")))
        }
        _ => Err(msg!("xb, xb start or xb stop: the crossband repeater")),
    }
}

// ---- Debug (TERM-21 to TERM-24) ----

use super::debug::{Job, Kind};
#[cfg(feature = "bench")]
use super::debug::{BK_REGS, MEM_MAX};

/// TERM-38 (decision 151): the bench builds' commands (bring-up and tx-bench, the `bench` cargo feature). A receive
/// or tx build compiles them out but knows their names, so it can say where they are; `gps` alone (the last fix)
/// is in every build, its `query`, `nmea` and `standby` are the bench's.
pub const BENCH_ONLY: [&str; 11] = ["mem", "mem32", "gpio", "bk", "si", "bt", "rx", "mic", "af", "txtest", "lcd"];

/// TERM-38: what a user build says to a bench command.
#[cfg(not(feature = "bench"))]
fn not_in_build(c: &[u8]) -> Msg {
    msg!("{}: not in this build: a bench build has it", Bytes(c))
}

#[cfg(feature = "bench")]
/// Hex, with or without 0x, up to 8 digits.
fn hexnum(w: &[u8]) -> Option<u32> {
    let w = w.strip_prefix(b"0x").or_else(|| w.strip_prefix(b"0X")).unwrap_or(w);
    if w.is_empty() || w.len() > 8 {
        return None;
    }
    w.iter().try_fold(0u32, |n, &c| Some(n << 4 | (c as char).to_digit(16)?))
}

#[cfg(feature = "bench")]
fn hex_word(t: &mut Toks, what: &str) -> Result<u32, Msg> {
    hexnum(t.word(what)?).ok_or_else(|| msg!("{}", what))
}

#[cfg(feature = "bench")]
fn mem(t: &mut Toks, words: bool) -> Result<Done, Msg> {
    let usage = if words { "mem32 ADDR [WORDS]: hex address, word-aligned; up to 256 words" } else { "mem ADDR [LEN]: hex address, up to 1024 bytes" };
    let addr = hex_word(t, usage)?;
    let n = match t.next()? {
        Some(Tok::Word(w)) => number(w).ok_or_else(|| msg!("{}", usage))?,
        _ => 16,
    };
    let len = if words { n * 4 } else { n };
    if n == 0 || len > MEM_MAX as u32 || (words && !addr.is_multiple_of(4)) {
        return Err(msg!("{}", usage));
    }
    // TERM-22: refused here, before anything goes to the radio, when any of it is outside the readable map.
    if let Err(why) = ferrite_core::memmap::check(addr, len, words) {
        return Err(msg!("{}", why));
    }
    Ok(Done::Debug(Job::new(Kind::Mem { addr, len: len as u16, words })))
}

#[cfg(feature = "bench")]
fn bk(t: &mut Toks) -> Result<Done, Msg> {
    const USAGE: &str = "bk CHIP REG [VALUE], bk CHIP dump: chip 0 (VHF) or 1 (UHF and HF), hex register and value; bk settle; bk save; bk turns [reset]";
    let chip = match t.word(USAGE)? {
        b"0" => 0,
        b"1" => 1,
        w if same(w, b"settle") => return settle(t),
        w if same(w, b"save") => return bk_save(t),
        w if same(w, b"turns") => return turns(t),
        _ => return Err(msg!("{}", USAGE)),
    };
    let w = t.word(USAGE)?;
    if same(w, b"dump") {
        return Ok(Done::Debug(Job::new(Kind::BkDump { chip })));
    }
    let reg = hexnum(w).filter(|r| *r < BK_REGS as u32).ok_or_else(|| msg!("{}", USAGE))? as u8;
    match t.next()? {
        None => Ok(Done::Debug(Job::new(Kind::BkRead { chip, reg }))),
        Some(Tok::Word(v)) => {
            let val = hexnum(v).filter(|v| *v <= 0xFFFF).ok_or_else(|| msg!("{}", USAGE))? as u16;
            if let Some(why) = ferrite_core::msg::bk_write_refused(reg, val) {
                return Err(msg!("refused: {}", why));
            }
            Ok(Done::Debug(Job::new(Kind::BkWrite { chip, reg, val })))
        }
        _ => Err(msg!("{}", USAGE)),
    }
}

#[cfg(feature = "bench")]
/// TERM-33 (decision 132, a bench test): `txtest` asks what is armed, `txtest off` disarms, `txtest MODE [FILTERS]
/// [LEVEL]` arms MODE (4fsk, sweep, afsk) for the next PTT key-up with the BK4829's TX audio `voice`, `flat` or
/// `data` (the default, as an APRS burst goes out) at LEVEL percent of the mode's nominal (10 to 150,
/// default 100); `txtest afsk-bk [LEVEL]` the BK4829 tone generator's AFSK, which takes no filters word (decision
/// 134); `txtest suite` every test in one key-up (TERM-34, decision 139), no other word. Anything else is refused
/// here, before anything is sent.
fn txtest(t: &mut Toks) -> Result<Done, Msg> {
    use ferrite_core::txtest::{Arm, Ask, Filters, Signal, LEVEL_DEFAULT, LEVEL_MAX, LEVEL_MIN};
    const USAGE: &str = "txtest 4fsk|sweep|afsk [voice|flat|data] [10-150 %], txtest afsk-bk [10-150 %], txtest suite, txtest off; txtest alone shows what is armed";
    let level = |w: &[u8]| number(w).filter(|n| (LEVEL_MIN as u32..=LEVEL_MAX as u32).contains(n)).map(|n| n as u8).ok_or_else(|| msg!("{}", USAGE));
    let ask = match t.next()? {
        None => Ask::Show,
        Some(Tok::Word(w)) if same(w, b"off") => Ask::Set(None),
        Some(Tok::Word(w)) => {
            let signal = Signal::parse(w).ok_or_else(|| msg!("{}", USAGE))?;
            let mut arm = Arm { signal, filters: signal.default_filters(), level: LEVEL_DEFAULT };
            match t.next()? {
                None => {}
                Some(_) if !signal.takes_level() => return Err(msg!("{}", USAGE)),
                Some(Tok::Word(f)) => match Filters::parse(f) {
                    Some(_) if !signal.dac() => return Err(msg!("{}", USAGE)),
                    Some(f) => {
                        arm.filters = f;
                        match t.next()? {
                            None => {}
                            Some(Tok::Word(l)) => arm.level = level(l)?,
                            _ => return Err(msg!("{}", USAGE)),
                        }
                    }
                    None => arm.level = level(f)?,
                },
                _ => return Err(msg!("{}", USAGE)),
            }
            Ask::Set(Some(arm))
        }
        _ => return Err(msg!("{}", USAGE)),
    };
    if t.next()?.is_some() {
        return Err(msg!("{}", USAGE));
    }
    Ok(Done::Debug(Job::new(Kind::TxTest(ask))))
}

#[cfg(feature = "bench")]
/// TERM-41 (a bench test, the power bench): `bk save` shows Battery Save's cycle; `bk save lp` puts Low Power Mode's
/// in place of the level's, `bk save ON OFF [HOLD_S [RECENT_S]]` any (ms, then seconds; hold and recent default
/// 0), `bk save normal` gives the level back. RAM only.
fn bk_save(t: &mut Toks) -> Result<Done, Msg> {
    use ferrite_core::rxsleep::{Cycle, LOW_POWER};
    const USAGE: &str = "bk save [lp | normal | ON_MS OFF_MS [HOLD_S [RECENT_S]]]: on/off 10-5000 ms, hold and recent 0-3600 s";
    let mut n = [None; 4];
    let mut word = None;
    for (i, slot) in n.iter_mut().enumerate() {
        match t.next()? {
            None => break,
            Some(Tok::Word(w)) if i == 0 && (same(w, b"lp") || same(w, b"normal")) => {
                word = Some(same(w, b"lp"));
                break;
            }
            Some(Tok::Word(w)) => *slot = Some(number(w).ok_or_else(|| msg!("{}", USAGE))?),
            _ => return Err(msg!("{}", USAGE)),
        }
    }
    if t.next()?.is_some() {
        return Err(msg!("{}", USAGE));
    }
    let set = match (word, n) {
        (Some(true), _) => Some(Some(LOW_POWER)),
        (Some(false), _) => Some(None),
        (None, [None, ..]) => None,
        (None, [Some(on), Some(off), hold, recent]) => {
            let ms = |v: u32| (10..=5000).contains(&v);
            let s = |v: Option<u32>| v.filter(|&v| v <= 3600).map(|v| v * 1000).ok_or_else(|| msg!("{}", USAGE));
            if !ms(on) || !ms(off) {
                return Err(msg!("{}", USAGE));
            }
            let (hold_ms, recent_ms) = (s(hold.or(Some(0)))?, s(recent.or(Some(0)))?);
            Some(Some(Cycle { on_ms: on as u16, off_ms: off as u16, hold_ms, recent_ms }))
        }
        _ => return Err(msg!("{}", USAGE)),
    };
    Ok(Done::Debug(Job::new(Kind::Save(set))))
}

#[cfg(feature = "bench")]
/// TERM-40 (a bench test, the power bench): `lcd` shows the display's bench state; `lcd idle|partial|sleep on|off`,
/// `lcd fps N` (1-20, 0 normal), `lcd bl N|normal`, `lcd keys N|normal` (percent) change one thing; `lcd reset`;
/// `lcd bus` times a whole-screen fill.
fn lcd(t: &mut Toks) -> Result<Done, Msg> {
    use ferrite_core::msg::LcdSet;
    const USAGE: &str = "lcd [idle|partial|sleep on|off] [fps 0-20] [bl|keys 0-100|normal] [paint old|new] [bus] [reset]";
    let bad = || msg!("{}", USAGE);
    fn word<'a>(t: &mut Toks<'a>) -> Result<Option<&'a [u8]>, Msg> {
        match t.next()? {
            None => Ok(None),
            Some(Tok::Word(w)) => Ok(Some(w)),
            _ => Err(msg!("{}", USAGE)),
        }
    }
    let Some(what) = word(t)? else { return Ok(Done::Debug(Job::new(Kind::Lcd(LcdSet::Ask)))) };
    if same(what, b"paint") {
        // DISP-29: `old` brings back clear-then-paint (a new screen cleared first, regions drawn once per band), to
        // compare by eye; `new` (the default) paints each pixel once. RAM only, here in the terminal.
        use ferrite_ui::canvas::{set_single_pass, single_pass};
        let on = match word(t)? {
            Some(a) if same(a, b"old") => Some(false),
            Some(a) if same(a, b"new") => Some(true),
            None => None,
            Some(_) => return Err(bad()),
        };
        if t.next()?.is_some() {
            return Err(bad());
        }
        if let Some(on) = on {
            set_single_pass(on);
        }
        return Ok(Done::Said(msg!("paint {}", if single_pass() { "new: each pixel once" } else { "old: clear, then paint" })));
    }
    let set = if same(what, b"reset") {
        LcdSet::Reset
    } else if same(what, b"bus") {
        LcdSet::Bus
    } else {
        let arg = word(t)?.ok_or_else(bad)?;
        let on = || if same(arg, b"on") { Some(true) } else if same(arg, b"off") { Some(false) } else { None };
        let pct = |max: u32| if same(arg, b"normal") { Some(None) } else { number(arg).filter(|&n| n <= max).map(|n| Some(n as u8)) };
        match what {
            _ if same(what, b"idle") => LcdSet::Idle(on().ok_or_else(bad)?),
            _ if same(what, b"partial") => LcdSet::Partial(on().ok_or_else(bad)?),
            _ if same(what, b"sleep") => LcdSet::Sleep(on().ok_or_else(bad)?),
            _ if same(what, b"fps") => LcdSet::Fps(pct(20).ok_or_else(bad)?.unwrap_or(0)),
            _ if same(what, b"bl") => LcdSet::Bl(pct(100).ok_or_else(bad)?),
            _ if same(what, b"keys") => LcdSet::Keys(pct(100).ok_or_else(bad)?),
            _ => return Err(bad()),
        }
    };
    if t.next()?.is_some() {
        return Err(bad());
    }
    Ok(Done::Debug(Job::new(Kind::Lcd(set))))
}

#[cfg(feature = "bench")]
/// WATCH-17.1 (decision 262, a bench measure): `bk turns` shows each slot's whole turns on a chip and their total ms
/// since boot or the last reset; `bk turns reset` shows them, then zeroes them.
fn turns(t: &mut Toks) -> Result<Done, Msg> {
    const USAGE: &str = "bk turns [reset]: each slot's turns on a chip and their ms";
    let reset = match t.next()? {
        None => false,
        Some(Tok::Word(w)) if same(w, b"reset") => true,
        _ => return Err(msg!("{}", USAGE)),
    };
    if t.next()?.is_some() {
        return Err(msg!("{}", USAGE));
    }
    Ok(Done::Debug(Job::new(Kind::Turns { reset })))
}

#[cfg(feature = "bench")]
/// TERM-31 (decision 119, a bench test): `bk settle` says the BK4829 settle waits in force, `bk settle N` sets both
/// to N ms, `bk settle W S` the wake and the sleep wait apart; 0 to 20 ms, decimal; 0 is stock's (no wait).
fn settle(t: &mut Toks) -> Result<Done, Msg> {
    const USAGE: &str = "bk settle [WAKE [SLEEP]]: ms, 0 to 20 (0 is stock's, no wait); bk settle alone shows them";
    let mut ms = [None; 2];
    for m in ms.iter_mut() {
        *m = match t.next()? {
            None => break,
            Some(Tok::Word(w)) => Some(number(w).filter(|&n| n <= ferrite_core::settle::MAX_MS as u32).ok_or_else(|| msg!("{}", USAGE))? as u8),
            _ => return Err(msg!("{}", USAGE)),
        };
    }
    if t.next()?.is_some() {
        return Err(msg!("{}", USAGE));
    }
    let set = ms[0].and_then(|w| ferrite_core::settle::Settle::new(w, ms[1].unwrap_or(w)));
    Ok(Done::Debug(Job::new(Kind::Settle(set))))
}

#[cfg(feature = "bench")]
/// TERM-29 (decision 114, a bench test): `rx both` holds both band receive selects on, `rx one` goes back to
/// the normal one, `rx` alone says which. The Radio task refuses it while keyed, in HF mode and in crossband.
fn rx(t: &mut Toks) -> Result<Done, Msg> {
    const USAGE: &str = "rx both, rx one, or rx to ask: both band receive selects held on (a bench test)";
    let on = match t.next()? {
        None => None,
        Some(Tok::Word(w)) if same(w, b"both") => Some(true),
        Some(Tok::Word(w)) if same(w, b"one") => Some(false),
        _ => return Err(msg!("{}", USAGE)),
    };
    if t.next()?.is_some() {
        return Err(msg!("{}", USAGE));
    }
    Ok(Done::Debug(Job::new(Kind::RxBoth(on))))
}

/// TERM-30, TERM-38: a user build's `gps`, the last GGA; the module's CASIC sends (TERM-36) are a bench build's.
#[cfg(not(feature = "bench"))]
fn gps(t: &mut Toks) -> Result<Done, Msg> {
    match t.next()? {
        None => Ok(Done::Debug(Job::new(Kind::Gga))),
        Some(Tok::Word(w)) if [&b"query"[..], b"nmea", b"standby"].iter().any(|b| same(w, b)) => Err(msg!("gps {}: not in this build: a bench build has it", Bytes(w))),
        _ => Err(msg!("gps alone: query, nmea and standby need a bench build")),
    }
}

/// TERM-30, TERM-36: `gps` (the last GGA), `gps query [0|1|2|4|6]` (CAS06, the module's answer), `gps nmea off`
/// (CAS03, all NMEA off) and `gps nmea on` (Ferrite's NMEA set, GPS-34), `gps standby N [quiet]` (CAS12 for N s; quiet: NMEA off
/// first, not on after): nothing else goes to the module ([`ferrite_core::gpspower::casic_refused`]).
#[cfg(feature = "bench")]
fn gps(t: &mut Toks) -> Result<Done, Msg> {
    use ferrite_core::gpspower::{casic_refused, CAS_NMEA, CAS_QUERY, CAS_STANDBY};
    const USAGE: &str = "gps, gps query [0|1|2|4|6] (CAS06), gps nmea off|on (CAS03), gps standby N [quiet] (CAS12, 1-65535 s)";
    let mut quiet = false;
    let (id, arg) = match t.next()? {
        None => return Ok(Done::Debug(Job::new(Kind::Gga))),
        Some(Tok::Word(w)) if same(w, b"query") => match t.next()? {
            None => (CAS_QUERY, 0),
            Some(Tok::Word(n)) => (CAS_QUERY, number(n).filter(|&n| n <= 9).ok_or_else(|| msg!("{}", USAGE))? as u16),
            _ => return Err(msg!("{}", USAGE)),
        },
        Some(Tok::Word(w)) if same(w, b"nmea") => match t.next()? {
            Some(Tok::Word(o)) if same(o, b"off") => (CAS_NMEA, 0),
            Some(Tok::Word(o)) if same(o, b"on") => (CAS_NMEA, 1),
            _ => return Err(msg!("{}", USAGE)),
        },
        Some(Tok::Word(w)) if same(w, b"standby") => {
            let n = match t.next()? {
                Some(Tok::Word(n)) => number(n).filter(|&n| n <= 65535).ok_or_else(|| msg!("{}", USAGE))? as u16,
                _ => return Err(msg!("{}", USAGE)),
            };
            match t.next()? {
                None => {}
                Some(Tok::Word(q)) if same(q, b"quiet") => quiet = true,
                _ => return Err(msg!("{}", USAGE)),
            }
            (CAS_STANDBY, n)
        }
        _ => return Err(msg!("{}", USAGE)),
    };
    if t.next()?.is_some() {
        return Err(msg!("{}", USAGE));
    }
    if let Some(why) = casic_refused(id, arg) {
        return Err(msg!("refused: {}", why));
    }
    Ok(Done::Debug(Job::new(Kind::Gps { id, arg, quiet })))
}

#[cfg(feature = "bench")]
/// TERM-27: `bt AT+VERS?`, one read-only query to the Bluetooth module.
fn bt(t: &mut Toks) -> Result<Done, Msg> {
    const USAGE: &str = "bt AT, bt AT+HELP or bt AT+WORD? (queries only); bt stat; bt baud 9600; bt pace 20 50";
    let Some(Tok::Word(w)) = t.next()? else { return Err(msg!("{}", USAGE)) };
    // `bt stat`: USART1's counters; `bt baud N`: USART1 at N for this power-on (the MCU's side; the module untouched).
    if same(w, b"stat") && t.clone().next()?.is_none() {
        return Ok(Done::Debug(Job::new(Kind::BtStat(0, None))));
    }
    // `bt linkz`: the link codecs timed on the radio (decision 285, step 3)
    if same(w, b"linkz") && t.clone().next()?.is_none() {
        return Ok(Done::Debug(Job::new(Kind::Linkz)));
    }
    // `bt pace B G`: LINK-36's bursts of B bytes and G ms gaps, in RAM until a restart (B 0: unpaced).
    if same(w, b"pace") {
        const PACE: &str = "bt pace B G: bursts of B bytes (0 to 255, 0: unpaced), G ms apart (0 to 1000)";
        let mut num = || match t.next() {
            Ok(Some(Tok::Word(n))) => number(n),
            _ => None,
        };
        let p = num().zip(num()).and_then(|(b, g)| ferrite_core::link::BtPace::new(b, g));
        let Some(p) = p.filter(|_| matches!(t.next(), Ok(None))) else { return Err(msg!("{}", PACE)) };
        return Ok(Done::Debug(Job::new(Kind::BtStat(0, Some(p)))));
    }
    if same(w, b"baud") {
        const RATES: &str = "bt baud 9600, 19200, 38400, 57600 or 115200 (USART1 only: the module is never told)";
        let r = match t.next()? {
            Some(Tok::Word(n)) => number(n).filter(|r| ferrite_core::msg::BT_BENCH_RATES.contains(r)),
            _ => None,
        };
        let Some(r) = r.filter(|_| matches!(t.next(), Ok(None))) else { return Err(msg!("{}", RATES)) };
        return Ok(Done::Debug(Job::new(Kind::BtStat(r, None))));
    }
    if t.next()?.is_some() || w.len() > 16 {
        return Err(msg!("{}", USAGE));
    }
    if let Some(why) = ferrite_core::msg::bt_query_refused(w) {
        return Err(msg!("refused: {}", why));
    }
    let mut text = [0u8; 16];
    text[..w.len()].copy_from_slice(w);
    Ok(Done::Debug(Job::new(Kind::Bt { text, n: w.len() as u8 })))
}

#[cfg(feature = "bench")]
fn si(t: &mut Toks) -> Result<Done, Msg> {
    const USAGE: &str = "si BYTES... [reply N], si prop P [V]: hex; the reply starts with the status byte";
    let mut cmd = [0u8; 8];
    let (mut n, mut reply) = (0usize, 8u32);
    if let Some(Tok::Word(w)) = t.clone().next()? {
        if same(w, b"prop") {
            t.next()?;
            let p = hex_word(t, USAGE)?.min(0xFFFF);
            return Ok(Done::Debug(Job::new(match t.next()? {
                None => Kind::Si { cmd: [0x13, 0, (p >> 8) as u8, p as u8, 0, 0, 0, 0], n: 4, reply: 4 },
                Some(Tok::Word(v)) => {
                    let v = hexnum(v).filter(|v| *v <= 0xFFFF).ok_or_else(|| msg!("{}", USAGE))?;
                    Kind::Si { cmd: [0x12, 0, (p >> 8) as u8, p as u8, (v >> 8) as u8, v as u8, 0, 0], n: 6, reply: 1 }
                }
                _ => return Err(msg!("{}", USAGE)),
            })));
        }
    }
    while let Some(tok) = t.next()? {
        let Tok::Word(w) = tok else { return Err(msg!("{}", USAGE)) };
        if same(w, b"reply") {
            reply = t.next()?.and_then(|k| if let Tok::Word(w) = k { number(w) } else { None }).filter(|r| (1..=16).contains(r)).ok_or_else(|| msg!("reply: 1 to 16 bytes"))?;
            break;
        }
        if n == cmd.len() {
            return Err(msg!("si: at most 8 command bytes"));
        }
        cmd[n] = hexnum(w).filter(|b| *b <= 0xFF).ok_or_else(|| msg!("{}", USAGE))? as u8;
        n += 1;
    }
    if n == 0 {
        return Err(msg!("{}", USAGE));
    }
    Ok(Done::Debug(Job::new(Kind::Si { cmd, n: n as u8, reply: reply as u8 })))
}

// ---- Dump ----

/// Where a dump is: the section and the item in it.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct DumpPos {
    sec: u8,
    i: u16,
}

const HEAD: [&str; 3] = [
    "# One command per line, as typed after : in the terminal; :load reads it back. # starts a note.",
    "# Channels and zones are merged by number: lines replace what they name, nothing else is deleted.",
    "# Settings, by their numbers in the radio's menu",
];

/// The dump's next line into `out`; false once it is over (TERM-13: streamed, never held whole).
pub fn dump_line<S: ChannelStore>(app: &App, st: &S, dev: &Device, p: &mut DumpPos, out: &mut Line) -> bool {
    out.clear();
    let settable = groups::settable().count();
    loop {
        let i = p.i;
        p.i += 1;
        match p.sec {
            0 if i == 0 => {
                put!(out, "# Ferrite dump v1, firmware {}", VERSION.strip_prefix('v').unwrap_or(VERSION));
                return true;
            }
            0 if (i as usize) <= HEAD.len() => {
                out.push(HEAD[i as usize - 1].as_bytes());
                return true;
            }
            1 if (i as usize) < settable => {
                if let Some(path) = groups::settable().nth(i as usize).filter(|p| groups::greyed(app, p).is_none()) {
                    set_line(app, dev, path, out);
                    return true;
                }
            }
            1 if i as usize == settable => {
                put!(out, "# 0 9 4 Region: {} (changes only on the radio)", app.set.region_name());
                return true;
            }
            1 if i as usize == settable + 1 => {
                if let Some(c) = app.set.boot_callsign() {
                    put!(out, "# 0 8 6 Callsign: {} (set on the radio)", Bytes(c));
                    return true;
                }
            }
            2 if i == 0 => {
                out.push("# Zones (a channel's zone=N is one of these)".as_bytes());
                return true;
            }
            2 if i <= st.zones() as u16 => {
                zone_line(i as u8 - 1, st, out);
                return true;
            }
            3 if i == 0 => {
                out.push("# Range presets (0 1 4)".as_bytes());
                return true;
            }
            3 if i <= st.data().presets() as u16 => {
                if let Some(r) = st.data().preset(i as u8 - 1) {
                    preset_line(i as u8 - 1, &r, out);
                    return true;
                }
            }
            4 if i == 0 => {
                num(out, b"# Channels (", st.count() as u32);
                out.push(b" of 1000)");
                return true;
            }
            4 if i <= st.count() => {
                if let Some(c) = st.by_number(i - 1) {
                    ch_line(&c, st, out);
                    return true;
                }
            }
            5 if i == 0 => {
                if let Some(h) = app.set.home_ch.filter(|&h| st.by_num(h).is_some()) {
                    num(out, b"home ", h as u32 + 1);
                    return true;
                }
            }
            6 if i == 0 => {
                out.push(b"end");
                return true;
            }
            6 => return false,
            _ => (p.sec, p.i) = (p.sec + 1, 0),
        }
    }
}

/// The console's lists: help, the settings not at their defaults, one channel.
pub fn list_line<S: ChannelStore>(app: &App, st: &S, dev: &Device, live: &ferrite_core::msg::LiveAll, l: List, i: u16, out: &mut Line) -> bool {
    out.clear();
    match l {
        List::Ui | List::UiTexts => crate::uistate::line(app, st, dev, live, i, out), // the texts: the link's own
        List::UiKeys => crate::uistate::key_line(i, out),
        List::UiStatus if i == 0 => {
            crate::uistate::status_line(app, live, out);
            true
        }
        List::UiStatus => false,
        List::Help => match super::piece(HELP.as_bytes(), b'\n', i as usize).or_else(|| super::piece(HELP_BENCH.as_bytes(), b'\n', (i as usize).wrapping_sub(HELP_LINES))) {
            Some(h) => {
                out.push(h);
                true
            }
            None => false,
        },
        List::Changed => {
            let mut changed = groups::settable().filter(|p| {
                let mut v = Buf::<24>::new();
                value(app, dev, p, &mut v)
            });
            match changed.nth(i as usize) {
                Some(p) => {
                    set_line(app, dev, p, out);
                    true
                }
                None if i == 0 => {
                    out.push("All settings are at their defaults.".as_bytes());
                    true
                }
                None => false,
            }
        }
        List::Slots => {
            if i as usize >= ferrite_core::msg::SLOTS {
                return false;
            }
            slot_line(app, st, i as u8, out);
            true
        }
        List::Show(n) => match (i, st.by_num(n)) {
            (0, Some((c, _))) => {
                ch_line(&c, st, out);
                true
            }
            _ => false,
        },
    }
}

const HELP: &str = concat!(
    "Commands: type : then one of these, and Enter. A dump is made of the same lines.\n",
    "\n",
    "  set 0 7 1 Dark         a setting, by its number in the radio's menu\n",
    "  get 0 7 1   get        one setting; every setting not at its default\n",
    "  ch 12                  channel 12, as its ch line\n",
    "  slot, slot A           the slots now; slot A 146.52 nfm high tone 100.0, slot A vfo,\n",
    "                         slot B active, slot A offset off|-0.6, slot A split 446.0:\n",
    "                         as the keys would (a channel slot: add vfo)\n",
    "  ch 12 power high       one field: name rx offset tx mode bw tone rxtone txtone\n",
    "                         power zone step squelch busy reverse skip aprs rxonly hidden priority\n",
    "  ch 12 \"Name\" 146.52 offset=+0.6 tone=100.0 ...   a whole channel\n",
    "  ch 12 delete           delete channel 12\n",
    "  zone 2 \"Repeaters\"     a zone (made in order); fields scan shown resume ...\n",
    "  preset 1 \"2 m\" 144 148 a range preset; fields step mode bw\n",
    "  home 12   home none    the home channel\n",
    "  dump                   everything, as these lines; save it with your terminal's log\n",
    "  load                   read a dump back: send the file, it ends at the line end\n",
    "  clear channels N       delete every channel (N: how many there are)\n",
    "  gps                    the GPS module's last GGA sentence, its age and heights\n",
    "  xb   xb start  xb stop the crossband repeater: its state; start it as its # does; stop it",
);
/// The lines in [`HELP`].
const HELP_LINES: usize = {
    let (h, mut i, mut n) = (HELP.as_bytes(), 0, 1);
    while i < h.len() {
        n += (h[i] == b'\n') as usize;
        i += 1;
    }
    n
};
/// TERM-38: the bench commands' help in a bench build, or where they are in a user build.
#[cfg(feature = "bench")]
const HELP_BENCH: &str = concat!(
    "\n",
    "Bench builds only (bring-up, tx-bench; hex addresses, registers and values; Esc stops):\n",
    "  mem 20000000 64        bytes of MCU memory (flash, RAM)\n",
    "  mem32 40010808 4       32-bit words: use this for peripheral registers\n",
    "  gpio                   ports A to E: input and output bits, 15 to 0\n",
    "  bk 0 30   bk 0 dump    a BK4829 register, or all of them (0 VHF, 1 UHF/HF)\n",
    "  bk 1 47 6142           write one, then read it back (no TX/PA bits, no GPIO)\n",
    "  si 10 00   si prop 4001 [0003]   an SI4732 command; get or set a property\n",
    "  bt AT+VERS?            ask the Bluetooth module (AT, AT+HELP, AT+WORD? only)\n",
    "  bt stat, bt baud N     USART1's byte and error counts; its rate (the MCU's side)\n",
    "  bt pace B G            Bluetooth sends B-byte bursts G ms apart (RAM only)\n",
    "  rx both   rx one       bench test: VHF and UHF receive selects both on, or one\n",
    "  bk settle 2 1          bench test: BK4829 wake and sleep waits, ms (0 stock's)\n",
    "  gps query [0|1|2|4|6]  ask the GPS module (CAS06: 0 firmware, 1 hardware, 6 chip)\n",
    "  gps nmea off   on      bench test: CAS03, all NMEA off (RAM only); on: Ferrite's set\n",
    "  gps standby 60         bench test: CAS12, the module asleep 60 s (cable log)\n",
    "  gps standby 60 quiet   bench test: NMEA off, CAS12; NMEA on after the wake or 63 s\n",
    "  mic                    bench test: 32 ms of the mic tap (PA0) at 8 kHz, summed up\n",
    "  txtest 4fsk|sweep|afsk [voice|flat|data] [10-150]   bench test: that signal\n",
    "                         at that % on the next PTT key-up until release (tx builds)\n",
    "  txtest afsk-bk [10-150]  the same AFSK flags from the BK4829's tone generator\n",
    "  txtest suite           every transmit test in one key-up, 60 s (sdrsuite.py)\n",
    "  The radio's own logic may rewrite a register you set (a tune, Battery Save).",
);
#[cfg(not(feature = "bench"))]
const HELP_BENCH: &str = concat!(
    "\n",
    "Bench commands: mem mem32 gpio bk si bt rx mic txtest, gps query|nmea|standby:\n",
    "  not in this build; a bench build (bring-up, tx-bench) has them.",
);

// ---- Slots (TERM-35) ----

/// `slot`, `slot X`, `slot X [vfo] [FREQ] [fm|nfm|am] [low|mid|high] [tone T] [rxtone T] [txtone T] [active]`: the
/// slots' working fields, read or changed as the keypad would ([`crate::slotctl`]); a refusal changes nothing. T is
/// one word (off, 100.0, D023N) or a mode and a value (ctcss 100.0, dcs-n 023, dcs-i 023; TONE-2.1).
fn slot<S: ChannelStore>(app: &mut App, st: &S, t: &mut Toks) -> Result<Done, Msg> {
    const USAGE: &str = "slot A|B|C [vfo] [FREQ] [fm|nfm|am] [low|mid|high] [tone T] [rxtone T] [txtone T] [offset off|+N|-N] [split TX] [active]";
    let s = match t.next()? {
        None => return Ok(Done::List(List::Slots)),
        Some(Tok::Word(w)) if w.len() == 1 && (b'a'..=b'c').contains(&w[0].to_ascii_lowercase()) => w[0].to_ascii_lowercase() - b'a',
        _ => return Err(msg!("{}", USAGE)),
    };
    let mut c = crate::slotctl::SlotChange::default();
    while let Some(tok) = t.next()? {
        let Tok::Word(w) = tok else { return Err(msg!("{}", USAGE)) };
        if let Some(m) = ["fm", "nfm", "am"].iter().position(|m| same(w, m.as_bytes())) {
            c.mode = Some(m as u8);
        } else if let Some(p) = POWERS.iter().position(|p| same(w, p.as_bytes())) {
            c.power = Some(p as u8);
        } else if same(w, b"vfo") {
            c.vfo = true;
        } else if same(w, b"active") {
            c.active = true;
        } else if same(w, b"tone") {
            let v = tone_arg(t, "tone")?;
            (c.rx_tone, c.tx_tone) = (Some(v), Some(v));
        } else if same(w, b"rxtone") {
            c.rx_tone = Some(tone_arg(t, "rxtone")?);
        } else if same(w, b"txtone") {
            c.tx_tone = Some(tone_arg(t, "txtone")?);
        } else if same(w, b"offset") {
            let v = t.word("offset: off, or MHz with a sign, e.g. -0.6")?;
            c.offset = Some((if same(v, b"off") { 0 } else { offset(v)? }, false));
        } else if same(w, b"split") {
            c.offset = Some((freq(t.word("split: the TX frequency in MHz")?, "split")? as i32, true));
        } else if w.first().is_some_and(u8::is_ascii_digit) {
            c.freq = Some(freq(w, "slot: the frequency in MHz, e.g. 146.52")?);
        } else {
            return Err(msg!("{}: not a slot word; {}", Bytes(w), USAGE));
        }
    }
    if !c.is_empty() {
        app.slot_apply(s, &c, st).map_err(|why| msg!("slot {}: {}", Bytes(&[b'A' + s]), why))?;
    }
    let mut l = Line::new();
    slot_line(app, st, s, &mut l);
    Ok(Done::Said(Msg::from(&l.as_bytes()[..l.len().min(96)])))
}

/// "A* vfo 146.52000 mode=NFM power=high tone=100.0 offset=+0.6" or "B  ch 12 "Club" 145.23000 ...": the active
/// slot starred.
pub fn slot_line<S: ChannelStore>(app: &App, st: &S, s: u8, out: &mut Line) {
    let g = app.slot_state(s);
    out.push(&[b'A' + s, if g.active { b'*' } else { b' ' }, b' ']);
    match g.channel {
        Some(n) => {
            match st.by_num(n) {
                Some((c, _)) => head(b"ch ", n, c.name(), out),
                None => num(out, b"ch ", n as u32 + 1),
            }
            out.push(b" ");
        }
        None if g.aprs => out.push(b"aprs "),
        None => out.push(b"vfo "),
    }
    out.push(mhz5(g.freq).as_bytes());
    mode_word(g.mode, out);
    power_word(g.power, out);
    tones(g.tx_tone, g.rx_tone, out);
    if g.split || g.offset != 0 {
        offset_kv(g.split, g.offset, out);
    }
    if g.scanning {
        out.push(b" scanning");
    }
}
