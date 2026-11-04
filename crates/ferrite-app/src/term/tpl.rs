//! The full-screen pages as a program in the SPI flash, and the interpreter that runs it (TERM-11 to TERM-20,
//! TERM-47; decision 346). Everything a page shows and does is the program's: the text, layout, list rows,
//! hints, help, each key's action, the channel list's sorting, find, marks and changes to several channels. It is
//! written in a small stack language (`pages.vt`, described there), compiled by `build.rs` into the template pack,
//! an SPI part (UPD-21). The firmware keeps this interpreter and a few generic natives: a value (the terminal's
//! size, the store's counts, a device reading), a channel's field by number through the store, a setting by its
//! menu number through the menu's own tables (`groups`), the radio's own formatters (MHz, tones, a field's text), and
//! the writes the App already has (a channel saved, deleted, renumbered; a setting changed).
//!
//! The pack (format 2), read from the part through two small windows for each redraw, key or poll step (never kept):
//!
//! ```text
//! pack     "FVT" 2 | u8 segments | u8 0 | u16 lib length | (u16 offset, u16 length)[segments] | lib | segment...
//! lib      JMP frame | JMP key | JMP prep | JMP poll | words...      (code at address 0)
//! segment  JMP draw | JMP key | JMP hints | JMP help | JMP prep | JMP poll | words... | 0 0   (after the lib)
//! ```
//!
//! The segments are the pages in `pages.vt`'s order (`term::Page`'s, then any a page chains to: the channel
//! list's tools), then each page's help (`NPAGES` on: read in place of the page while its help shows). A run sees the
//! lib and one segment as one code space, read through a small window each ([`Code`]: a miss reads the next 128
//! bytes from the part): a segment's code is placed at the lib's length, so it calls the lib's words directly and the lib calls the segment's through its table (PCALL k). A run that sets
//! `chain` is followed by the same entry in that segment. Numbers are little-endian. Code is bytes: an opcode,
//! then its inline argument (a string's length, then its bytes); a read past the code is
//! zeros, RET.
//!
//! ```text
//! 00 RET  01 LIT8 i8  02 LIT16 i16  03 JMP a16  04 JZ a16 (pops)  05 CALL a16
//! 06 DUP  07 DROP  08 SWAP  09 OVER  0A + 0B - 0C * 0D / 0E mod 0F = 10 < 11 > 12 and 13 or (bitwise) 14 not
//! 15 min  16 max  17 GET v  18 SET v  19 AGET (i -- x)  1A ASET (x i --): the sorted page's numbers
//! 1B STR n bytes (print: columns counted, clipped at the width)  1C RAW n bytes (an escape sequence)
//! 1D AT (col --: spaces up to it)  1E NUM (n --)  1F NUMW c (n width --, padded with c)  20 PCALL k
//! 21 NAT id io (io: what the native takes from the stack, low nibble, and gives back, high)
//! ```
//!
//! A run starts with one number on the stack (the row, or the key) and ends at its last RET. Nothing in a bad pack
//! can panic or hang the radio: the stacks wrap, a read past the code is RET, and a run stops after `FUEL` steps.

use super::{blocked, cmd, cmd_trim, digits, list_len, mhz5, radio_channel, radio_setting, version, zone_name, Cx, Term, W, ALL, FIELDS};
use crate::chan::{Channel, ChannelStore, ZoneId, MAX_CHANNELS, NO_ZONE};
use crate::chfield::{any_room, check_edit, Ctx, Op};
use crate::screens::advanced::GROUPS;
use crate::screens::groups::{self, SUBS};
use crate::slot::{mode_chip, tone_text, Item};
use core::cmp::Ordering;
use ferrite_core::freq::{mhz, Freq10};
use ferrite_core::gpspower::GpsPhase;
use ferrite_core::msg::SLOTS;
use ferrite_ui::text::Buf;
use ferrite_ui::widgets as w;

#[allow(dead_code)]
mod pack {
    include!(concat!(env!("OUT_DIR"), "/vt_pack.rs"));
}
pub(super) use pack::*;

/// The code is read through a small cache: `LINES` lines of `LINE` bytes, the least recently used refilled on a
/// miss. A run's buffer is these, never the whole page.
const LINE: usize = 32;
pub(super) const LINES: usize = 32;
/// Steps a run may take (a sorted page's fill or a mark-all is a few tens of thousands).
const FUEL: u32 = 2_000_000;
/// Rows a sorted page holds at most: the tallest terminal less the list's other rows.
pub(super) const VIS: usize = 56;
/// Bytes of marks: one bit per channel number (TERM-14).
pub(super) const MARK_BYTES: usize = MAX_CHANNELS as usize / 8;

