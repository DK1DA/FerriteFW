//! SPI parts (decision 335, UPD-20 – UPD-23): data the firmware keeps on the SPI flash instead of its own, installed
//! from the `.ffw` update file with every update (the text pack, the terminal's page templates, the boot logo, predictive text's words).
//!
//! Each part starts with a 32-byte header (little-endian):
//!
//! ```text
//! magic [4] | format u16 (1) | reserved u16 | build [16] (the build ID, SELF-5, NUL-padded) | len u32 (the body's
//! bytes) | crc u32 (CRC-32 of the body, zlib's)
//! ```
//!
//! then its body. The firmware uses a part only when the header is its kind's, the build ID is its own
//! ([`crate::BUILD`]) and the CRC holds ([`mount`], once at power-on); otherwise every user of it falls back (a
//! short built-in message, the plain command line, the wordmark without its logo).
//!
//! Who reads the chip: the firmware sets [`set_reader`] once its store is made (reads straight from the W25Q, a
//! program or erase suspended meanwhile); the simulator sets one over the part files it was given. With no reader,
//! a host build (tests, the simulators by default) uses the parts built into it, so goldens keep the full text.

use core::ops::Range;
use core::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering::Relaxed};

/// The header's size.
pub const HEADER: usize = 32;
/// The header format this firmware reads and the packer writes.
pub const FORMAT: u16 = 1;

/// A kind of part: its magic, its file in the `.ffw` (`spi/NAME`) and the SPI flash it may take (its start is where
/// it goes; the slot's end is its maximum size).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kind {
    pub magic: [u8; 4],
    pub file: &'static str,
    pub slot: Range<u32>,
}

/// The English text pack: layout 3's text region (64 KB at 0x090000).
pub const TEXT: Kind = Kind { magic: *b"FTXT", file: "text-en.bin", slot: crate::layout::TEXT_PACK };
/// The boot logo's two masks: the first 32 KB of layout 3's boot screens region.
pub const LOGO: Kind = Kind { magic: *b"FLGO", file: "logo.bin", slot: crate::layout::SCREENS.start..crate::layout::SCREENS.start + 0x8000 };
/// The terminal's page templates (TERM-47): the next 32 KB of the boot screens region.
pub const TERM: Kind = Kind { magic: *b"FVTP", file: "term-vt.bin", slot: crate::layout::SCREENS.start + 0x8000..crate::layout::SCREENS.start + 0x1_0000 };

/// Predictive text's words (decision 345): layout 3's dictionary region (64 KB at 0x0B0000).
pub const DICT: Kind = Kind { magic: *b"FDIC", file: "words-en.bin", slot: crate::layout::DICTIONARY };

/// Every kind, in the order [`mount`]'s bits and the `.ffw`'s parts list them.
pub const KINDS: [Kind; 4] = [TEXT, LOGO, TERM, DICT];

/// A part's header for `body`, as the packer writes it (`build`: the firmware's build ID, at most 16 bytes).
pub fn header(kind: &Kind, build: &str, body: &[u8]) -> [u8; HEADER] {
    let mut h = [0u8; HEADER];
    h[0..4].copy_from_slice(&kind.magic);
    h[4..6].copy_from_slice(&FORMAT.to_le_bytes());
    let b = build.as_bytes();
    let n = b.len().min(16);
    h[8..8 + n].copy_from_slice(&b[..n]);
    h[24..28].copy_from_slice(&(body.len() as u32).to_le_bytes());
    h[28..32].copy_from_slice(&crc32(0, body).to_le_bytes());
    h
}

/// The header's body length and CRC when it is `kind`'s, in this format, for `build`, and its body fits the slot.
pub fn check_header(kind: &Kind, h: &[u8; HEADER], build: &str) -> Option<(u32, u32)> {
    let le32 = |i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
    let b = build.as_bytes();
    let n = b.len().min(16);
    let id_ok = h[8..8 + n] == b[..n] && h[8 + n..24].iter().all(|&c| c == 0);
    let len = le32(24);
    let fits = (len as u64) + HEADER as u64 <= kind.slot.len() as u64;
    (h[0..4] == kind.magic && u16::from_le_bytes([h[4], h[5]]) == FORMAT && id_ok && fits && len > 0).then_some((len, le32(28)))
}

