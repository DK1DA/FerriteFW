//! Predictive text's dictionary (manual TEXT-1 to TEXT-11): about 570 message and amateur-radio words packed
//! by `tools/mkwords.py` into `assets/words/dict.bin` (format v2, in that tool's docstring), linked into slow
//! flash and read in place (TEXT-8): no copy in RAM. A word is found by its keys (2 abc … 9 wxyz): the bucket
//! of its first two keys is decoded entry by entry, and the entries with the same keys come best first. The
//! blob also holds the short forms (TEXT-4) and the factory quick messages (TEXT-7).

use ferrite_ui::text::Buf;

const BLOB: &[u8] = include_bytes!("../../../assets/words/dict.bin");
#[cfg_attr(target_os = "none", link_section = ".slow_rodata")]
static DICT: [u8; BLOB.len()] = {
    let mut a = [0u8; BLOB.len()];
    let mut i = 0;
    while i < BLOB.len() {
        a[i] = BLOB[i];
        i += 1;
    }
    a
};

/// Header: magic, version, count, then the two section offsets; the bucket table after it.
const HEAD: usize = 16;
const BUCKETS: usize = 72;
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

fn u16_at(i: usize) -> usize {
    u16::from_le_bytes([DICT[i], DICT[i + 1]]) as usize
}

fn u32_at(i: usize) -> usize {
    u32::from_le_bytes([DICT[i], DICT[i + 1], DICT[i + 2], DICT[i + 3]]) as usize
}

/// Whether the blob is one this code reads.
fn valid() -> bool {
    DICT.len() > HEAD + BUCKETS * 6 && &DICT[..4] == b"FT9D" && DICT[4] == 2
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
    if keys.is_empty() || keys.len() > WORD_MAX || !valid() || keys.iter().any(|k| !(b'2'..=b'9').contains(k)) {
        return;
    }
    let b = if keys.len() == 1 { 64 + (keys[0] - b'2') as usize } else { (keys[0] - b'2') as usize * 8 + (keys[1] - b'2') as usize };
    let t = HEAD + b * 6;
    let (mut at, n) = (u32_at(t), u16_at(t + 4));
    let mut prev = [0u8; WORD_MAX];
    for _ in 0..n {
        let h = DICT[at];
        let (p, sl) = ((h >> 4) as usize, (h & 15) as usize);
        at += 1;
        let mut word = prev;
        let (mut bits, mut nb, mut k) = (0u32, 0, at);
        for c in word.iter_mut().skip(p).take(sl) {
            while nb < 5 {
                bits = bits << 8 | DICT[k] as u32;
                k += 1;
                nb += 8;
            }
            nb -= 5;
            *c = b'a' - 1 + (bits >> nb & 31) as u8;
        }
        at += (sl * 5).div_ceil(8);
        let flags = DICT[at];
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
    let n = if valid() { DICT[at] as usize } else { 0 };
    let mut i = at + 1;
    core::iter::from_fn(move || {
        let s = &DICT[i.min(DICT.len())..];
        let end = s.iter().position(|&c| c == 0)?;
        i += end + 1;
        Some(&s[..end])
    })
    .take(n * 2)
}

/// The short form offered after `word` (TEXT-4): received → QSL, thanks → TNX. Capitals.
pub fn short(word: &[u8]) -> Option<&'static [u8]> {
    if !valid() {
        return None;
    }
    let mut it = strings(u32_at(8));
    while let (Some(w), Some(s)) = (it.next(), it.next()) {
        if w.eq_ignore_ascii_case(word) {
            return Some(s);
        }
    }
    None
}

/// The factory quick messages (TEXT-7): `i` of [`QUICK`].
pub fn quick(i: usize) -> &'static [u8] {
    if !valid() {
        return b"";
    }
    let at = u32_at(12);
    let n = DICT[at] as usize;
    let mut s = &DICT[at + 1..];
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