// The program's names for the firmware's values.
const _: () = assert!(C_ALL == ALL as i16 as i32 && C_NOZONE == NO_ZONE as i16 as i32 && NL <= 8 && C_MAXCH == MAX_CHANNELS as i32 && C_VIS == VIS as i32);
const _: () = {
    use crate::chan::*;
    assert!(C_FLAG_SKIP == FLAG_SKIP as i32 && C_FLAG_APRS == FLAG_APRS as i32 && C_FLAG_RX_ONLY == FLAG_RX_ONLY as i32);
    assert!(C_FLAG_HIDDEN == FLAG_HIDDEN as i32 && C_FLAG_PRIORITY == FLAG_PRIORITY as i32);
};

/// What C holds with no channel.
const BLANK: Channel = Channel { num: 0, zone: NO_ZONE, rx: 0, name: [0; 16], cfg: crate::slot::DEFAULT_CFG, flags: 0, tones: 0 };

/// The pages' state: the program's variables (which page, selections, the change in progress), the sorted
/// page, the marks, the channel being edited (the terminal's own copy, written through the App on every change),
/// a value being typed, and the last thing found.
#[derive(Clone, Copy)]
pub(crate) struct Pages {
    pub(super) v: [i16; NV],
    pub(super) vis: [u16; VIS],
    pub(super) marks: [u8; MARK_BYTES],
    pub(super) edit: Option<Channel>,
    pub(super) line: Buf<24>,
    pub(super) typing: bool,
    pub(super) lmax: u8,
    pub(super) find: Buf<16>,
}

impl Pages {
    pub(super) const fn new() -> Self {
        Pages { v: [0; NV], vis: [0; VIS], marks: [0; MARK_BYTES], edit: None, line: Buf::new(), typing: false, lmax: 0, find: Buf::new() }
    }
}

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

fn read(off: usize, b: &mut [u8]) -> bool {
    #[cfg(not(target_os = "none"))]
    if !ferrite_core::part::has_reader() {
        return match PACK.get(off..off + b.len()) {
            Some(s) => {
                b.copy_from_slice(s);
                true
            }
            None => false,
        };
    }
    ferrite_core::part::read_body(&ferrite_core::part::TERM, off as u32, b)
}

fn u16_at(b: &[u8], i: usize) -> usize {
    b[i] as usize | (b[i + 1] as usize) << 8
}

/// The lib and one segment of the pack, read from the part through a small cache of lines (the least recently
/// used refilled on a miss), so a run needs no buffer the size of its code. Addresses are the code's: the lib's
/// from 0, the segment's from the lib's length. A read past the code, or one that fails, is zeros (RET).
pub(super) struct Code {
    /// The lib's length and its place in the part.
    lib: (usize, usize),
    /// The segment's place in the part and its length.
    seg: (usize, usize),
    /// Each line's address / LINE (`EMPTY`: none), when it was last used, and its bytes.
    tag: [u16; LINES],
    used: [u16; LINES],
    buf: [[u8; LINE]; LINES],
    /// The line used last (most reads are the next byte), and the clock `used` counts in.
    last: usize,
    clock: u16,
}

const EMPTY: u16 = u16::MAX;
/// Where the lib starts in the part: after the header and the segment table.
const LIB_AT: usize = 8;

// The tests' counts: lines read from the part, and the most lines one run used (its working set: the cache must
// hold it, or the run reads the same lines over and over).
#[cfg(test)]
struct Peak;
#[cfg(test)]
impl Drop for Peak {
    fn drop(&mut self) {
        let n = TOUCH.with(|x| x.borrow().iter().map(|w| w.count_ones()).sum::<u32>());
        PEAK.with(|p| p.set(p.get().max(n)));
    }
}
#[cfg(test)]
std::thread_local! { pub(super) static PEAK: core::cell::Cell<u32> = const { core::cell::Cell::new(0) }; }
#[cfg(test)]
std::thread_local! { static TOUCH: core::cell::RefCell<[u64; 32]> = const { core::cell::RefCell::new([0; 32]) }; }
#[cfg(test)]
std::thread_local! { pub(super) static FILLS: core::cell::Cell<u32> = const { core::cell::Cell::new(0) }; }