/// zlib's CRC-32, continued from `crc` (start at 0), a bit at a time: no 1 KB table in the image for a check made
/// once at power-on (30 KB, a few ms).
pub fn crc32(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// Reads `buf.len()` bytes of the SPI flash at an address: false when it can't.
pub type ReadFn = fn(u32, &mut [u8]) -> bool;

/// The reader, as a `ReadFn`'s address (0: none set).
static READER: AtomicUsize = AtomicUsize::new(0);
/// Which kinds [`mount`] found good (bit i: `KINDS[i]`), and their body lengths.
static OK: AtomicU8 = AtomicU8::new(0);
static LEN: [AtomicU32; KINDS.len()] = [const { AtomicU32::new(0) }; KINDS.len()];

/// Sets the reader (the firmware: its store's raw read; the simulator: its part files). Call [`mount`] after.
pub fn set_reader(f: ReadFn) {
    READER.store(f as usize, Relaxed);
}

/// Whether a reader was set (a host build without one uses its built-in parts).
pub fn has_reader() -> bool {
    READER.load(Relaxed) != 0
}

fn read(addr: u32, buf: &mut [u8]) -> bool {
    let f = READER.load(Relaxed);
    if f == 0 {
        return false;
    }
    // SAFETY: only `set_reader` stores here, and it stores a `ReadFn`.
    let f: ReadFn = unsafe { core::mem::transmute::<usize, ReadFn>(f) };
    f(addr, buf)
}

/// Checks one part through `read`: its body length when the header is good for `build` and the body's CRC holds.
/// Reads the body in 256-byte pieces (a stack buffer: no RAM kept).
#[inline(never)]
pub fn check(kind: &Kind, build: &str, read: &mut dyn FnMut(u32, &mut [u8]) -> bool) -> Option<u32> {
    let mut h = [0u8; HEADER];
    if !read(kind.slot.start, &mut h) {
        return None;
    }
    let (len, crc) = check_header(kind, &h, build)?;
    let (mut c, mut at, mut buf) = (0u32, 0u32, [0u8; 256]);
    while at < len {
        let n = (len - at).min(buf.len() as u32) as usize;
        if !read(kind.slot.start + HEADER as u32 + at, &mut buf[..n]) {
            return None;
        }
        c = crc32(c, &buf[..n]);
        at += n as u32;
    }
    (c == crc).then_some(len)
}

/// Checks every part through the reader (at power-on, once the store is made; about 20 ms for 30 KB): which are
/// good, as bits of [`KINDS`]. A part not good is never read.
pub fn mount() -> u8 {
    let mut ok = 0;
    for (i, k) in KINDS.iter().enumerate() {
        let len = check(k, crate::BUILD, &mut read).unwrap_or(0);
        LEN[i].store(len, Relaxed);
        ok |= ((len != 0) as u8) << i;
    }
    OK.store(ok, Relaxed);
    ok
}

fn index(kind: &Kind) -> usize {
    KINDS.iter().position(|k| k.magic == kind.magic).unwrap_or(0)
}

/// How many parts are good, of how many: a host build with no reader set has its own, all of them.
pub fn count() -> (u32, u32) {
    let all = KINDS.len() as u32;
    if cfg!(not(target_os = "none")) && !has_reader() {
        return (all, all);
    }
    (OK.load(Relaxed).count_ones(), all)
}

/// Whether [`mount`] found the part good.
pub fn ok(kind: &Kind) -> bool {
    OK.load(Relaxed) & (1 << index(kind)) != 0
}

/// A good part's body length (0: not good).
pub fn len(kind: &Kind) -> u32 {
    if ok(kind) { LEN[index(kind)].load(Relaxed) } else { 0 }
}

/// Reads a good part's body bytes at `off`: false when the part isn't good or the bytes lie past its end.
pub fn read_body(kind: &Kind, off: u32, buf: &mut [u8]) -> bool {
    let end = off as u64 + buf.len() as u64;
    ok(kind) && end <= len(kind) as u64 && read(kind.slot.start + HEADER as u32 + off, buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    fn image(kind: &Kind, build: &str, body: &[u8]) -> Vec<u8> {
        let mut v = header(kind, build, body).to_vec();
        v.extend_from_slice(body);
        v
    }

    /// UPD-21: a part is used only when its kind, format, build ID and CRC all hold.
    #[test]
    fn a_part_is_good_only_for_its_build_and_crc() {
        let body = b"hello, parts".repeat(40);
        let img = image(&TEXT, "abc1234", &body);
        let mut rd = |a: u32, b: &mut [u8]| {
            let o = (a - TEXT.slot.start) as usize;
            img.get(o..o + b.len()).map(|s| b.copy_from_slice(s)).is_some()
        };
        assert_eq!(check(&TEXT, "abc1234", &mut rd), Some(body.len() as u32));
        assert_eq!(check(&TEXT, "abc1234+", &mut rd), None, "another build");
        assert_eq!(check(&TEXT, "abc123", &mut rd), None, "a prefix of the ID isn't the ID");
        assert_eq!(check(&LOGO, "abc1234", &mut rd), None, "another kind's magic");
        let mut bad = img.clone();
        bad[HEADER + 5] ^= 1;
        let mut rd2 = |a: u32, b: &mut [u8]| {
            let o = (a - TEXT.slot.start) as usize;
            bad.get(o..o + b.len()).map(|s| b.copy_from_slice(s)).is_some()
        };
        assert_eq!(check(&TEXT, "abc1234", &mut rd2), None, "a flipped bit");
        let blank = [0xFFu8; 64];
        let mut rd3 = |_a: u32, b: &mut [u8]| {
            b.copy_from_slice(&blank[..b.len()]);
            true
        };
        assert_eq!(check(&TEXT, "abc1234", &mut rd3), None, "an erased slot");
        let mut big = header(&TERM, "x", &[0; 4]);
        big[24..28].copy_from_slice(&(TERM.slot.len() as u32).to_le_bytes());
        assert_eq!(check_header(&TERM, &big, "x"), None, "a body past the slot");
    }

    #[test]
    fn crc_is_zlibs() {
        assert_eq!(crc32(0, b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(crc32(0, b"1234"), b"56789"), crate::reader::crc32(0, b"123456789"));
    }

    /// The slots: each inside the region it belongs to, on sector boundaries, writable, apart.
    #[test]
    fn slots_are_writable_and_apart() {
        for k in &KINDS {
            assert!(crate::layout::writable(k.slot.start, k.slot.len() as u32), "{}", k.file);
            assert!(k.slot.start % 4096 == 0 && k.slot.end % 4096 == 0);
        }
        assert!(TEXT.slot == crate::layout::TEXT_PACK);
        const { assert!(LOGO.slot.end <= TERM.slot.start && TERM.slot.end <= crate::layout::SCREENS.end) };
    }
}
