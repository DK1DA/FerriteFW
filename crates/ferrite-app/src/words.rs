//! Predictive text's dictionary (manual TEXT-1 to TEXT-11): about 570 message and amateur-radio words packed
//! by `tools/mkwords.py` into `assets/words/dict.bin` (format v2, in that tool's docstring). The words (header,
//! bucket table, entries) are an SPI part (`words-en.bin`, decision 345) installed with every update and read
//! through a 64-byte window (TEXT-8: no copy in RAM); without the part no words are offered. A word is found by
//! its keys (2 abc … 9 wxyz): the bucket of its first two keys is decoded entry by entry, and the entries with
//! the same keys come best first. The short forms (TEXT-4) and the factory quick messages (TEXT-7) stay in MCU
//! flash, read in place.

use ferrite_core::part;
use ferrite_ui::text::Buf;

const BLOB: &[u8] = include_bytes!("../../../assets/words/dict.bin");
const fn u32_le(b: &[u8], i: usize) -> usize {
    b[i] as usize | (b[i + 1] as usize) << 8 | (b[i + 2] as usize) << 16 | (b[i + 3] as usize) << 24
}
/// Where the short forms start (the header's first section offset): the words before it are the SPI part
/// ([`ferrite_core::part::DICT`], decision 345), the short forms and quick messages after it stay in MCU flash.
const SHORT_AT: usize = u32_le(BLOB, 8);
/// Where the quick messages start (the header's second section offset).
const QUICK_AT: usize = u32_le(BLOB, 12);
const _: () = assert!(BLOB.len() > SHORT_AT && QUICK_AT > SHORT_AT && BLOB[4] == 2);

/// The part's body as built with this crate: the header, the bucket table and the words (host builds only: the
/// packer, tests and the simulators without parts).
#[cfg(not(target_os = "none"))]
pub static BODY: &[u8] = BLOB.split_at(SHORT_AT).0;

/// The short forms and the factory quick messages, kept in MCU flash (read in place, no copy in RAM).
#[cfg_attr(target_os = "none", link_section = ".slow_rodata")]
static TAIL: [u8; BLOB.len() - SHORT_AT] = {
    let mut a = [0u8; BLOB.len() - SHORT_AT];
    let mut i = 0;
    while i < a.len() {
        a[i] = BLOB[SHORT_AT + i];
        i += 1;
    }
    a
};

/// Word bytes at `off`: from the SPI part, or on a host with no reader set, from [`BODY`].
fn read(off: usize, buf: &mut [u8]) -> bool {
    #[cfg(not(target_os = "none"))]
    if !part::has_reader() {
        return match BODY.get(off..off + buf.len()) {
            Some(s) => {
                buf.copy_from_slice(s);
                true
            }
            None => false,
        };
    }
    part::read_body(&part::DICT, off as u32, buf)
}

/// Whether the words are there to read (without the part, predictive text offers no words).
pub fn present() -> bool {
    (cfg!(not(target_os = "none")) && !part::has_reader()) || part::ok(&part::DICT)
}

/// A small window on the words, refilled as a lookup walks on (a bucket is read in order).
struct Rd {
    base: usize,
    n: usize,
    buf: [u8; 64],
}

impl Rd {
    fn new() -> Self {
        Rd { base: 0, n: 0, buf: [0; 64] }
    }

    fn at(&mut self, i: usize) -> u8 {
        if i < self.base || i >= self.base + self.n {
            self.fill(i);
        }
        if i >= self.base && i < self.base + self.n { self.buf[i - self.base] } else { 0 }
    }

    #[inline(never)]
    fn fill(&mut self, i: usize) {
        let n = self.buf.len().min(SHORT_AT.saturating_sub(i));
        self.base = i;
        self.n = if n > 0 && read(i, &mut self.buf[..n]) { n } else { 0 };
    }

    fn u16_at(&mut self, i: usize) -> usize {
        self.at(i) as usize | (self.at(i + 1) as usize) << 8
    }

    fn u32_at(&mut self, i: usize) -> usize {
        self.u16_at(i) | self.u16_at(i + 2) << 16
    }
}

/// Header: magic, version, count, then the two section offsets; the bucket table (72 of 6 bytes) after it.
const HEAD: usize = 16;
/// The longest word.
pub const WORD_MAX: usize = 15;