impl Code {
    /// The lib and segment `pg` of the part; None when the part isn't there or is bad.
    #[inline(never)]
    pub(super) fn open(pg: i16) -> Option<Code> {
        let mut h = [0u8; 8];
        if !read(0, &mut h) || h[..4] != *b"FVT\x02" || pg as u32 >= h[4] as u32 {
            return None;
        }
        let mut e = [0u8; 4];
        read(LIB_AT + 4 * pg as usize, &mut e).then(|| Code {
            lib: (u16_at(&h, 6), LIB_AT + 4 * h[4] as usize),
            seg: (u16_at(&e, 0), u16_at(&e, 2)),
            tag: [EMPTY; LINES],
            used: [0; LINES],
            buf: [[0; LINE]; LINES],
            last: 0,
            clock: 0,
        })
    }

    /// The bytes from address `a` to the end of its line (at least one).
    #[inline(never)]
    fn at(&mut self, a: usize) -> &[u8] {
        let t = (a / LINE).min(EMPTY as usize - 1) as u16;
        #[cfg(test)]
        TOUCH.with(|x| x.borrow_mut()[t as usize / 64 % 32] |= 1 << (t % 64));
        let mut i = self.last;
        if self.tag[i] != t {
            i = match self.tag.iter().position(|&x| x == t) {
                Some(i) => i,
                None => self.fill(t),
            };
            self.last = i;
            self.clock = self.clock.wrapping_add(1);
            self.used[i] = self.clock;
        }
        &self.buf[i][a % LINE..]
    }

    /// Line `t` read into the least recently used place: the lib's bytes, then the segment's.
    #[inline(never)]
    fn fill(&mut self, t: u16) -> usize {
        let i = (0..LINES).max_by_key(|&j| self.clock.wrapping_sub(self.used[j])).unwrap_or(0);
        #[cfg(test)]
        FILLS.with(|f| f.set(f.get() + 1));
        let ((lib, lib_at), (so, sl)) = (self.lib, self.seg);
        let a = t as usize * LINE;
        let b = &mut self.buf[i];
        b.fill(0);
        // the lib's part of the line, then the segment's (each clipped to its end)
        let l = lib.saturating_sub(a).min(LINE);
        let s0 = (a + l).saturating_sub(lib);
        let n = sl.saturating_sub(s0).min(LINE - l);
        if !(read(lib_at + a, &mut b[..l]) && read(so + s0, &mut b[l..l + n])) {
            b.fill(0);
        }
        self.tag[i] = t;
        i
    }

    fn byte(&mut self, a: usize) -> u8 {
        self.at(a)[0]
    }
}

/// The number stack: it wraps, so a bad program can't index past it.
struct Stk {
    s: [i32; 16],
    p: usize,
}

impl Stk {
    #[inline(never)]
    fn pop(&mut self) -> i32 {
        self.p = self.p.wrapping_sub(1);
        self.s[self.p & 15]
    }

    #[inline(never)]
    fn push(&mut self, v: i32) {
        self.s[self.p & 15] = v;
        self.p = self.p.wrapping_add(1);
    }

    fn flag(&mut self, b: bool) {
        self.push(b as i32);
    }

    fn u(&mut self) -> usize {
        self.pop() as usize
    }
}

/// A sort column's order for two channels (TERM-14): the column as one number (a name's 16 bytes in any case,
/// big-endian), then the channel number, so the order is total.
pub(super) fn order(col: i32, a: &Channel, b: &Channel) -> Ordering {
    let key = |c: &Channel| -> (u128, u16) {
        let k = match col {
            1 => {
                let mut n = [0u8; 16];
                for (d, s) in n.iter_mut().zip(c.name()) {
                    *d = s.to_ascii_lowercase();
                }
                u128::from_be_bytes(n)
            }
            2 => c.rx as u128,
            3 => c.cfg.tx_of(c.rx) as u128,
            4 => (c.cfg.mode as u128) << 1 | c.cfg.narrow as u128,
            5 => (c.cfg.tx_tone as u128) << 16 | c.cfg.rx_tone as u128,
            6 => c.cfg.power as u128,
            7 => c.flags as u128,
            _ => 0,
        };
        (k, c.num)
    };
    key(a).cmp(&key(b))
}

