//! The command line (TERM-12) and dump/load (TERM-13). One command per line, the same whether typed after `:`
//! or read back by `load`: a dump is a script of `set`, `zone`, `preset`, `ch` and `home` lines, so whatever
//! `load` accepts can also be typed, and every line is checked as the radio's own editors check it (TERM-6,
//! TERM-7). A line is applied whole or not at all.
//!
//! Nothing is buffered whole: [`dump_line`] makes one line at a time from where [`DumpPos`] says it is.

use super::{blocked, mhz5, parse_mhz};
use crate::chan::{Channel, ChannelStore, Name, FLAG_APRS, FLAG_RX_ONLY, FLAG_SKIP, MAX_CHANNELS, NO_ZONE};
use crate::data::{Op, RangePreset, ZoneCfg};
use crate::screens::groups::{self, SETTABLE, SUBS};
use crate::slot::{band_of, BAND_125M, CTCSS, DCS, DCS_FLAG, DCS_INV, DEFAULT_CFG, MODES, SLOT_MODES, SQ_GLOBAL, STEPS, STEP_12K5};
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
        fmt(slow!($t), &[])
    };
    ($t:literal, $($a:expr),+ $(,)?) => {
        fmt(slow!($t), &[$(Arg::from($a)),+])
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
                return Err(msg!("a name has no closing quote: end it with \""));
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

/// A channel or zone number as written, 1-based, to the radio's 0-based one.
fn index(w: &[u8], max: u32, what: &str) -> Result<u32, Msg> {
    match number(w) {
        Some(n) if (1..=max).contains(&n) => Ok(n - 1),
        _ => Err(msg!("{}: a number from 1 to {}", what, max)),
    }
}

/// "146.52", "146.520 00" (one word here): MHz from 18 MHz up to 1000 MHz.
fn freq(w: &[u8], what: &str) -> Result<Freq10, Msg> {
    match parse_mhz(w) {
        Some(f) if w.iter().all(|c| c.is_ascii_digit() || *c == b'.') && (mhz(18)..mhz(1000)).contains(&f) => Ok(f),
        _ => Err(msg!("{}: MHz from 18 to 1000, e.g. 146.52", what)),
    }
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
        let c = code.iter().fold(0u16, |n, &k| n * 8 + (k - b'0') as u16);
        if !DCS.contains(&c) {
            return Err(msg!("{}: D{} is not a standard DCS code", what, Bytes(code)));
        }
        return Ok(DCS_FLAG | if inv { DCS_INV } else { 0 } | c);
    }
    let t = match parse_mhz(w.strip_prefix(b"T").unwrap_or(w)) {
        // parse_mhz reads "100.0" as 100.0 MHz in 10 Hz: tenths of a hertz are its 10 kHz digit.
        Some(f) if f % 10_000 == 0 => f / 10_000,
        _ => return Err(bad()),
    };
    match CTCSS.iter().find(|&&c| c as u32 == t) {
        Some(&c) => Ok(c),
        None => Err(msg!("{}: {}.{} Hz is not a standard CTCSS tone", what, t / 10, t % 10)),
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
const BUSY: [&str; 3] = ["off", "carrier", "tone"];
const BW: [&str; 2] = ["wide", "narrow"];
/// Channel fields by name, in the order a `ch` line writes them.
const FIELDS: [&str; 18] =
    ["name", "rx", "offset", "mode", "bw", "tone", "rxtone", "txtone", "power", "zone", "step", "squelch", "busy", "reverse", "skip", "aprs", "rxonly", "tx"];

// ---- Channels ----

/// Channel `c` as its `ch` line: number, name and RX, then the fields always written, then those not at their
/// defaults. Every field `load` reads back is here, so a dump and a load give the same channel.
fn ch_line<S: ChannelStore>(c: &Channel, st: &S, out: &mut Line) {
    let g = &c.cfg;
    num(out, b"ch ", c.num as u32 + 1);
    out.push(b" ");
    quoted(c.name(), out);
    out.push(b" ");
    out.push(mhz5(c.rx).as_bytes());
    if g.split {
        // Split: TX is a frequency of its own (MENU 5's 4th choice)
        out.push(b" offset=split tx=");
        out.push(mhz5(g.offset.max(0) as Freq10).as_bytes());
    } else {
        out.push(b" offset=");
        offset_word(g.offset, out);
    }
    out.push(b" mode=");
    out.push(MODES[g.mode as usize % MODES.len()].as_bytes());
    if g.rx_tone == g.tx_tone {
        out.push(b" tone=");
        tone_word(g.tx_tone, out);
    } else {
        out.push(b" txtone=");
        tone_word(g.tx_tone, out);
        out.push(b" rxtone=");
        tone_word(g.rx_tone, out);
    }
    out.push(b" power=");
    out.push(POWERS[g.power as usize % 3].as_bytes());
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
        out.push(BUSY[g.busy as usize % 3].as_bytes());
    }
    for (on, w) in [(g.reverse, "reverse"), (c.flags & FLAG_SKIP != 0, "skip"), (c.flags & FLAG_APRS != 0, "aprs"), (c.flags & FLAG_RX_ONLY != 0, "rxonly")] {
        if on {
            out.push(b" ");
            out.push(w.as_bytes());
        }
    }
}

/// One field of a channel from `key` and its value (a bare flag word is "on").
fn field<S: ChannelStore>(c: &mut Channel, key: &[u8], v: Option<Tok>, st: &S) -> Result<(), Msg> {
    let k = FIELDS.iter().position(|f| same(key, f.as_bytes())).ok_or_else(|| msg!("{}: no such field; the fields are name, rx, offset, mode, bw, tone, power, zone, ...", Bytes(key)))?;
    let flag = |c: &mut Channel, bit: u8, v: Option<Tok>, what: &str| -> Result<(), Msg> {
        let on = match v {
            None => true,
            Some(Tok::Word(w)) => on_off(w, what)?,
            Some(Tok::Quoted(_)) => return Err(msg!("{}: on or off", what)),
        };
        c.flags = if on { c.flags | bit } else { c.flags & !bit };
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
            g.mode = pick(v, &MODES[..SLOT_MODES], "mode")?;
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
        _ => g.busy = pick(v, &BUSY, "busy")?,
    }
    Ok(())
}

/// What the radio's channel editor would refuse (CHAN-22, TX-9, TERM-7).
pub(crate) fn check(c: &Channel) -> Result<(), Msg> {
    if !(mhz(64)..mhz(1000)).contains(&c.rx) {
        return Err(msg!("rx: a channel is from 64 to 1000 MHz"));
    }
    if c.cfg.split && c.cfg.offset <= 0 {
        return Err(msg!("offset=split needs its TX frequency: tx=146.52"));
    }
    if !ferrite_core::txplan::relay_ok(c.cfg.rx_of(c.rx), c.cfg.tx_of(c.rx)) {
        return Err(msg!("Split TX needs the HF relay: not allowed with this RX frequency")); // TX-37
    }
    let tx = c.cfg.tx_of(c.rx) as i64;
    if !(mhz(64) as i64..mhz(1000) as i64).contains(&tx) {
        return Err(msg!("offset: the TX frequency would be outside 64 to 1000 MHz"));
    }
    if band_of(c.cfg.tx_of(c.rx)) == Some(BAND_125M) && c.cfg.power != 0 { // the cap goes by the TX band
        return Err(msg!("power: fixed at 0.25 W on 1.25 m; use power=low"));
    }
    Ok(())
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

fn ch<S: ChannelStore>(app: &mut App, st: &S, t: &mut Toks) -> Result<Done, Msg> {
    let num = index(t.word("ch: which channel, e.g. ch 12")?, MAX_CHANNELS as u32, "ch")? as u16;
    let old = st.by_num(num).map(|x| x.0);
    let missing = || msg!("ch {}: no such channel; make it with ch {} \"Name\" 146.52", num as u32 + 1, num as u32 + 1);
    if t.clone().next()?.is_some() && super::list::radio_channel(app) == Some(num) {
        return Err(msg!("Channel {} is open in the radio's editor: change it there or close it", num as u32 + 1)); // TERM-17
    }
    match t.next()? {
        None => old.map(|_| Done::List(List::Show(num))).ok_or_else(missing),
        // A whole channel: name, RX, then fields; the rest at their defaults.
        Some(Tok::Quoted(q)) => {
            let name: Buf<16> = unquote(q, "name")?;
            let rx = freq(t.word("ch: the RX frequency after the name, e.g. ch 12 \"Name\" 146.52")?, "rx")?;
            let mut c = Channel { num, zone: NO_ZONE, rx, name: [0; 16], cfg: DEFAULT_CFG, flags: 0 };
            c.set_name(name.as_bytes());
            while let Some(tok) = t.next()? {
                let Tok::Word(w) = tok else { return Err(msg!("ch: a name in quotes goes right after the number")) };
                match w.iter().position(|&b| b == b'=') {
                    Some(e) => field(&mut c, &w[..e], Some(Tok::Word(&w[e + 1..])), st)?,
                    None => field(&mut c, w, None, st)?,
                }
            }
            check(&c)?;
            Ok(Done::Said(save(app, st, c)))
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
            if band_of(c.rx) == Some(BAND_125M) && same(key, b"power") {
                return Err(msg!("power: fixed at 0.25 W on 1.25 m"));
            }
            field(&mut c, key, v, st)?;
            if t.next()?.is_some() {
                return Err(msg!("ch: one field at a time, e.g. ch 12 power high"));
            }
            check(&c)?;
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

fn secs(w: &[u8], max: u32, what: &str) -> Result<u8, Msg> {
    match number(w.strip_suffix(b"s").unwrap_or(w)) {
        Some(n) if (1..=max).contains(&n) => Ok(n as u8),
        _ => Err(msg!("{}: radio, or 1s to {}s", what, max)),
    }
}

fn zone_line<S: ChannelStore>(z: u8, st: &S, out: &mut Line) {
    let c = st.data().zone_cfg(z);
    num(out, b"zone ", z as u32 + 1);
    out.push(b" ");
    quoted(st.zone_label(z, b"", b"").as_bytes(), out);
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
    out.push(b" smart=");
    out.push(o(c.smart).as_bytes());
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
        return Err(msg!("{} {}: the radio couldn't take it now (full, or busy writing); try again", what, n as u32 + 1));
    }
    Ok(Done::Said(msg!("{} {} saved", cap, n as u32 + 1)))
}

const ZONE_KEYS: [&str; 8] = ["scan", "shown", "distance", "smart", "resume", "dwell", "pri1", "pri2"];

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
        let i = i.ok_or_else(|| msg!("zone: no field {}; they are scan, shown, distance, resume, dwell, smart, pri1, pri2", Bytes(k)))?;
        match i {
            0..=3 => *[&mut c.scan_all, &mut c.shown, &mut c.by_distance, &mut c.smart][i] = on_off(v, ZONE_KEYS[i])?,
            4 => {
                c.resume = match v {
                    _ if same(v, b"radio") => 0,
                    _ if same(v, b"carrier") => 11,
                    _ if same(v, b"stop") => 12,
                    _ => secs(v, 10, "resume").map_err(|_| msg!("resume: radio, 1s to 10s, carrier or stop"))?,
                }
            }
            5 => c.dwell = if same(v, b"radio") { 255 } else { secs(v, 254, "dwell")? },
            _ => c.pri[i - 6] = if same(v, b"none") { None } else { Some(index(v, MAX_CHANNELS as u32, "pri")? as u16) },
        }
    }
    let stored = z < have && st.zone_name(z) == Some(name) && st.data().zone_cfg(z) == c;
    put(st, "Zone", "zone", z, stored, Op::PutZone(z, name, c))
}

fn preset_line(i: u8, p: &RangePreset, out: &mut Line) {
    num(out, b"preset ", i as u32 + 1);
    out.push(b" ");
    quoted(p.name.as_bytes(), out);
    out.push(b" ");
    out.push(mhz5(p.start).as_bytes());
    out.push(b" ");
    out.push(mhz5(p.end).as_bytes());
    out.push(b" step=");
    step_word(p.step, out);
    out.push(b" mode=");
    out.push(MODES[p.mode as usize % MODES.len()].as_bytes());
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
    let mut p = RangePreset { name, start, end, step: STEP_12K5, mode: 1, narrow: true, skips: 0 };
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
        } else {
            return Err(msg!("preset: no field {}; they are step, mode, bw", Bytes(k)));
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
        p[2] = digit(t, groups::items(p[0], p[1]).len())?;
        return Ok((p, 3));
    }
    Ok((p, 2))
}

/// The item's name ("Theme").
fn path_name(p: &[u8]) -> &'static str {
    match p {
        [g, x] => groups::items(*g, 0)[*x as usize - 1].0,
        [g, s, x] => groups::items(*g, *s)[*x as usize - 1].0,
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
    match groups::wired(app, dev, p, b) {
        Some(d) => d,
        None => {
            let e = match p {
                [g, x] => groups::items(*g, 0)[*x as usize - 1],
                [g, s, x] => groups::items(*g, *s)[*x as usize - 1],
                _ => return false,
            };
            b.set(e.1.as_bytes());
            false
        }
    }
}

fn set(app: &mut App, dev: &Device, t: &mut Toks) -> Result<Done, Msg> {
    let (p, n) = path(t)?;
    let p = &p[..n];
    let name = path_name(p);
    if blocked(p) {
        return Err(msg!("{} changes only on the radio, on its own screen there", name));
    }
    if super::list::radio_setting(app).is_some_and(|(q, ql)| &q[..ql] == p) {
        return Err(msg!("{} is open on the radio's menu: change it there, or move off it", name)); // TERM-17
    }
    if let Some(why) = groups::greyed(app, p) {
        return Err(msg!("{}: {}", name, why));
    }
    if !SETTABLE.contains(&p) {
        return Err(msg!("{} can't be set yet: it keeps its default until its feature lands", name));
    }
    let want = t.rest();
    if want.is_empty() {
        return Err(msg!("set needs a value after the number, e.g. set 0 7 1 Dark"));
    }
    // Step through the values as the knob does until one reads as asked; a full turn brings it back.
    let mut v = Buf::<24>::new();
    value(app, dev, p, &mut v);
    let start = v;
    let mut all = msg!("{}: one of ", name);
    for k in 0..64 {
        if same(v.as_bytes(), want) {
            let mut m = Msg::new();
            path_text(p, &mut m);
            put!(m, " {}: {}", name, Bytes(v.as_bytes()));
            return Ok(Done::Said(m));
        }
        if k > 0 {
            all.push(b", ");
        }
        all.push(v.as_bytes());
        groups::change(app, p, 1);
        value(app, dev, p, &mut v);
        if v == start {
            break;
        }
    }
    Err(all)
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
    out.push(v.as_bytes());
    while out.len() < 32 {
        out.push(b" ");
    }
    out.push(b"  # ");
    out.push(path_name(p).as_bytes());
}

// ---- Running a line ----

/// Runs one line: typed after `:`, or read by `load` (`loading`: only the lines a dump holds).
pub fn run<S: ChannelStore>(app: &mut App, st: &S, dev: &Device, line: &[u8], loading: bool) -> Result<Done, Msg> {
    let mut t = Toks::new(line);
    let c = match t.next()? {
        None => return Ok(Done::Nothing),
        Some(Tok::Word(c)) => c,
        Some(Tok::Quoted(_)) => return Err(msg!("a command first, e.g. set, ch, zone; help lists them")),
    };
    let only_typed = |what: &str| -> Result<Done, Msg> { Err(msg!("{} can't be in a load: it reads set, zone, preset, ch and home lines", what)) };
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
        _ if same(c, b"help") || c == b"?" => Ok(Done::List(List::Help)),
        _ if same(c, b"dump") => Ok(Done::Dump),
        _ if same(c, b"mem") || same(c, b"mem32") => mem(&mut t, same(c, b"mem32")),
        _ if same(c, b"gpio") => Ok(Done::Debug(Job::new(Kind::Gpio))),
        _ if same(c, b"bk") => bk(&mut t),
        _ if same(c, b"si") => si(&mut t),
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

// ---- Debug (TERM-21 to TERM-24) ----

use super::debug::{Job, Kind, BK_REGS, MEM_MAX};

/// Hex, with or without 0x, up to 8 digits.
fn hexnum(w: &[u8]) -> Option<u32> {
    let w = w.strip_prefix(b"0x").or_else(|| w.strip_prefix(b"0X")).unwrap_or(w);
    if w.is_empty() || w.len() > 8 {
        return None;
    }
    w.iter().try_fold(0u32, |n, &c| Some(n << 4 | (c as char).to_digit(16)?))
}

fn hex_word(t: &mut Toks, what: &str) -> Result<u32, Msg> {
    hexnum(t.word(what)?).ok_or_else(|| msg!("{}", what))
}

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
    Ok(Done::Debug(Job::new(Kind::Mem { addr, len: len as u16, words })))
}

fn bk(t: &mut Toks) -> Result<Done, Msg> {
    const USAGE: &str = "bk CHIP REG [VALUE], bk CHIP dump: chip 0 (VHF) or 1 (UHF and HF), hex register and value";
    let chip = match t.word(USAGE)? {
        b"0" => 0,
        b"1" => 1,
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
    slow!("# One command per line, as typed after : in the terminal; :load reads it back. # starts a note."),
    slow!("# Channels and zones are merged by number: lines replace what they name, nothing else is deleted."),
    slow!("# Settings, by their numbers in the radio's menu"),
];

/// The dump's next line into `out`; false once it is over (TERM-13: streamed, never held whole).
pub fn dump_line<S: ChannelStore>(app: &App, st: &S, dev: &Device, p: &mut DumpPos, out: &mut Line) -> bool {
    out.clear();
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
            1 if (i as usize) < SETTABLE.len() => {
                let path = SETTABLE[i as usize];
                if groups::greyed(app, path).is_none() {
                    set_line(app, dev, path, out);
                    return true;
                }
            }
            1 if i as usize == SETTABLE.len() => {
                put!(out, "# 0 9 4 Region: {} (changes only on the radio)", app.set.region_name());
                return true;
            }
            1 if i as usize == SETTABLE.len() + 1 => {
                if let Some(c) = app.set.boot_callsign() {
                    put!(out, "# 0 8 7 Callsign: {} (set on the radio)", Bytes(c));
                    return true;
                }
            }
            2 if i == 0 => {
                out.push(slow!("# Zones (a channel's zone=N is one of these)").as_bytes());
                return true;
            }
            2 if i <= st.zones() as u16 => {
                zone_line(i as u8 - 1, st, out);
                return true;
            }
            3 if i == 0 => {
                out.push(slow!("# Range presets (0 1 7)").as_bytes());
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
pub fn list_line<S: ChannelStore>(app: &App, st: &S, dev: &Device, l: List, i: u16, out: &mut Line) -> bool {
    out.clear();
    match l {
        List::Help => match HELP.get(i as usize) {
            Some(h) => {
                out.push(h.as_bytes());
                true
            }
            None => false,
        },
        List::Changed => {
            let changed = SETTABLE.iter().filter(|p| {
                let mut v = Buf::<24>::new();
                value(app, dev, p, &mut v)
            });
            match changed.copied().nth(i as usize) {
                Some(p) => {
                    set_line(app, dev, p, out);
                    true
                }
                None if i == 0 => {
                    out.push(slow!("All settings are at their defaults.").as_bytes());
                    true
                }
                None => false,
            }
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

const HELP: [&str; 24] = [
    slow!("Commands: type : then one of these, and Enter. A dump is made of the same lines."),
    "",
    slow!("  set 0 7 1 Dark         a setting, by its number in the radio's menu"),
    slow!("  get 0 7 1   get        one setting; every setting not at its default"),
    slow!("  ch 12                  channel 12, as its ch line"),
    slow!("  ch 12 power high       one field: name rx offset tx mode bw tone rxtone txtone"),
    slow!("                         power zone step squelch busy reverse skip aprs rxonly"),
    slow!("  ch 12 \"Name\" 146.52 offset=+0.6 tone=100.0 ...   a whole channel"),
    slow!("  ch 12 delete           delete channel 12"),
    slow!("  zone 2 \"Repeaters\"     a zone (made in order); fields scan shown resume ..."),
    slow!("  preset 1 \"2 m\" 144 148 a range preset; fields step mode bw"),
    slow!("  home 12   home none    the home channel"),
    slow!("  dump                   everything, as these lines; save it with your terminal's log"),
    slow!("  load                   read a dump back: send the file, it ends at the line end"),
    slow!("  clear channels N       delete every channel (N: how many there are)"),
    "",
    slow!("Debug (hex addresses, registers and values; Esc stops):"),
    slow!("  mem 20000000 64        bytes of MCU memory (flash, RAM)"),
    slow!("  mem32 40010808 4       32-bit words: use this for peripheral registers"),
    slow!("  gpio                   ports A to E: input and output bits, 15 to 0"),
    slow!("  bk 0 30   bk 0 dump    a BK4829 register, or all of them (0 VHF, 1 UHF/HF)"),
    slow!("  bk 1 47 6142           write one, then read it back (no TX/PA bits, no GPIO)"),
    slow!("  si 10 00   si prop 4001 [0003]   an SI4732 command; get or set a property"),
    slow!("  The radio's own logic may rewrite a register you set (a tune, Battery Save)."),
];
