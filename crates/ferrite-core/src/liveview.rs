//! Live View (manual chapter 29, LINK-47, LINK-48; Appendix G "Live View"): the UI readback (LINK-19's text,
//! `ferrite-app` `uistate.rs`) made small enough for Bluetooth's ~280 bytes a second (LINK-36). The whole compact
//! state once, then only what changed since the last one the session was sent, token by token, each update
//! sequence-numbered and checked so a lost one is noticed and the whole state asked for again.
//!
//! The compact form is the readback itself, line for line, less what moves on every redraw (`state`'s `frame=` and
//! `ago=`), each line cut after the token that would take its encoding past [`LINE_ROOM`]. A line is split into
//! tokens at the spaces outside double quotes (a backslash escapes the next byte inside them), so joining its tokens
//! with one space gives the line back. On the wire, text is coded with [`DICT`]: a byte 0x01-0x1F stands for its
//! entry, every other byte for itself (the readback has no control bytes: they read as `?`).
//!
//! A UI event's payload (after the event kind) is `seq`, `flags` (1 from nothing: drop every line first; 2 more
//! follows, the check not valid yet), `check` (u16: FNV-1a 32 over every line, each followed by `\n`, the low 16
//! bits), `lines` (the line count after), then ops, each led by a byte whose top two bits are the op and the low six
//! the line:
//!
//! - `0` LINE: length (u8), the line, coded.
//! - `1` TOKENS: the line's token count after (u8), then entries (u8 count): token index (bit 7: the value only, the
//!   key up to its first `=` is the one there), length (u8), the token or value, coded.
//! - `2` TOKENS with the token count unchanged: as `1` without the count byte.
//!
//! [`Track`] remembers what the session has (the lines' text, 1 KB; a line it has no room for by its hash, then
//! sent whole when it changes), so the delta is exact. Pure logic: the radio and the host tests run the same code.

/// The most lines the compact form carries (the readback's first ones).
pub const LINES: usize = 48;
/// The text kept of what the session has; lines past it are kept by hash and sent whole when they change.
pub const TEXT: usize = 1024;
/// A coded line's most bytes: a LINE op always fits an otherwise empty event ([`HEAD`] + 2 + this <= 240).
pub const LINE_ROOM: usize = 230;
/// The UI event's header after the kind byte: seq, flags, check (2), lines.
pub const HEAD: usize = 5;
/// [`DICT`]'s version, in `SUBSCRIBE`'s answer: Console refuses a stream coded with one it doesn't know.
pub const DICT_VERSION: u8 = 1;

/// The flags byte.
pub const FULL: u8 = 1;
pub const MORE: u8 = 2;
/// The ops.
pub const OP_LINE: u8 = 0;
pub const OP_TOKENS: u8 = 1;
/// TOKENS with the line's token count unchanged: no count byte.
pub const OP_SAME: u8 = 2;
/// A token entry's index bit: the value only.
pub const VALUE: u8 = 0x80;

/// The text dictionary: byte `k` (1-31) codes entry `k - 1`. Chosen from the readback's most common pieces (home's
/// cards, a list's rows, the hints); never reordered within a [`DICT_VERSION`].
pub const DICT: [&[u8]; 31] = [
    b"row ",
    b"\" \"",
    b" sel",
    b" dot",
    b" grey",
    b"card ",
    b" active=",
    b" head=\"",
    b"\" state=\"",
    b" freq=",
    b" name=\"",
    b"\" mode=\"",
    b"\" tone=\"",
    b"\" shift=\"",
    b"\" power=\"",
    b"\" s=\"S",
    b"\" meter=",
    b" rx=",
    b" scan=",
    b" kind=slot",
    b"hints \"",
    b"screen ",
    b"\" title=\"",
    b"bar lock=",
    b" watch=",
    b" inbox=",
    b" battery=",
    b" clock=",
    b"state dark=",
    b" keyed=",
    b"qmenu \"",
];

pub const FNV0: u32 = 0x811C_9DC5;