/// The channel after (or before) channel `from` in the sort order, in the list (`zone`, or every channel): one
/// pass over every channel. `from` -1 (or gone): the first (or last).
#[inline(never)]
fn seek<S: ChannelStore>(st: &S, from: i32, zone: i32, col: i32, after: bool) -> i32 {
    let a = if from < 0 { None } else { st.get(from as u16) };
    let want = if after { Ordering::Greater } else { Ordering::Less };
    let mut best: Option<Channel> = None;
    for n in 0..MAX_CHANNELS {
        let Some(c) = st.get(n).filter(|c| zone as ZoneId == ALL || c.zone == zone as ZoneId) else { continue };
        if a.as_ref().is_none_or(|a| order(col, &c, a) == want) && best.as_ref().is_none_or(|b| order(col, &c, b) == want.reverse()) {
            best = Some(c);
        }
    }
    best.map_or(-1, |c| c.num as i32)
}

/// A setting's path after MENU 0: item `row` of group `g` (or of its third-level list `sub`).
fn path(g: i32, sub: i32, row: i32) -> ([u8; 3], usize) {
    let (g, sub, r) = (g as u8, sub as u8, row as u8 + 1);
    if sub == 0 { ([g, r, 0], 2) } else { ([g, sub, r], 3) }
}

impl Term {
    pub(super) fn marked(&self, num: u16) -> bool {
        self.pg.marks.get(num as usize / 8).is_some_and(|b| b >> (num % 8) & 1 != 0)
    }

    pub(super) fn marks_n(&self) -> u16 {
        self.pg.marks.iter().map(|b| b.count_ones() as u16).sum()
    }

    /// Runs the code from `pc` with `arg` on the stack, until its last return.
    #[inline(never)]
    pub(super) fn vm<S: ChannelStore>(&mut self, code: &mut Code, mut pc: usize, arg: i32, cx: &mut Cx<S>, w: &mut W) {
        #[cfg(test)]
        TOUCH.with(|x| *x.borrow_mut() = [0; 32]);
        #[cfg(test)]
        let _g = Peak;
        let mut k = Stk { s: [0; 16], p: 0 };
        let (mut rs, mut rp) = ([0u16; 16], 0usize);
        let mut c = BLANK;
        let mut loc = [0i32; 8];
        k.push(arg);
        for _ in 0..FUEL {
            // an opcode and up to two bytes after it (a page's code ends with two zeros, so this reads no further)
            let [op, a, b] = [code.byte(pc), code.byte(pc + 1), code.byte(pc + 2)];
            let a16 = a as usize | (b as usize) << 8;
            pc += 1;
            match op {
                OP_RET => {
                    if rp == 0 {
                        return;
                    }
                    rp -= 1;
                    pc = rs[rp & 15] as usize;
                }
                OP_LIT8 => {
                    k.push(a as i8 as i32);
                    pc += 1;
                }
                OP_LIT16 => {
                    k.push(a16 as i16 as i32);
                    pc += 2;
                }
                OP_JMP => pc = a16,
                OP_JZ => pc = if k.pop() == 0 { a16 } else { pc + 2 },
                OP_CALL | OP_PCALL => {
                    rs[rp & 15] = (pc + if op == OP_CALL { 2 } else { 1 }) as u16;
                    rp += 1;
                    pc = if op == OP_CALL { a16 } else { code.lib.0 + 3 * a as usize };
                }
                OP_DUP | OP_DROP | OP_NOT | OP_AGET | OP_AT | OP_NUM => {
                    let x = k.pop();
                    match op {
                        OP_DUP => {
                            k.push(x);
                            k.push(x);
                        }
                        OP_NOT => k.flag(x == 0),
                        OP_AGET => k.push(self.pg.vis.get(x as usize).map_or(0, |&n| n as i32)),
                        OP_AT => w.at(x as u16),
                        OP_NUM => w.n(x as u32),
                        _ => {}
                    }
                }
                OP_SWAP..=OP_MAX | OP_ASET => {
                    let y = k.pop();
                    let x = k.pop();
                    let r = match op {
                        OP_SWAP => {
                            k.push(y);
                            x
                        }
                        OP_OVER => {
                            k.push(x);
                            k.push(y);
                            x
                        }
                        OP_ADD => x.wrapping_add(y),
                        OP_SUB => x.wrapping_sub(y),
                        OP_MUL => x.wrapping_mul(y),
                        OP_DIV => x.checked_div(y).unwrap_or(0),
                        OP_MOD => x.checked_rem(y).unwrap_or(0),
                        OP_EQ => (x == y) as i32,
                        OP_LT => (x < y) as i32,
                        OP_GT => (x > y) as i32,
                        OP_AND => x & y,
                        OP_OR => x | y,
                        OP_MIN => x.min(y),
                        OP_MAX => x.max(y),
                        _ => {
                            if let Some(v) = self.pg.vis.get_mut(y as usize) {
                                *v = x as u16;
                            }
                            continue;
                        }
                    };
                    k.push(r);
                }
                OP_GET => {
                    let a = a as usize;
                    k.push(if a < NV { self.pg.v[a] as i32 } else { loc[a & 7] });
                    pc += 1;
                }
                OP_SET => {
                    let (a, x) = (a as usize, k.pop());
                    if a < NV {
                        self.pg.v[a] = x as i16;
                    } else {
                        loc[a & 7] = x;
                    }
                    pc += 1;
                }
                OP_STR | OP_RAW => {
                    // text in pieces, a line at a time (the packer writes a middle dot as one byte, so no
                    // character's bytes are cut apart); an escape sequence whole
                    let (mut p, end) = (pc + 1, pc + 1 + a as usize);
                    let mut esc = Buf::<24>::new();
                    while p < end {
                        let s = code.at(p);
                        let s = &s[..s.len().min(end - p)];
                        p += s.len();
                        if op == OP_RAW {
                            esc.push(s);
                        } else {
                            w.s(s);
                        }
                    }
                    if op == OP_RAW {
                        w.put(esc.as_bytes());
                    }
                    pc = end;
                }
                OP_NUMW => {
                    let wd = k.u();
                    let mut d = [0; 10];
                    let s = digits(k.pop() as u32, &mut d);
                    for _ in s.len()..wd.min(16) {
                        w.s(&[a]);
                    }
                    w.s(s);
                    pc += 1;
                }
                OP_NAT => {
                    let io = b;
                    pc += 2;
                    let mut x = [0; 4];
                    for j in (0..(io as usize & 7).min(4)).rev() {
                        x[j] = k.pop();
                    }
                    let r = self.nat(a, x, &mut c, cx, w);
                    if io >> 4 != 0 {
                        k.push(r);
                    }
                }
                _ => return,
            }
        }
    }

