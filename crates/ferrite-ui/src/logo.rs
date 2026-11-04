//! The boot logo's artwork as an SPI part (decision 335, UPD-23): its two 112 px masks (the ring and the windings,
//! run-length encoded as `tools/mkassets.py` writes them) live on the SPI flash ([`ferrite_core::part::LOGO`]), not
//! in the MCU's. Without the part (missing, another build's, damaged) the boot screen shows the wordmark alone.
//!
//! Body: ring length u16, windings length u16, the ring's bytes, the windings' bytes.
//!
//! The masks are streamed a row at a time through a 256-byte window ([`Rows`]): no RAM kept, about 2.5 KB read per
//! drawing. Only the boot screen draws them (its animation, then the still logo).

use crate::font4::rle_row;
use ferrite_core::part;

/// The masks' size, px.
pub const SIZE: usize = 112;
/// Which mask: the ring or the windings.
pub const RING: usize = 0;
pub const WIND: usize = 1;

/// The body as built into a host build (the packer, tests and the simulators without parts): the header and the
/// two masks.
#[cfg(not(target_os = "none"))]
fn builtin() -> [&'static [u8]; 2] {
    [crate::assets::LOGO112_RING.bits, crate::assets::LOGO112_WIND.bits]
}

/// The part's body (host builds: the packer writes it).
#[cfg(feature = "std")]
pub fn body() -> std::vec::Vec<u8> {
    let [r, w] = builtin();
    let mut v = std::vec::Vec::new();
    v.extend_from_slice(&(r.len() as u16).to_le_bytes());
    v.extend_from_slice(&(w.len() as u16).to_le_bytes());
    v.extend_from_slice(r);
    v.extend_from_slice(w);
    v
}

/// Whether there is a logo to draw (a host with no reader set: the built-in masks).
pub fn present() -> bool {
    (cfg!(not(target_os = "none")) && !part::has_reader()) || part::ok(&part::LOGO)
}

/// A mask's rows in turn, from the part (or the built-in masks).
pub struct Rows {
    /// The mask's bytes in the body: start and end.
    start: u32,
    end: u32,
    /// The next row's byte, relative to `start`.
    pos: u32,
    /// The window: `n` bytes from `at` (relative to `start`).
    win: [u8; 256],
    at: u32,
    n: usize,
    #[cfg(not(target_os = "none"))]
    host: Option<&'static [u8]>,
}

impl Rows {
    /// Mask `which` ([`RING`] or [`WIND`]); with no part, rows read empty.
    pub fn new(which: usize) -> Rows {
        let mut r = Rows {
            start: 0,
            end: 0,
            pos: 0,
            win: [0; 256],
            at: 0,
            n: 0,
            #[cfg(not(target_os = "none"))]
            host: None,
        };
        #[cfg(not(target_os = "none"))]
        if !part::has_reader() {
            r.host = Some(builtin()[which & 1]);
            return r;
        }
        let mut h = [0u8; 4];
        if part::read_body(&part::LOGO, 0, &mut h) {
            let (lr, lw) = (u16::from_le_bytes([h[0], h[1]]) as u32, u16::from_le_bytes([h[2], h[3]]) as u32);
            (r.start, r.end) = if which == RING { (4, 4 + lr) } else { (4 + lr, 4 + lr + lw) };
        }
        r
    }

    /// The next row's pixels (0-15) into out[..SIZE].
    pub fn next_row(&mut self, out: &mut [u8]) {
        let w = SIZE.min(out.len());
        #[cfg(not(target_os = "none"))]
        if let Some(b) = self.host {
            let mut p = self.pos as usize;
            rle_row(b, &mut p, w, out);
            self.pos = p as u32;
            return;
        }
        let len = self.end - self.start;
        // A row is at most about 120 bytes: refill while less than 128 are left in the window.
        if self.at + (self.n as u32) < len.min(self.pos + 128) || self.pos < self.at {
            let n = (len - self.pos).min(self.win.len() as u32) as usize;
            let ok = part::read_body(&part::LOGO, self.start + self.pos, &mut self.win[..n]);
            (self.at, self.n) = (self.pos, if ok { n } else { 0 });
        }
        let mut p = (self.pos - self.at) as usize;
        rle_row(&self.win[..self.n], &mut p, w, out);
        self.pos = self.at + p as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{LOGO112_RING, LOGO112_WIND};
    use crate::font4::MaskRows;

    /// UPD-23: the streamed rows (here the built-in masks) are the masks' own rows.
    #[test]
    fn rows_are_the_masks() {
        for (which, m) in [(RING, &LOGO112_RING), (WIND, &LOGO112_WIND)] {
            let (mut a, mut b) = (Rows::new(which), MaskRows::new(m));
            for _ in 0..SIZE {
                let (mut x, mut y) = ([0u8; SIZE], [0u8; SIZE]);
                a.next_row(&mut x);
                b.next_row(&mut y);
                assert_eq!(x, y);
            }
        }
    }
}