/// FNV-1a 32 of `b`, from `h`.
pub fn fnv(mut h: u32, b: &[u8]) -> u32 {
    for &c in b {
        h = (h ^ c as u32).wrapping_mul(0x0100_0193);
    }
    h
}

/// `t` coded with [`DICT`] (the longest entry that matches, else the byte) into `out`: the length, or None if it
/// doesn't fit.
pub fn encode(t: &[u8], out: &mut [u8]) -> Option<usize> {
    let (mut i, mut o) = (0, 0);
    while i < t.len() {
        let mut best: Option<(usize, usize)> = None;
        for (k, e) in DICT.iter().enumerate() {
            if e.len() > best.map_or(1, |b| b.1) && t[i..].starts_with(e) {
                best = Some((k, e.len()));
            }
        }
        let (c, n) = match best {
            Some((k, n)) => (k as u8 + 1, n),
            None => (if t[i] < 0x20 { b'?' } else { t[i] }, 1),
        };
        *out.get_mut(o)? = c;
        (i, o) = (i + n, o + 1);
    }
    Some(o)
}

/// [`encode`]'s length.
pub fn encoded_len(t: &[u8]) -> usize {
    let mut b = [0u8; 256];
    encode(&t[..t.len().min(256)], &mut b).unwrap_or(256)
}

/// The tokens of a line: split at the spaces outside double quotes.
pub fn tokens(line: &[u8]) -> Tokens<'_> {
    Tokens { b: line, i: 0, done: false }
}

pub struct Tokens<'a> {
    b: &'a [u8],
    i: usize,
    done: bool,
}

impl<'a> Iterator for Tokens<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<&'a [u8]> {
        if self.done {
            return None;
        }
        let (start, mut quoted, mut esc) = (self.i, false, false);
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if esc {
                esc = false;
            } else if quoted && c == b'\\' {
                esc = true;
            } else if c == b'"' {
                quoted = !quoted;
            } else if c == b' ' && !quoted {
                let t = &self.b[start..self.i];
                self.i += 1;
                return Some(t);
            }
            self.i += 1;
        }
        self.done = true;
        Some(&self.b[start..])
    }
}

/// A token's key, up to and with its first `=` outside quotes (`meter=`); None for a bare token or a quoted one.
pub fn key(t: &[u8]) -> Option<&[u8]> {
    let i = t.iter().position(|&c| c == b'=' || c == b'"')?;
    (t[i] == b'=' && i > 0).then(|| &t[..=i])
}

/// The compact form of readback line `raw` into `out` (LINK-47): `state`'s `frame=` and `ago=` left out, tokens
/// past [`LINE_ROOM`] coded bytes left out. Its length.
pub fn compact(raw: &[u8], out: &mut [u8; 256]) -> usize {
    let state = raw.starts_with(b"state ") || raw == b"state";
    let (mut n, mut coded) = (0usize, 0usize);
    for (k, t) in tokens(raw).enumerate() {
        if state && (t.starts_with(b"frame=") || t.starts_with(b"ago=")) {
            continue;
        }
        let sep = (k > 0 && n > 0) as usize;
        let c = encoded_len(t) + sep;
        if coded + c > LINE_ROOM || n + sep + t.len() > out.len() {
            break;
        }
        if sep == 1 {
            out[n] = b' ';
        }
        out[n + sep..n + sep + t.len()].copy_from_slice(t);
        (n, coded) = (n + sep + t.len(), coded + c);
    }
    n
}

/// One line the session has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rec {
    hash: u32,
    /// Its text at `off` in [`Track::text`], `len` bytes; [`NOT_KEPT`] when there was no room (the hash alone).
    off: u16,
    len: u16,
}

const NOT_KEPT: u16 = 0xFFFF;
const EMPTY: Rec = Rec { hash: FNV0, off: 0, len: 0 };