    /// The natives: what the program asks of the firmware. `x` holds what it takes from the stack (the deepest
    /// first); what it gives back is returned (`NAT_IO` says whether there is any).
    #[inline(never)]
    fn nat<S: ChannelStore>(&mut self, id: u8, x: [i32; 4], c: &mut Channel, cx: &mut Cx<S>, w: &mut W) -> i32 {
        let st = cx.store;
        let dev = cx.dev;
        let mut v = Buf::<16>::new();
        let (a, z) = (x[0], x[0] as ZoneId);
        let mut ld = |o: Option<Channel>| {
            *c = o.unwrap_or(BLANK);
            o.is_some() as i32
        };
        let s: &[u8] = match id {
            N_ROWS => return self.rows as i32,
            N_COLS => return self.cols as i32,
            N_BT => return self.bt as i32,
            N_PIN => return self.pin_gate() as i32,
            N_PINN => return self.pin_n as i32,
            N_CMDON => return self.cmd as i32,
            N_BNOTE => return self.bnote as i32,
            N_WARN => return self.warn as i32,
            N_NOTEON => return !self.note.is_empty() as i32,
            N_TYPING => return self.pg.typing as i32,
            N_LNUM => {
                let s = cmd_trim(self.pg.line.as_bytes());
                let ok = !s.is_empty() && s.len() <= 4 && s.iter().all(u8::is_ascii_digit);
                return if ok { s.iter().fold(0, |n, &d| n * 10 + (d - b'0') as i32) } else { -1 };
            }
            N_HASCALL => return cx.app.set.boot_callsign().is_some() as i32,
            N_SSB => return dev.selftest.ssb_patch as i32,
            N_TXBUILT => return dev.tx_built as i32,
            N_CLOCK => return dev.clock_min.map_or(-1, |m| m as i32),
            N_BATTMV => return dev.battery_mv as i32,
            N_GPS => return dev.gps as i32,
            N_COLOUR => return cx.app.set.term_colour as i32,
            N_GPSPH => {
                return match dev.gps_phase {
                    GpsPhase::Standby(_) => 1,
                    GpsPhase::Waking => 2,
                    GpsPhase::Running | GpsPhase::Searching => 3,
                    _ => 0,
                }
            }
            N_GPSS => return if let GpsPhase::Standby(s) = dev.gps_phase { s as i32 } else { 0 },
            N_BTDEV => return dev.bt as i32,
            N_BSAVE => return cx.app.bsave as i32,
            N_COUNT => return st.count() as i32,
            N_ZONES => return st.zones() as i32,
            N_CEDIT => return ld(self.pg.edit),
            N_RADIOCH => return radio_channel(cx.app).map_or(-1, |n| n as i32),
            N_HOMECH => return cx.app.set.home_ch.map_or(-1, |n| n as i32),
            N_NMARKS => return self.marks_n() as i32,
            N_ACTIVE => return cx.app.active() as i32,
            N_NEV => return st.data().events() as i32,
            N_CMDTAIL => {
                // the command line's tail, when it is wider than the row
                let n = self.clen as usize;
                &self.cbuf[n.saturating_sub((self.cols as usize).saturating_sub(4))..n]
            }
            N_CMDOPEN => {
                (self.cmd, self.clen) = (true, 0);
                b""
            }
            N_NOTE => self.note.as_bytes(),
            N_NOTECLR => {
                self.note.clear();
                (self.warn, self.bnote) = (false, false);
                b""
            }
            N_CAP => {
                w.cap(true);
                b""
            }
            N_LNAME => {
                self.pg.line = Buf::from(c.name());
                b""
            }
            N_LSTOP => {
                self.pg.typing = false;
                b""
            }
            N_LINE => self.pg.line.as_bytes(),
            N_LFIND => {
                let s = cmd_trim(self.pg.line.as_bytes());
                if !s.is_empty() {
                    self.pg.find = Buf::from(&s[..s.len().min(16)]);
                }
                b""
            }
            N_FIND => self.pg.find.as_bytes(),
            N_VERSION => version(),
            N_CALLSIGN => cx.app.set.boot_callsign().unwrap_or(b"No callsign set"),
            N_REGION => cx.app.set.region_name().as_bytes(),
            N_CAL => groups::cal_text(dev.selftest.cal).as_bytes(),
            N_ESET => {
                self.pg.edit = Some(*c);
                b""
            }
            N_ECLR => {
                self.pg.edit = None;
                b""
            }
            N_ELOAD => {
                self.pg.edit = self.pg.edit.map(|e| st.get(e.num).unwrap_or(e));
                b""
            }
            N_CMODE | N_CTONE | N_CITEM | N_FTEXT => {
                match id {
                    N_CMODE => mode_chip(&c.cfg, &mut v),
                    N_CTONE => tone_text(c.cfg.tx_tone, &mut v),
                    N_CITEM => (if a == 0 { Item::Offset } else { Item::Power }).text(&c.cfg, false, &mut v),
                    _ => FIELDS[a as usize % FIELDS.len()].text(c, cx.app.set.home_ch, true, &mut v),
                }
                if v.is_empty() && id == N_CTONE { b"-" } else { v.as_bytes() }
            }
            N_CPUT => {
                cx.app.put_channel(*c);
                b""
            }
            N_UNMARK => {
                self.pg.marks = [0; MARK_BYTES];
                b""
            }
            N_SAY => {
                // 0 say, 1 a warning; 2 and 3 on the bottom row until the next key
                (self.note, self.warn, self.bnote) = (w.cap(false), a & 1 != 0, a & 2 != 0);
                self.all_dirty();
                b""
            }
            N_LSTART => {
                (self.pg.lmax, self.pg.line, self.pg.typing) = (a as u8, Buf::new(), true);
                b""
            }
            N_ZNAME => {
                w.s(zone_name(z, st).as_bytes());
                b""
            }
            N_MHZ => {
                w.s(mhz5(a as Freq10).as_bytes());
                b""
            }
            N_FREQ3 => {
                w.s(w::freq3(a as Freq10).as_bytes());
                b""
            }
            N_CDEL => {
                cx.app.delete_channel(a as u16, st);
                b""
            }
            N_CREN => {
                cx.app.renumber_channel(*c, a as u16);
                b""
            }
            N_FLABEL => FIELDS[a as usize % FIELDS.len()].label(true).as_bytes(),
            N_CNAME => {
                let n = c.name();
                &n[..n.len().min(a as usize)]
            }
            N_GNAME => GROUPS[a as usize % GROUPS.len()].0.as_bytes(),
            N_GDESC => {
                w.s(crate::text::load(GROUPS[a as usize % GROUPS.len()].1).as_bytes());
                b""
            }
            N_CAPCMP => {
                // 0: the typed line, any case; 1: holds the text found, any case; 2: starts with it
                let m = w.cap(false);
                let (m, t) = (m.as_bytes(), if a == 0 { cmd_trim(self.pg.line.as_bytes()) } else { self.pg.find.as_bytes() });
                return match a {
                    0 => m.eq_ignore_ascii_case(t),
                    1 => t.is_empty() || m.windows(t.len()).any(|x| x.eq_ignore_ascii_case(t)),
                    _ => m.starts_with(t),
                } as i32;
            }
            N_ZLEN => return list_len(z, st) as i32,
            N_CGET => return ld(st.get(a as u16)),
            N_CF => {
                return match a {
                    0 => c.num as i32,
                    1 => c.zone as i16 as i32,
                    2 => c.rx as i32,
                    3 => c.cfg.tx_of(c.rx) as i32,
                    4 => c.cfg.split as i32,
                    5 => c.cfg.offset,
                    _ => c.flags as i32,
                }
            }
            N_NEWCH => return self.new_channel(cx, z),
            N_MARK => return self.marked(a as u16) as i32,
            N_NEXTMARK => return (a.max(0) as u16..MAX_CHANNELS).find(|&n| self.marked(n)).map_or(-1, |n| n as i32),
            N_PREVMARK => return (0..a.clamp(0, MAX_CHANNELS as i32) as u16).rev().find(|&n| self.marked(n)).map_or(-1, |n| n as i32),
            N_MARKBELOW => return (0..a.clamp(0, MAX_CHANNELS as i32) as u16).filter(|&n| self.marked(n)).count() as i32,
            N_CSET => {
                if x[1] == 0 {
                    c.num = a as u16;
                } else {
                    c.zone = z;
                }
                b""
            }
            N_MARKSET => {
                let n = a as usize;
                if let Some(b) = self.pg.marks.get_mut(n / 8) {
                    *b = *b & !(1 << (n % 8)) | ((x[1] != 0) as u8) << (n % 8);
                }
                b""
            }
            N_EVAGO => return st.data().event(a as u16).map_or(0, |e| e.ago_s as i32),
            N_EV => {
                let Some(e) = st.data().event(a as u16) else { return 0 };
                match x[1] {
                    1 => w.s(e.src.name().as_bytes()),
                    2 => w.s(e.name.as_bytes()),
                    _ => w.s(e.text.as_bytes()),
                }
                b""
            }
            N_CAT => return ld(super::chan_at(z, x[1] as u16, st)),
            N_ROOM => return st.zone_room(z, x[1] as u16) as i32,
            N_ISUB => return SUBS.iter().any(|(q, _)| *q == (a as u8, x[1] as u8)) as i32,
            N_FCHG => return self.fchg(cx, a as usize % FIELDS.len(), x[1], x[2]),
            N_SEEK => return seek(st, a, x[1], x[2], x[3] != 0),
            N_NITEMS | N_INAME | N_IHELP | N_IVAL | N_SCHG | N_SRESET => return self.setting(cx, w, id, x),
            _ => {
                // a slot's state (A, B, C)
                let i = (a as usize).min(SLOTS - 1);
                let (s, l) = (cx.app.slot(i as u8), cx.live.slots[i]);
                match id {
                    N_SLOTCH => return ld(s.ch.filter(|_| s.on_channel())),
                    N_APRS => return cx.app.aprs_slot(i as u8) as i32,
                    N_SLOTF => return if l.scanning { l.freq } else { cx.app.tuned_freq(i as u8) } as i32,
                    N_SLOTOFF => return cx.app.slot_off(i as u8) as i32,
                    N_SQ => return l.squelch_open as i32,
                    N_SCANNING => return l.scanning as i32,
                    N_LIVEF => return l.freq as i32,
                    N_SLOTMODE => mode_chip(s.cfg(), &mut v),
                    N_SLOTTONE => tone_text(s.cfg().tx_tone, &mut v),
                    _ => {
                        w.s(w::s_text(l.rssi).as_bytes());
                        return 0;
                    }
                }
                w.s(v.as_bytes());
                return 0;
            }
        };
        w.s(s);
        0
    }

