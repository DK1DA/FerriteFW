//! The text pack (decision 335, UPD-22): long, rarely read text (notes, help, the terminal's usage lines) kept on the
//! SPI flash instead of the MCU's. A call site names its text by the literal itself, `txt!("...")`, which build.rs
//! turns into the text's number; the literal never reaches the MCU's flash. The pack is an SPI part
//! ([`ferrite_core::part::TEXT`]) installed with every update; without it (missing, another build's, damaged) a text
//! reads empty and its user shows its short fallback, or nothing for a note.
//!
//! A text is read when it is drawn or sent, into the caller's [`Text`] (255 bytes at most, on the stack): two short
//! reads, never per frame (a screen's region hashes the text's number, not its bytes).
//!
//! The body (the part after its header): count u16, 0 u16, an entry per text (u32: the offset from the body's start,
//! the length in the top 8 bits), then the texts, UTF-8.

use ferrite_core::part;

/// A text in the pack, by number (`txt!`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Txt(pub u16);

/// A text read from the pack.
#[derive(Clone)]
pub struct Text {
    n: u8,
    b: [u8; 255],
}

impl Text {
    pub const EMPTY: Text = Text { n: 0, b: [0; 255] };

    pub fn as_bytes(&self) -> &[u8] {
        &self.b[..self.n as usize]
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// A text from bytes (cut at 255).
    pub fn from_bytes(b: &[u8]) -> Text {
        let mut t = Text::EMPTY;
        let n = b.len().min(255);
        t.b[..n].copy_from_slice(&b[..n]);
        t.n = n as u8;
        t
    }

    /// The text, or `or` when the pack didn't have it.
    pub fn or<'a>(&'a self, or: &'a [u8]) -> &'a [u8] {
        if self.n == 0 { or } else { self.as_bytes() }
    }
}

/// The pack's body as built with this crate (host builds only: the packer, tests and the simulators without parts).
#[cfg(not(target_os = "none"))]
pub static BODY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/text_en.bin"));

/// Body bytes at `off`: from the SPI part, or on a host with no reader set, from [`BODY`].
fn read(off: u32, buf: &mut [u8]) -> bool {
    #[cfg(not(target_os = "none"))]
    if !part::has_reader() {
        return match BODY.get(off as usize..off as usize + buf.len()) {
            Some(s) => {
                buf.copy_from_slice(s);
                true
            }
            None => false,
        };
    }
    part::read_body(&part::TEXT, off, buf)
}

/// Whether the pack is there to read (UPD-22: About and the self-test say when it isn't).
pub fn present() -> bool {
    (cfg!(not(target_os = "none")) && !part::has_reader()) || part::ok(&part::TEXT)
}

/// Reads a text (empty when the pack is missing).
#[inline(never)]
pub fn load(t: Txt) -> Text {
    let mut out = Text::EMPTY;
    let mut e = [0u8; 4];
    if read(4 + 4 * t.0 as u32, &mut e) {
        let v = u32::from_le_bytes(e);
        let n = (v >> 24) as usize;
        if read(v & 0xFF_FFFF, &mut out.b[..n]) && core::str::from_utf8(&out.b[..n]).is_ok() {
            out.n = n as u8;
        }
    }
    out
}

/// The text a table string's two-character code names (`txtc!`: a control character 0x01-0x1F, then one of
/// 0x20-0x5F, so never a `|`), if it is one.
pub fn code(s: &str) -> Option<Txt> {
    match *s.as_bytes() {
        [a @ 1..=0x1F, b @ 0x20..=0x5F] => Some(Txt((a as u16 - 1) << 6 | (b as u16 - 0x20))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UPD-22: every text in the built pack reads back as its literal (the host's built-in body).
    #[test]
    fn texts_read_back() {
        let n = u16::from_le_bytes([BODY[0], BODY[1]]) as usize;
        assert!(n > 100, "the texts went to the pack");
        for i in 0..n {
            let e = u32::from_le_bytes(BODY[4 + 4 * i..8 + 4 * i].try_into().unwrap());
            let t = load(Txt(i as u16));
            assert_eq!(t.as_bytes(), &BODY[(e & 0xFF_FFFF) as usize..][..(e >> 24) as usize], "text {i}");
        }
        assert_eq!(load(txt!("Commands: type : then one of these, and Enter. A dump is made of the same lines.")).as_str(), "Commands: type : then one of these, and Enter. A dump is made of the same lines.");
        assert_eq!(code("\u{3}\u{25}"), Some(Txt(133)));
        assert_eq!((code("abc"), code("ab")), (None, None));
    }
}