/// What a session's Live View has been sent (LINK-47, LINK-48): the lines' text as the PC has them.
pub struct Track {
    text: [u8; TEXT],
    used: u16,
    rec: [Rec; LINES],
    n: u8,
    /// The last UI event's sequence number.
    seq: u8,
    /// The next event starts from nothing (a subscription, or the PC asked again).
    full: bool,
}

impl Default for Track {
    fn default() -> Self {
        Self::new()
    }
}

impl Track {
    pub const fn new() -> Self {
        Track { text: [0; TEXT], used: 0, rec: [EMPTY; LINES], n: 0, seq: 0, full: true }
    }

    /// The whole state goes again with the next event.
    pub fn restart(&mut self) {
        (self.n, self.used, self.full, self.rec) = (0, 0, true, [EMPTY; LINES]);
    }

    /// The sequence number the next event will carry.
    pub fn next_seq(&self) -> u8 {
        self.seq.wrapping_add(1)
    }

    fn kept(&self, i: usize) -> Option<&[u8]> {
        let r = self.rec[i];
        (r.len != NOT_KEPT).then(|| &self.text[r.off as usize..(r.off + r.len) as usize])
    }

    /// Line `i`'s text replaced by `t` (kept if there is room, else by hash alone).
    fn set(&mut self, i: usize, t: &[u8]) {
        let r = self.rec[i];
        let old = if r.len == NOT_KEPT { 0 } else { r.len as usize };
        let (off, end) = if r.len == NOT_KEPT { (self.used as usize, self.used as usize) } else { (r.off as usize, r.off as usize + old) };
        let fits = self.used as usize - old + t.len() <= TEXT;
        let new = if fits { t.len() } else { 0 };
        // the texts after this one move up or down
        let used = self.used as usize;
        if new != old {
            self.text.copy_within(end..used, off + new);
            for (k, r) in self.rec.iter_mut().enumerate() {
                if k != i && r.len != NOT_KEPT && r.off as usize >= end && end < used {
                    r.off = (r.off as usize + new - old) as u16;
                }
            }
        }
        self.text[off..off + new].copy_from_slice(&t[..new]);
        self.used = (used + new - old) as u16;
        self.rec[i] = Rec { hash: fnv(FNV0, t), off: off as u16, len: if fits { t.len() as u16 } else { NOT_KEPT } };
    }

    /// Line `i` forgotten (its text's room freed).
    fn drop_line(&mut self, i: usize) {
        self.set(i, b"");
        self.rec[i] = EMPTY;
    }

    /// The next UI event's payload (after its kind byte) into `out` (at most `out.len()` bytes): the lines that
    /// changed since the last one, as many whole line updates as fit, from `line(i, buf)` (readback line `i`'s raw
    /// text into `buf`, its length; None past the last). None when nothing changed (or nothing fits).
    pub fn update(&mut self, mut line: impl FnMut(u16, &mut [u8; 256]) -> Option<usize>, out: &mut [u8]) -> Option<usize> {
        if out.len() < HEAD + 2 + LINE_ROOM {
            return None;
        }
        let full = self.full;
        if full {
            self.restart();
        }
        let (mut o, mut ops, mut complete, mut check, mut n) = (HEAD, 0usize, true, FNV0, 0usize);
        let mut raw = [0u8; 256];
        let mut c = [0u8; 256];
        let mut op = [0u8; 256];
        while n < LINES {
            let Some(k) = line(n as u16, &mut raw) else { break };
            let cl = compact(&raw[..k.min(256)], &mut c);
            let t = &c[..cl];
            let i = n;
            n += 1;
            check = fnv(fnv(check, t), b"\n");
            let had = i < self.n as usize;
            if !complete || had && self.rec[i].hash == fnv(FNV0, t) {
                continue;
            }
            let m = match self.kept(i).filter(|_| had) {
                Some(old) => tokens_op(i as u8, old, t, &mut op).filter(|&m| m < 2 + encoded_len(t)),
                None => None,
            };
            let m = match m {
                Some(m) => m,
                None => {
                    op[0] = OP_LINE << 6 | i as u8;
                    let e = encode(t, &mut op[2..]).unwrap_or(0);
                    op[1] = e as u8;
                    2 + e
                }
            };
            if o + m > out.len() {
                complete = false;
                continue;
            }
            out[o..o + m].copy_from_slice(&op[..m]);
            (o, ops) = (o + m, ops + 1);
            if !had {
                self.rec[i] = EMPTY;
            }
            self.set(i, t);
        }
        // lines the PC drops (past the new count); new ones not sent yet are empty there, as `rec` has them
        for i in n..self.n as usize {
            self.drop_line(i);
        }
        // (the first op always fits: `out` has room for a whole line)
        if ops == 0 && n == self.n as usize && !full {
            return None;
        }
        self.n = n as u8;
        self.seq = self.seq.wrapping_add(1);
        self.full = false;
        let flags = if full { FULL } else { 0 } | if complete { 0 } else { MORE };
        let check = if complete { check as u16 } else { 0 };
        out[0] = self.seq;
        out[1] = flags;
        out[2..4].copy_from_slice(&check.to_le_bytes());
        out[4] = n as u8;
        Some(o)
    }
}