    /// A setting by its menu numbers (group, third-level list or 0, row): the count, an item's name, help or value,
    /// a reset, or a change (TERM-6, TERM-7): 0 changed, 1 not settable yet, 2 open on the radio's menu (TERM-17),
    /// 3 left to the radio, 4 greyed (the note says why).
    #[inline(never)]
    fn setting<S: ChannelStore>(&mut self, cx: &mut Cx<S>, w: &mut W, id: u8, x: [i32; 4]) -> i32 {
        let (g, sub, i, d) = (x[0], x[1], if id == N_NITEMS { 0 } else { x[2] }, x[3]);
        let items = groups::items(g as u8, sub as u8);
        let (p, pl) = path(g, sub, i);
        let p = &p[..pl];
        let e = items.get(i as usize);
        match id {
            N_NITEMS => return items.len() as i32,
            N_INAME => w.s(e.map_or("", |e| e.name()).as_bytes()),
            N_IHELP => w.s(e.and_then(|e| e.help()).map(crate::text::load).as_ref().map_or(&b""[..], |t| t.as_bytes())),
            N_IVAL => {
                let mut v = Buf::<24>::new();
                let dot = match groups::wired(cx.app, cx.dev, p, &mut v) {
                    Some(d) => d,
                    None => {
                        match p {
                            [2, 1] => {
                                v.push(digits(cx.store.count() as u32, &mut [0; 10]));
                                v.push(b" channels");
                            }
                            _ => v.set(e.map_or("", |e| e.value()).as_bytes()),
                        }
                        false
                    }
                };
                if let Some(why) = groups::greyed(cx.app, p) {
                    v.set(why.as_bytes());
                }
                w.s(if dot { b"* " } else { b"  " });
                w.s(v.as_bytes());
            }
            N_SRESET => {
                let ok = !blocked(p);
                if ok {
                    groups::reset(cx.app, p);
                }
                return ok as i32;
            }
            _ if radio_setting(cx.app).is_some_and(|(r, rl)| r[..rl] == *p) => return 2, // TERM-17: the radio's menu has it
            _ if blocked(p) => return 3,
            _ => {
                if let Some(why) = groups::greyed(cx.app, p) {
                    self.note.set(why.as_bytes());
                    (self.warn, self.bnote) = (true, false);
                    return 4;
                }
                return !groups::change(cx.app, p, d) as i32;
            }
        }
        0
    }

