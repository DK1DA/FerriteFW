//! Text formatting without `core::fmt`: a fixed buffer that `ufmt::uwrite!` writes into, and the few
//! printf conversions ufmt lacks (decimal widths, `%+d`, `%-5s`). Hex with zero padding is ufmt's `{:04X}`.
use core::convert::Infallible;
use ufmt::{uDisplay, uWrite, Formatter};

/// A fixed buffer for formatted text: what does not fit is cut off, as `snprintf` does.
#[derive(Clone, Copy)]
pub struct Buf<const N: usize> {
    b: [u8; N],
    n: usize,
}

impl<const N: usize> Buf<N> {
    pub const fn new() -> Self {
        Buf { b: [0; N], n: 0 }
    }

    pub fn clear(&mut self) {
        self.n = 0;
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.b[..self.n]
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn push(&mut self, s: &[u8]) {
        let take = s.len().min(N - self.n);
        self.b[self.n..self.n + take].copy_from_slice(&s[..take]);
        self.n += take;
    }

    pub fn set(&mut self, s: &[u8]) {
        self.n = 0;
        self.push(s);
    }
}

impl<const N: usize> PartialEq for Buf<N> {
    fn eq(&self, o: &Self) -> bool {
        self.as_bytes() == o.as_bytes()
    }
}

impl<const N: usize> Eq for Buf<N> {}

impl<const N: usize> core::hash::Hash for Buf<N> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.as_bytes().hash(h);
    }
}

impl<const N: usize> Default for Buf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Buf<N> {
    /// A buffer holding s (cut off at N).
    pub fn from(s: &[u8]) -> Self {
        let mut b = Self::new();
        b.push(s);
        b
    }
}

impl<const N: usize> uWrite for Buf<N> {
    type Error = Infallible;
    fn write_str(&mut self, s: &str) -> Result<(), Infallible> {
        self.push(s.as_bytes());
        Ok(())
    }
}

/// Formats into a buffer (cleared first) and gives back its bytes: `v!(b, "{} ms", t)`.
#[macro_export]
macro_rules! v {
    ($buf:expr, $($fmt:tt)*) => {{
        $buf.clear();
        let _ = ufmt::uwrite!($buf, $($fmt)*);
        $buf.as_bytes()
    }};
}

fn pad<W: uWrite + ?Sized>(f: &mut Formatter<'_, W>, n: usize, c: char) -> Result<(), W::Error> {
    for _ in 0..n {
        f.write_char(c)?;
    }
    Ok(())
}

fn digits(mut v: u32) -> usize {
    let mut n = 1;
    while v >= 10 {
        v /= 10;
        n += 1;
    }
    n
}

/// `%0Nu`
pub struct Zero(pub u32, pub usize);
impl uDisplay for Zero {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        pad(f, self.1.saturating_sub(digits(self.0)), '0')?;
        self.0.fmt(f)
    }
}

/// `%Nu`: right-aligned with spaces.
pub struct Wide(pub u32, pub usize);
impl uDisplay for Wide {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        pad(f, self.1.saturating_sub(digits(self.0)), ' ')?;
        self.0.fmt(f)
    }
}

/// `%+d`
pub struct Plus(pub i32);
impl uDisplay for Plus {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        if self.0 >= 0 {
            f.write_char('+')?;
        }
        self.0.fmt(f)
    }
}

/// Bytes that are text in C (`%s` of a char array): printable ASCII as is, anything else as '?'.
pub struct Bytes<'a>(pub &'a [u8]);
impl uDisplay for Bytes<'_> {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        for &c in self.0 {
            f.write_char(if (0x20..0x7F).contains(&c) { c as char } else { '?' })?;
        }
        Ok(())
    }
}

/// `%-Ns`: left-aligned, padded with spaces.
pub struct Left<'a>(pub &'a str, pub usize);
impl uDisplay for Left<'_> {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        f.write_str(self.0)?;
        pad(f, self.1.saturating_sub(self.0.len()), ' ')
    }
}

/// `%Ns`: right-aligned, padded with spaces.
pub struct Right<'a>(pub &'a str, pub usize);
impl uDisplay for Right<'_> {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        pad(f, self.1.saturating_sub(self.0.len()), ' ')?;
        f.write_str(self.0)
    }
}

/// `%c` of a byte (port letters, SI4732 revision characters).
pub struct Ch(pub u8);
impl uDisplay for Ch {
    fn fmt<W: uWrite + ?Sized>(&self, f: &mut Formatter<'_, W>) -> Result<(), W::Error> {
        Bytes(&[self.0]).fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The cases of fw/tests/host/test_fmt.c that the firmware uses, through ufmt.
    #[test]
    fn conversions() {
        let mut b = Buf::<63>::new();
        assert_eq!(v!(b, "key {} {} {}", Wide(7, 2), "matrix", "down"), b"key  7 matrix down");
        assert_eq!(v!(b, "{}:{}:{}", Zero(0, 2), Zero(4, 2), Zero(9, 2)), b"00:04:09");
        assert_eq!(v!(b, "{} {} {}", Plus(12), Plus(-12), Plus(0)), b"+12 -12 +0");
        assert_eq!(v!(b, "{}", i32::MIN), b"-2147483648");
        assert_eq!(v!(b, "{:x} {:04x} {:06X}", 255u32, 255u32, 0xEF4015u32), b"ff 00ff EF4015");
        assert_eq!(v!(b, "{}|{}|", Left("ab", 4), Right("ab", 4)), b"ab  |  ab|");
        assert_eq!(v!(b, "{:08X}", 0x1234_ABCDu32), b"1234ABCD");
        assert_eq!(v!(b, "{}{}", Bytes(b"P\xFFx"), Ch(b'C')), b"P?xC");
        let mut small = Buf::<4>::new();
        assert_eq!(v!(small, "{}", "abcdefgh"), b"abcd"); // truncation
        assert_eq!(v!(small, "{}", 123456), b"1234");
    }
}