/// A TOKENS op for line `i` from `old` to `new` into `op`: its length, or None if it doesn't fit 256 bytes.
fn tokens_op(i: u8, old: &[u8], new: &[u8], op: &mut [u8; 256]) -> Option<usize> {
    op[0] = OP_TOKENS << 6 | i;
    let mut o = 3;
    let mut olds = tokens(old);
    let (mut count, mut entries) = (0usize, 0u8);
    for (k, t) in tokens(new).enumerate() {
        count += 1;
        let was = olds.next();
        if was == Some(t) {
            continue;
        }
        if k > 0x7F {
            return None;
        }
        let (idx, body) = match (was.and_then(key), key(t)) {
            (Some(a), Some(b)) if a == b => (k as u8 | VALUE, &t[b.len()..]),
            _ => (k as u8, t),
        };
        *op.get_mut(o)? = idx;
        let e = encode(body, op.get_mut(o + 2..)?)?;
        op[o + 1] = e as u8;
        o += 2 + e;
        entries += 1;
    }
    if count > 128 {
        return None; // more than a client splices (a line of 255 bytes has fewer)
    }
    op[1] = count as u8;
    op[2] = entries;
    if tokens(old).count() == count {
        op[0] = OP_SAME << 6 | i;
        op.copy_within(2..o, 1);
        return Some(o - 1);
    }
    Some(o)
}

/// What a Live View client keeps: the lines as text (Console's own, a `Vec` of lines in the tests).
pub trait Lines {
    fn count(&self) -> usize;
    /// Lines dropped or added (empty) to `n`.
    fn set_count(&mut self, n: usize);
    fn line(&self, i: usize) -> &[u8];
    fn set_line(&mut self, i: usize, t: &[u8]);
}

/// Why a client can't apply an event: it asks for the whole state again (`SUBSCRIBE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resync {
    /// Not the next sequence number: one was lost or came late.
    Seq,
    /// The event didn't parse.
    Bad,
    /// Applied, but the lines don't add up to the check.
    Check,
}

/// [`DICT`]-coded `b` decoded into `out`: its length (cut at `out`'s).
pub fn decode(b: &[u8], out: &mut [u8]) -> usize {
    let mut o = 0;
    for &c in b {
        let e: &[u8] = match c {
            1..=31 => DICT[c as usize - 1],
            _ => core::slice::from_ref(&c),
        };
        for &x in e {
            if o < out.len() {
                out[o] = x;
                o += 1;
            }
        }
    }
    o
}

/// The check over a client's lines (LINK-48).
pub fn check(l: &impl Lines) -> u16 {
    let mut h = FNV0;
    for i in 0..l.count() {
        h = fnv(fnv(h, l.line(i)), b"\n");
    }
    h as u16
}