    /// A field's change through its rules (the radio's channel editor: TERM-6, TERM-7): 0 refused (the note says
    /// why), 1 and 2 the home channel set or not, 3 open in the radio's editor (TERM-17), 4 saved, 5 the same.
    #[inline(never)]
    fn fchg<S: ChannelStore>(&mut self, cx: &mut Cx<S>, i: usize, op: i32, d: i32) -> i32 {
        let Some(c) = self.pg.edit else { return 5 };
        let (app, st, line) = (&mut *cx.app, cx.store, self.pg.line);
        let room = |z| st.zone_room(z, c.num);
        let (o, zones, room): (_, _, &dyn Fn(ZoneId) -> bool) = match op {
            0 => (Op::Step(d), st.zones(), &room),
            1 => (Op::Reset, 0, &any_room),
            _ => (Op::Typed(line.as_bytes()), 0, &any_room),
        };
        let (mut n, mut home) = (c, app.set.home_ch);
        if let Err(r) = FIELDS[i].change(&mut n, &mut home, o, Ctx { zones, region: app.set.region(), room }) {
            let mut m = cmd::Msg::new();
            r.line(&mut m);
            (self.note, self.warn, self.bnote) = (m, true, false);
            return 0;
        }
        if home != app.set.home_ch {
            app.set.home_ch = home;
            return if home == Some(c.num) { 1 } else { 2 };
        }
        if radio_channel(app) == Some(n.num) {
            return 3;
        }
        if Some(n) == self.pg.edit {
            return 5;
        }
        app.put_channel(n);
        self.pg.edit = Some(n);
        4
    }

    /// A new channel at the first free number, in `zone`, on the active slot's frequency (TERM-14), now the one
    /// edited: its number, -1 with every number used, -2 when the slot's settings don't fit a channel (said).
    #[inline(never)]
    fn new_channel<S: ChannelStore>(&mut self, cx: &mut Cx<S>, zone: ZoneId) -> i32 {
        let Some(num) = cx.store.free_num(0) else { return -1 };
        let s = &cx.app.slots[cx.app.active as usize];
        let rx = s.freq().clamp(mhz(64), mhz(1300)); // CHAN-22
        let c = Channel { num, zone, rx, name: [0; 16], cfg: *s.cfg(), flags: 0, tones: 0 };
        if let Err(r) = check_edit(&c) {
            let mut m = cmd::Msg::new();
            r.line(&mut m);
            (self.note, self.warn, self.bnote) = (m, true, true);
            return -2; // the slot's TX wouldn't fit a channel (CHAN-22, TX-37)
        }
        cx.app.put_channel(c);
        self.pg.edit = Some(c);
        num as i32
    }
}
