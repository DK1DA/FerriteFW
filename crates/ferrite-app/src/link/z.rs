//! LINK-51 (decision 286): link compression, one shared block for the one session that asked first. Frames on the
//! owning port go through here: out, each frame's payload is coded with the radio-to-PC window and sent as a `Z`
//! frame (type 0x7D, 0xFD for an answer: the inner type, the frame count, the coded bytes) when that is shorter,
//! else plain; in, `Z` frames are decoded with the PC-to-radio window and handed on plain. Every frame either way,
//! plain or coded, goes into its direction's window and count, so a lost one shows as a count that doesn't match:
//! the frame is dropped, and the PC starts over with a `HELLO`, which resets both ends (as does every `HELLO` answer).

use crate::term::Sink;
use ferrite_core::link::{self as wire, MAX_PAYLOAD, MAX_WIRE};
use ferrite_core::linkz::{LzDec, LzEnc, DOWN, UP};

/// A `Z` frame's type (| 0x80 for an answer).
pub const Z: u8 = 0x7D;

/// The shared block: the windows, the counts, the frames being put together, and which session owns it.
pub struct Zip {
    enc: LzEnc<DOWN>,
    dec: LzDec<UP>,
    down_n: u8,
    up_n: u8,
    /// The port whose session owns it (LINK-51: first come, first served).
    pub owner: Option<usize>,
    out: [u8; MAX_WIRE],
    out_n: usize,
    inb: [u8; MAX_WIRE],
    in_n: usize,
}

impl Default for Zip {
    fn default() -> Self {
        Self::new()
    }
}

impl Zip {
    pub const fn new() -> Self {
        Zip { enc: LzEnc::new(), dec: LzDec::new(), down_n: 0, up_n: 0, owner: None, out: [0; MAX_WIRE], out_n: 0, inb: [0; MAX_WIRE], in_n: 0 }
    }

    /// Both windows and counts start over (a `HELLO` answer, a new owner, a release).
    pub fn reset(&mut self) {
        self.enc.reset();
        self.dec.reset();
        (self.down_n, self.up_n, self.out_n, self.in_n) = (0, 0, 0, 0);
    }

    /// One received byte from the owning port; `feed` gets the frames to hand on (plain), each with its closing 0.
    pub fn take(&mut self, b: u8, feed: &mut impl FnMut(u8)) {
        if b != 0 {
            if self.in_n < self.inb.len() {
                self.inb[self.in_n] = b;
                self.in_n += 1;
            } else {
                self.in_n = usize::MAX; // too long: dropped at its end
            }
            return;
        }
        let n = core::mem::take(&mut self.in_n);
        if n == 0 || n == usize::MAX {
            if n == 0 {
                feed(0); // a sync
            }
            return;
        }
        let mut raw = [0u8; MAX_WIRE];
        raw[..n].copy_from_slice(&self.inb[..n]);
        let Ok(f) = wire::parse(&mut self.inb[..n]) else {
            raw[..n].iter().for_each(|&c| feed(c)); // the session answers `bad frame`
            feed(0);
            return;
        };
        let (ty, seq) = (f.ty, f.seq);
        let mut plain = [0u8; MAX_PAYLOAD];
        let (ty, len) = if ty & 0x7F == Z {
            let (Some(&inner), Some(&cnt)) = (f.payload.first(), f.payload.get(1)) else { return };
            if cnt != self.up_n {
                return; // a frame lost on the way: the PC hears nothing and starts over
            }
            match self.dec.decode(&f.payload[2..], &mut plain) {
                Some(k) => (inner, k),
                None => return,
            }
        } else {
            self.dec.seen(f.payload);
            let k = f.payload.len();
            plain[..k].copy_from_slice(f.payload);
            (ty, k)
        };
        self.up_n = self.up_n.wrapping_add(1);
        let mut w = [0u8; MAX_WIRE];
        if let Some(k) = wire::build(ty, seq, &plain[..len], &mut w) {
            w[..k].iter().for_each(|&c| feed(c));
        }
    }

    /// One whole frame the owning session sent (with its 0), out to `sink`: coded when shorter, else plain.
    fn send(&mut self, sink: &mut impl Sink) {
        let n = core::mem::take(&mut self.out_n);
        let mut raw = [0u8; MAX_WIRE];
        raw[..n].copy_from_slice(&self.out[..n]);
        let Ok(f) = wire::parse(&mut self.out[..n - 1]) else {
            sink.put(&raw[..n]);
            return;
        };
        if f.ty == wire::msg::HELLO | wire::ANSWER {
            sink.put(&raw[..n]); // always plain: both ends start over after it
            self.reset();
            return;
        }
        let mut coded = [0u8; MAX_PAYLOAD];
        coded[0] = f.ty;
        coded[1] = self.down_n;
        let k = self.enc.encode(f.payload, &mut coded[2..]);
        self.down_n = self.down_n.wrapping_add(1);
        let mut w = [0u8; MAX_WIRE];
        match k.filter(|&k| k + 2 < f.payload.len()) {
            Some(k) => {
                if let Some(m) = wire::build(Z | (f.ty & wire::ANSWER), f.seq, &coded[..k + 2], &mut w) {
                    sink.put(&w[..m]);
                }
            }
            None => sink.put(&raw[..n]), // the window took it anyway, as the PC's will
        }
    }
}

/// The owning session's output, through the block.
pub struct ZOut<'a, S: Sink> {
    pub z: &'a mut Zip,
    pub inner: &'a mut S,
}

impl<S: Sink> Sink for ZOut<'_, S> {
    fn room(&self) -> usize {
        self.inner.room().saturating_sub(self.z.out_n)
    }

    fn put(&mut self, b: &[u8]) {
        for &c in b {
            if self.z.out_n < self.z.out.len() {
                self.z.out[self.z.out_n] = c;
                self.z.out_n += 1;
            }
            if c == 0 {
                self.z.send(self.inner);
            }
        }
    }
}