/// A client applies a UI event's payload (after the kind byte), `last` being the last sequence number it applied
/// (None: none yet, only a full one will do). The event's sequence number, or why the whole state must be asked
/// for again. The reference for Console (`tools/ferrite_link.py` has the same in Python).
pub fn apply(l: &mut impl Lines, ev: &[u8], last: Option<u8>) -> Result<u8, Resync> {
    let [seq, flags, c0, c1, n, ref ops @ ..] = *ev else { return Err(Resync::Bad) };
    if flags & FULL != 0 {
        l.set_count(0);
    } else if last.map(|s| s.wrapping_add(1)) != Some(seq) {
        return Err(Resync::Seq);
    }
    l.set_count(n as usize);
    let mut i = 0;
    let mut buf = [0u8; 512];
    while i < ops.len() {
        let (op, ln) = (ops[i] >> 6, (ops[i] & 0x3F) as usize);
        if ln >= n as usize {
            return Err(Resync::Bad);
        }
        match op {
            OP_LINE => {
                let len = *ops.get(i + 1).ok_or(Resync::Bad)? as usize;
                let t = ops.get(i + 2..i + 2 + len).ok_or(Resync::Bad)?;
                let k = decode(t, &mut buf);
                l.set_line(ln, &buf[..k]);
                i += 2 + len;
            }
            OP_TOKENS | OP_SAME => {
                let (count, at) = match op {
                    OP_SAME => (tokens(l.line(ln)).count(), i + 1),
                    _ => (*ops.get(i + 1).ok_or(Resync::Bad)? as usize, i + 2),
                };
                let entries = *ops.get(at).ok_or(Resync::Bad)?;
                i = at + 1;
                // the line's tokens as they are, then each entry put in
                let mut toks: [(u16, u16); 128] = [(0, 0); 128];
                let mut store = [0u8; 2048];
                let mut used = 0usize;
                let old = l.line(ln);
                let mut have = 0;
                for t in tokens(old).take(count.min(128)) {
                    let e = (used + t.len()).min(store.len());
                    store[used..e].copy_from_slice(&t[..e - used]);
                    toks[have] = (used as u16, e as u16);
                    (used, have) = (e, have + 1);
                }
                for t in toks.iter_mut().take(count.min(128)).skip(have) {
                    *t = (used as u16, used as u16);
                }
                for _ in 0..entries {
                    let (idx, len) = (*ops.get(i).ok_or(Resync::Bad)?, *ops.get(i + 1).ok_or(Resync::Bad)? as usize);
                    let t = ops.get(i + 2..i + 2 + len).ok_or(Resync::Bad)?;
                    i += 2 + len;
                    let k = (idx & !VALUE) as usize;
                    if k >= count.min(128) {
                        return Err(Resync::Bad);
                    }
                    let mut v = [0u8; 256];
                    let vn = decode(t, &mut v);
                    let (a, b) = toks[k];
                    let keep = if idx & VALUE != 0 { key(&store[a as usize..b as usize]).map_or(0, |x| x.len()) } else { 0 };
                    // the new token goes at the end of `store`
                    let start = used;
                    let keep = keep.min(store.len() - start);
                    store.copy_within(a as usize..a as usize + keep, start);
                    let w = vn.min(store.len() - start - keep);
                    store[start + keep..start + keep + w].copy_from_slice(&v[..w]);
                    used = start + keep + w;
                    toks[k] = (start as u16, used as u16);
                }
                let mut o = 0;
                for (j, &(a, b)) in toks.iter().take(count.min(128)).enumerate() {
                    if j > 0 && o < buf.len() {
                        buf[o] = b' ';
                        o += 1;
                    }
                    let w = (b - a) as usize;
                    let w = w.min(buf.len() - o);
                    buf[o..o + w].copy_from_slice(&store[a as usize..a as usize + w]);
                    o += w;
                }
                l.set_line(ln, &buf[..o]);
            }
            _ => return Err(Resync::Bad),
        }
    }
    if flags & MORE == 0 && check(l) != u16::from_le_bytes([c0, c1]) {
        return Err(Resync::Check);
    }
    Ok(seq)
}

#[cfg(test)]
mod tests;