/// A dictionary word: its letters (lower case) and whether it is shown and sent in capitals (QSL, TNX).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Word {
    pub text: Buf<16>,
    pub caps: bool,
}

/// The key that types a letter (2 abc … 9 wxyz), as an ASCII digit; 0 for anything else.
pub fn key_of(c: u8) -> u8 {
    match c.to_ascii_lowercase() {
        b'a'..=b'c' => b'2',
        b'd'..=b'f' => b'3',
        b'g'..=b'i' => b'4',
        b'j'..=b'l' => b'5',
        b'm'..=b'o' => b'6',
        b'p'..=b's' => b'7',
        b't'..=b'v' => b'8',
        b'w'..=b'z' => b'9',
        _ => 0,
    }
}

/// The `n`th word (best first) whose keys are `keys` (ASCII digits 2-9), if there is one.
pub fn nth(keys: &[u8], n: usize) -> Option<Word> {
    let mut found = 0;
    let mut out = None;
    each(keys, |w| {
        if found == n {
            out = Some(w);
            return false;
        }
        found += 1;
        true
    });
    out
}

/// How many words have these keys.
pub fn count(keys: &[u8]) -> usize {
    let mut n = 0;
    each(keys, |_| {
        n += 1;
        true
    });
    n
}

/// Calls `f` with each word whose keys are `keys`, best first, until it returns false.
fn each(keys: &[u8], mut f: impl FnMut(Word) -> bool) {
    if keys.is_empty() || keys.len() > WORD_MAX || keys.iter().any(|k| !(b'2'..=b'9').contains(k)) {
        return;
    }
    let mut d = Rd::new();
    if !(d.at(0) == b'F' && d.at(1) == b'T' && d.at(2) == b'9' && d.at(3) == b'D' && d.at(4) == 2) {
        return;
    }
    let b = if keys.len() == 1 { 64 + (keys[0] - b'2') as usize } else { (keys[0] - b'2') as usize * 8 + (keys[1] - b'2') as usize };
    let t = HEAD + b * 6;
    let (mut at, n) = (d.u32_at(t), d.u16_at(t + 4));
    let mut prev = [0u8; WORD_MAX];
    for _ in 0..n {
        let h = d.at(at);
        let (p, sl) = ((h >> 4) as usize, (h & 15) as usize);
        at += 1;
        let mut word = prev;
        let (mut bits, mut nb, mut k) = (0u32, 0, at);
        for c in word.iter_mut().skip(p).take(sl) {
            while nb < 5 {
                bits = bits << 8 | d.at(k) as u32;
                k += 1;
                nb += 8;
            }
            nb -= 5;
            *c = b'a' - 1 + (bits >> nb & 31) as u8;
        }
        at += (sl * 5).div_ceil(8);
        let flags = d.at(at);
        at += 1;
        let len = p + sl;
        prev = word;
        let w = &word[..len];
        if len == keys.len() && w.iter().zip(keys).all(|(&c, &k)| key_of(c) == k) && !f(Word { text: Buf::from(w), caps: flags & 0x80 != 0 }) {
            return;
        }
    }
}

/// The NUL-terminated strings of a section, from `at`: `count` of them (pairs count as two each).
fn strings(at: usize) -> impl Iterator<Item = &'static [u8]> {
    let at = at - SHORT_AT;
    let n = TAIL[at] as usize;
    let mut i = at + 1;
    core::iter::from_fn(move || {
        let s = &TAIL[i.min(TAIL.len())..];
        let end = s.iter().position(|&c| c == 0)?;
        i += end + 1;
        Some(&s[..end])
    })
    .take(n * 2)
}

/// The short form offered after `word` (TEXT-4): received → QSL, thanks → TNX. Capitals.
pub fn short(word: &[u8]) -> Option<&'static [u8]> {
    let mut it = strings(SHORT_AT);
    while let (Some(w), Some(s)) = (it.next(), it.next()) {
        if w.eq_ignore_ascii_case(word) {
            return Some(s);
        }
    }
    None
}

