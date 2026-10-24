//! AX.25 UI frames as APRS uses them: destination, source and up to 8 digipeaters (7 bytes each: 6 callsign
//! characters shifted left one bit, then the SSID byte), control 0x03, PID 0xF0, then the information field.
//! Frames here are without the FCS (the modem checks and strips it).

use core::fmt;

/// Most digipeaters an address field may list.
pub const MAX_DIGIS: usize = 8;

/// A station address: up to 6 callsign characters and an SSID (0 to 15).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Address {
    call: [u8; 6],
    len: u8,
    /// 0 to 15 (higher bits are ignored when the address is written).
    pub ssid: u8,
    /// Digipeaters: has repeated the frame (shown as `*` in TNC2 text). Unused for source and destination.
    pub repeated: bool,
}

impl Address {
    /// From text such as `N0CALL-9` or `WIDE2-1`, with an optional trailing `*` (repeated). Callsigns are 1 to 6
    /// characters of `A`-`Z` and `0`-`9` (lower case is accepted and raised); SSIDs 0 to 15.
    pub fn from_text(t: &[u8]) -> Option<Self> {
        let (t, repeated) = match t.split_last() {
            Some((b'*', rest)) => (rest, true),
            _ => (t, false),
        };
        let (call, ssid) = match t.iter().position(|&c| c == b'-') {
            Some(i) => (&t[..i], number(&t[i + 1..])?),
            None => (t, 0),
        };
        if call.is_empty() || call.len() > 6 || ssid > 15 {
            return None;
        }
        let mut a = Address { call: [b' '; 6], len: call.len() as u8, ssid: ssid as u8, repeated };
        for (d, &c) in a.call.iter_mut().zip(call) {
            *d = c.to_ascii_uppercase();
            if !d.is_ascii_alphanumeric() {
                return None;
            }
        }
        Some(a)
    }

    /// From 7 address-field bytes. Returns the address and whether the extension bit marks it as the last.
    pub fn from_field(b: &[u8]) -> Option<(Self, bool)> {
        let b: &[u8; 7] = b.get(..7)?.try_into().ok()?;
        let mut a = Address { call: [b' '; 6], len: 0, ssid: (b[6] >> 1) & 15, repeated: b[6] & 0x80 != 0 };
        for i in 0..6 {
            if b[i] & 1 != 0 {
                return None; // the extension bit is only set in the SSID byte
            }
            let c = b[i] >> 1;
            match c {
                b' ' => {}
                c if c.is_ascii_alphanumeric() && a.len as usize == i => {
                    a.call[i] = c;
                    a.len += 1;
                }
                _ => return None, // not a character, or one after a space
            }
        }
        if a.len == 0 {
            return None;
        }
        Some((a, b[6] & 1 != 0))
    }

    /// The 7 address-field bytes; `last` sets the extension bit.
    pub fn to_field(&self, last: bool) -> [u8; 7] {
        let mut f = [0u8; 7];
        for i in 0..6 {
            f[i] = self.call[i] << 1;
        }
        f[6] = 0x60 | ((self.ssid & 15) << 1) | ((self.repeated as u8) << 7) | last as u8;
        f
    }

    /// The callsign without SSID.
    pub fn call(&self) -> &[u8] {
        &self.call[..self.len as usize]
    }

    /// Same station (callsign and SSID), ignoring the repeated flag.
    pub fn same(&self, o: &Address) -> bool {
        self.call() == o.call() && self.ssid == o.ssid
    }
}

impl Address {
    /// `CALL` or `CALL-SSID` as bytes, without `core::fmt` (about 2 KB of flash a firmware may not want).
    pub fn emit(&self, out: &mut impl FnMut(u8)) {
        self.call().iter().for_each(|&c| out(c));
        if self.ssid != 0 {
            out(b'-');
            if self.ssid >= 10 {
                out(b'1');
            }
            out(b'0' + self.ssid % 10);
        }
    }
}

/// `CALL` or `CALL-SSID`, without the repeated mark.
impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for &c in self.call() {
            fmt::Write::write_char(f, c as char)?;
        }
        if self.ssid != 0 {
            write!(f, "-{}", self.ssid)?;
        }
        Ok(())
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self}{}", if self.repeated { "*" } else { "" })
    }
}

fn number(t: &[u8]) -> Option<u32> {
    if t.is_empty() || t.len() > 2 || !t.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(t.iter().fold(0, |v, &c| v * 10 + (c - b'0') as u32))
}

/// A UI frame, borrowing its information field.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    pub dest: Address,
    pub src: Address,
    digis: [Address; MAX_DIGIS],
    ndigis: u8,
    pub info: &'a [u8],
}