/// A phrase's short form ending in `word` (TEXT-4): see you later → CUL when "see you" was typed just before it.
/// `head` is the text before the word; the second value is how many of its bytes the short form replaces (the
/// phrase's earlier words and the spaces after them).
pub fn phrase(head: &[u8], word: &[u8]) -> Option<(&'static [u8], usize)> {
    let h = head.trim_ascii_end();
    let mut it = strings(SHORT_AT);
    while let (Some(p), Some(s)) = (it.next(), it.next()) {
        let Some(sp) = p.iter().rposition(|&c| c == b' ') else { continue };
        let (front, last) = (&p[..sp], &p[sp + 1..]);
        if !last.eq_ignore_ascii_case(word) || h.len() < front.len() {
            continue;
        }
        let at = h.len() - front.len();
        if h[at..].eq_ignore_ascii_case(front) && (at == 0 || h[at - 1] == b' ') {
            return Some((s, head.len() - at));
        }
    }
    None
}

/// The factory quick messages (TEXT-7): `i` of [`QUICK`].
pub fn quick(i: usize) -> &'static [u8] {
    let at = QUICK_AT - SHORT_AT;
    let n = TAIL[at] as usize;
    let mut s = &TAIL[at + 1..];
    for k in 0..n {
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        if k == i {
            return &s[..end];
        }
        s = &s[(end + 1).min(s.len())..];
    }
    b""
}

/// How many quick messages there are.
pub const QUICK: usize = 9;

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(w: &str) -> std::vec::Vec<u8> {
        w.bytes().map(key_of).collect()
    }

    fn words(w: &str) -> std::vec::Vec<std::string::String> {
        let k = keys(w);
        (0..count(&k)).map(|i| std::string::String::from_utf8(nth(&k, i).unwrap().text.as_bytes().to_vec()).unwrap()).collect()
    }

    extern crate std;

    /// TEXT-9: core.txt's order settles words with the same keys.
    #[test]
    fn core_order() {
        assert_eq!(words("home")[..2], ["good", "home"]);
        assert_eq!(words("on")[0], "on");
        assert!(words("on").contains(&"no".into()));
        assert_eq!(words("me")[0], "me");
        assert_eq!(words("thanks"), ["thanks"]);
        assert_eq!(words("i"), ["i"]);
    }

    /// TEXT-3: keys that spell no word find nothing.
    #[test]
    fn no_word() {
        assert_eq!(count(b"99999"), 0);
        assert!(nth(b"99999", 0).is_none());
        assert_eq!(count(b""), 0);
        assert_eq!(count(b"21"), 0, "1 is punctuation, not a letter");
    }

    /// TEXT-4: Q-codes and shorthand in capitals; short forms after their words.
    #[test]
    fn caps_and_short_forms() {
        let q = nth(&keys("qsl"), 0).unwrap();
        assert_eq!(q.text.as_bytes(), b"qsl");
        assert!(q.caps);
        assert!(!nth(&keys("home"), 0).unwrap().caps);
        assert_eq!(short(b"received"), Some(&b"QSL"[..]));
        assert_eq!(short(b"Thanks"), Some(&b"TNX"[..]));
        assert_eq!(short(b"home"), None);
        assert_eq!(short(b"later"), None, "a phrase's short form needs its earlier words ([`phrase`])");
    }

    /// TEXT-4: a phrase's short form after its last word, replacing the words typed before it.
    #[test]
    fn phrase_short_forms() {
        assert_eq!(phrase(b"Ok, see you ", b"later"), Some((&b"CUL"[..], 8)));
        assert_eq!(phrase(b"Thank ", b"you"), Some((&b"TU"[..], 6)));
        assert_eq!(phrase(b"good ", b"night"), Some((&b"GN"[..], 5)));
        assert_eq!(phrase(b"see ", b"later"), None, "the whole phrase or nothing");
        assert_eq!(phrase(b"oversee you ", b"later"), None, "at a word start");
        assert_eq!(phrase(b"", b"later"), None);
    }

    /// TEXT-7: the factory quick messages come from quick.txt.
    #[test]
    fn quick_messages() {
        assert_eq!(quick(0), b"73");
        assert_eq!(quick(2), b"On my way");
        assert_eq!(quick(8), b"Are you on the repeater?");
        assert_eq!(quick(9), b"");
    }

    /// Every word in the dictionary is found by its own keys.
    #[test]
    fn every_word_found() {
        let list = include_str!("../../../assets/words/dict.txt");
        let mut n = 0;
        for l in list.lines() {
            let w = l.split('\t').next().unwrap().to_ascii_lowercase();
            assert!(words(&w).contains(&w), "{w}");
            n += 1;
        }
        assert!(n > 500);
    }
}