impl<'a> Frame<'a> {
    /// A frame from its bytes (without FCS). `None` if the address field is malformed or it is not a UI frame
    /// with no layer 3 protocol (control 0x03, PID 0xF0), which is what APRS uses.
    pub fn parse(b: &'a [u8]) -> Option<Self> {
        let (dest, last) = Address::from_field(b)?;
        if last {
            return None;
        }
        let (src, mut last) = Address::from_field(&b[7..])?;
        let mut f = Frame { dest, src, digis: [dest; MAX_DIGIS], ndigis: 0, info: &[] };
        let mut at = 14;
        while !last {
            if f.ndigis as usize == MAX_DIGIS {
                return None;
            }
            let (d, l) = Address::from_field(b.get(at..)?)?;
            f.digis[f.ndigis as usize] = d;
            f.ndigis += 1;
            last = l;
            at += 7;
        }
        if b.get(at..at + 2)? != [0x03, 0xF0] {
            return None;
        }
        f.dest.repeated = false;
        f.src.repeated = false;
        f.info = &b[at + 2..];
        Some(f)
    }

    /// A frame to send. `None` if the path has more than [`MAX_DIGIS`] entries.
    pub fn new(src: Address, dest: Address, path: &[Address], info: &'a [u8]) -> Option<Self> {
        if path.len() > MAX_DIGIS {
            return None;
        }
        let mut f = Frame { dest, src, digis: [dest; MAX_DIGIS], ndigis: path.len() as u8, info };
        f.digis[..path.len()].copy_from_slice(path);
        Some(f)
    }

    /// The digipeater path.
    pub fn digis(&self) -> &[Address] {
        &self.digis[..self.ndigis as usize]
    }

    /// Bytes needed by [`Frame::write`].
    pub fn len(&self) -> usize {
        16 + 7 * self.ndigis as usize + self.info.len()
    }

    /// Always false: a frame has at least two addresses.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The frame's bytes (without FCS) into `out`; returns the length, or `None` if `out` is too small.
    pub fn write(&self, out: &mut [u8]) -> Option<usize> {
        let out = out.get_mut(..self.len())?;
        let n = self.ndigis as usize;
        out[..7].copy_from_slice(&self.dest.to_field(false));
        out[7..14].copy_from_slice(&self.src.to_field(n == 0));
        for (i, d) in self.digis().iter().enumerate() {
            out[14 + 7 * i..21 + 7 * i].copy_from_slice(&d.to_field(i + 1 == n));
        }
        let at = 14 + 7 * n;
        out[at] = 0x03;
        out[at + 1] = 0xF0;
        out[at + 2..].copy_from_slice(self.info);
        Some(out.len())
    }
}

impl Frame<'_> {
    /// The TNC2 text (as `Display` writes it) as bytes, without `core::fmt`.
    pub fn emit_tnc2(&self, mut out: impl FnMut(u8)) {
        self.src.emit(&mut out);
        out(b'>');
        self.dest.emit(&mut out);
        for d in self.digis() {
            out(b',');
            d.emit(&mut out);
            if d.repeated {
                out(b'*');
            }
        }
        out(b':');
        for &c in self.info {
            if (0x20..0x7F).contains(&c) {
                out(c);
            } else {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                for x in [b'<', b'0', b'x', HEX[(c >> 4) as usize], HEX[(c & 15) as usize], b'>'] {
                    out(x);
                }
            }
        }
    }
}

/// TNC2 monitor format: `SRC>DEST,DIGI1*,DIGI2:info`: printable bytes of the information field as they are,
/// others as `<0xNN>`, as Dire Wolf writes them.
impl fmt::Display for Frame<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}>{}", self.src, self.dest)?;
        for d in self.digis() {
            write!(f, ",{d}{}", if d.repeated { "*" } else { "" })?;
        }
        f.write_str(":")?;
        for &c in self.info {
            if (0x20..0x7F).contains(&c) {
                fmt::Write::write_char(f, c as char)?;
            } else {
                write!(f, "<0x{c:02x}>")?;
            }
        }
        Ok(())
    }
}

/// A TNC2 header split from its information field, without parsing the addresses (third-party traffic and
/// APRS-IS lines may carry names that are not AX.25 callsigns, such as `qAR` or `TCPIP*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tnc2<'a> {
    pub src: &'a [u8],
    pub dest: &'a [u8],
    /// Comma-separated path, possibly empty.
    pub path: &'a [u8],
    pub info: &'a [u8],
}

impl<'a> Tnc2<'a> {
    /// Splits `SRC>DEST[,PATH]:info`.
    pub fn split(line: &'a [u8]) -> Option<Self> {
        let colon = line.iter().position(|&c| c == b':')?;
        let (head, info) = (&line[..colon], &line[colon + 1..]);
        let gt = head.iter().position(|&c| c == b'>')?;
        let (src, rest) = (&head[..gt], &head[gt + 1..]);
        let (dest, path) = match rest.iter().position(|&c| c == b',') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, &rest[rest.len()..]),
        };
        if src.is_empty() || dest.is_empty() {
            return None;
        }
        Some(Tnc2 { src, dest, path, info })
    }
}
