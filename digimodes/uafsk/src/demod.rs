//! Bell 202 AFSK demodulator: 1200 Hz (mark) and 2200 Hz (space) at 1200 bit/s, to HDLC frames. Integer only.
//!
//! [`crate::fsk::Fsk`] turns audio into bits (high-pass, correlators, per-tone normalisation, several slicers, a
//! clock each; after Dire Wolf); here each slicer's bits go through NRZI (same tone as the previous bit is a 1, a
//! change is a 0) into its own [`crate::hdlc::Deframer`], and a frame found by more than one slicer is reported
//! once. Each decision's margin goes along, so a frame that fails its FCS can be repaired
//! ([`Config::repair`]).
//!
//! **FX.25** ([`Config::fx25`], on by default): the same bits also go to an FX.25 tag hunter per slicer
//! ([`ufx25::Hunter`], 16 bytes each), and the slicers share one block buffer ([`Fx25Block`]): the first slicer to
//! find a tag claims it and collects the Reed-Solomon block behind it (a lower slicer that finds the same tag within
//! 16 bits takes over: measured as good as decoding every slicer's block). Correcting a block costs 0.27 to 1.03 M
//! instructions (1.7 to 6.4 ms on a Cortex-M4 at 240 MHz), too much for an audio path's sample budget, so the
//! demodulator only collects; the complete block waits in [`Demodulator::pending_fx25`] until the caller
//! releases it ([`Demodulator::release_fx25`]; or copies it out with [`Demodulator::take_fx25`]) and corrects it
//! where time allows ([`Fx25Block::decode`]: another task), or here with [`Demodulator::decode_fx25`]. A tag found
//! while the buffer is busy with another block is skipped ([`Demodulator::fx25_skipped`]). A clean FX.25
//! transmission is an ordinary AX.25 frame too: the plain copy comes out of [`Demodulator::push`] first, and the
//! block carries its FCS so the corrected copy is marked a duplicate.
//!
//! **Misses** ([`Demodulator::take_miss`]): a frame that ended on a flag but failed its FCS (and any repair) on
//! every slicer that framed it, while no slicer decoded it, is reported once per packet as a [`Miss`], with its
//! first bytes: what was heard but not decoded, for counting and logging.
//!
//! The type sets the cost: `Demodulator<S, C>` has `S` slicers (1 to 4; the fourth compares raw amplitudes) per chain and `C` chains (1 or 2). A
//! second chain runs without the high-pass: a little better on flat (discriminator) audio, for about twice the RAM.

pub use crate::fsk::Config;
use crate::fsk::{Fsk, BELL202};
use crate::hdlc::Deframer;
use ufx25::{Collector, Hunter, Mode, Tag, DEFAULT_TAG_ERRORS};

/// An FX.25 block as received (n bytes: information, then check bytes), to correct with [`Fx25Block::decode`].
/// 264 bytes; `Copy`, to hand to another task.
#[derive(Clone, Copy)]
pub struct Fx25Block {
    /// FCS of the plain AX.25 frame reported while this block came in, if any: the same frame, unless the FX.25
    /// copy is a different one.
    pub plain: Option<u16>,
    rx: Collector,
}

/// A frame recovered from an [`Fx25Block`].
#[derive(Debug)]
pub struct Fx25Frame<'a> {
    /// The AX.25 frame without its FCS.
    pub frame: &'a [u8],
    pub fcs: u16,
    /// Bytes the Reed-Solomon code corrected.
    pub corrected: u8,
    /// The code's check bytes: 16, 32 or 64.
    pub check: u8,
    /// The plain AX.25 deframer reported this frame already (it was intact): drop it, or count it as FX.25 heard.
    pub duplicate: bool,
}

/// Bytes of a [`Miss`] kept: the longest address field (10 addresses, 70 bytes), control and PID.
pub const MISS_HEAD: usize = 72;

/// A frame heard but not decoded ([`Demodulator::take_miss`]): every slicer that framed it failed its FCS, and
/// none decoded it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Miss {
    /// Bytes between the flags, FCS included.
    pub len: u16,
    /// The address field reads as AX.25 (2 to 10 callsigns): a packet that failed, not noise that happened to
    /// fall between two flags.
    pub addresses: bool,
    /// An FX.25 tag came in with it: its FX.25 copy may still be recovered.
    pub fx25: bool,
    /// The first bytes (up to [`MISS_HEAD`]; `len` of them if shorter).
    pub head: [u8; MISS_HEAD],
}

impl Miss {
    /// The best guess at its addresses as TNC2 text, `SOURCE>DEST,PATH`, a `?` for each character that isn't a
    /// callsign's (bit errors), at most 10 addresses (stops at the extension bit or the bytes kept).
    pub fn header(&self, mut out: impl FnMut(u8)) {
        fn put(a: &[u8], out: &mut impl FnMut(u8)) {
            for c in a[..6].iter().map(|c| c >> 1).filter(|&c| c != b' ') {
                out(if c.is_ascii_uppercase() || c.is_ascii_digit() { c } else { b'?' });
            }
            let ssid = a[6] >> 1 & 0x0F;
            if ssid != 0 {
                out(b'-');
                if ssid >= 10 {
                    out(b'1');
                }
                out(b'0' + ssid % 10);
            }
        }
        let n = (self.len as usize).min(MISS_HEAD) / 7;
        if n < 2 {
            return;
        }
        put(&self.head[7..14], &mut out);
        out(b'>');
        put(&self.head[..7], &mut out);
        let mut last = self.head[13] & 1 == 1;
        for i in 2..n {
            if last {
                break;
            }
            out(b',');
            put(&self.head[i * 7..i * 7 + 7], &mut out);
            last = self.head[i * 7 + 6] & 1 == 1;
        }
    }
}

impl Fx25Block {
    /// The code of the complete block, if it holds one.
    pub fn mode(&self) -> Option<Mode> {
        self.rx.mode()
    }

    /// Wrong bits in the tag.
    pub fn tag_errors(&self) -> u8 {
        self.rx.tag_errors()
    }

    /// The block as received (n bytes), or as corrected after [`Fx25Block::decode`].
    pub fn block(&self) -> &[u8] {
        self.rx.block()
    }

    /// Correct the block (in place) and find its frame: [`ufx25::decode`]. 0.27 to 1.03 M instructions on a
    /// Cortex-M4 (16 check bytes and no errors, to 64 and 32 errors), up to about 0.5 KB of stack. None: more
    /// wrong bytes than the code corrects, or no frame with a good FCS inside.
    pub fn decode(&mut self) -> Option<Fx25Frame<'_>> {
        let plain = self.plain;
        let check = self.rx.mode()?.check() as u8;
        let d = self.rx.decode()?;
        Some(Fx25Frame { frame: d.frame, fcs: d.fcs, corrected: d.corrected, check, duplicate: plain == Some(d.fcs) })
    }
}

/// Audio in, frames out. RAM on a Cortex-M4: 1,668 bytes as `Demodulator<3, 1>` (the default): three 330-byte
/// frame buffers, three 16-byte FX.25 tag hunters and one 264-byte FX.25 block; about 0.9 KB with one slicer, about
/// 3.0 KB with two chains.
pub struct Demodulator<const S: usize = 3, const C: usize = 1> {
    fsk: Fsk<S, C>,
    deframers: [[Deframer; S]; C],
    /// Each slicer's previous bit's tone, for NRZI.
    last_mark: [[bool; S]; C],
    /// FCS of the last frame reported and when, to drop the same frame from other slicers.
    last: Option<(u16, u32)>,
    age: u32,
    fx_on: bool,
    /// Samples per bit, for the FX.25 timings.
    spb: u16,
    hunters: [[Hunter; S]; C],
    /// The slicer (c·S + k) whose block [`Demodulator::fx`] is collecting.
    owner: u8,
    /// Tags skipped: the block buffer was busy with another block.
    skipped: u16,
    /// The one block buffer: collecting, or complete and waiting to be released.
    fx: Fx25Block,
    /// When any slicer last decoded a frame (duplicates too): a failure near it is that packet's, not a miss.
    good_at: u32,
    /// A miss being gathered from the slicers ([`MISS_BITS`]), and when.
    pend: Option<(Miss, u32)>,
    /// A miss whose window has passed, for [`Demodulator::take_miss`].
    ready: Option<Miss>,
}

/// The slicers frame the same packet within a few bits of each other: failures within this many bits are one
/// packet, and a frame decoded within it cancels them.
const MISS_BITS: u32 = 32;

/// A lower slicer that finds the same tag within this many bits of the claim takes the block over.
const TAKEOVER_BITS: u16 = 16;

impl<const S: usize, const C: usize> Demodulator<S, C> {
    /// `cfg.rate_hz` must give 8 to 40 samples per bit (9600 to 48000 samples/s).
    pub fn new(cfg: Config) -> Self {
        const { assert!(S * C <= 8, "at most 8 slicers in all") };
        Demodulator {
            fsk: Fsk::new(cfg, BELL202),
            deframers: core::array::from_fn(|_| {
                core::array::from_fn(|_| {
                    let mut d = Deframer::new();
                    d.set_repair(cfg.repair);
                    d
                })
            }),
            last_mark: [[false; S]; C],
            last: None,
            age: 0,
            fx_on: cfg.fx25,
            spb: (cfg.rate_hz / BELL202.baud) as u16,
            hunters: [[Hunter::new(); S]; C],
            owner: 0,
            skipped: 0,
            fx: Fx25Block { plain: None, rx: Collector::new() },
            good_at: 0,
            pend: None,
            ready: None,
        }
    }

    /// Feed one sample. `found` gets each new frame (without its FCS), once even if several slicers decode it.
    /// With FX.25 on, a block that has come in waits in [`Demodulator::pending_fx25`].
    pub fn push(&mut self, x: i16, mut found: impl FnMut(&[u8])) {
        self.age = self.age.wrapping_add(1);
        let window = MISS_BITS * self.spb as u32;
        if matches!(self.pend, Some((_, at)) if self.age.wrapping_sub(at) > window) {
            self.ready = self.pend.take().map(|(m, _)| m);
        }
        let Demodulator { fsk, deframers, last_mark, last, age, fx_on, spb, hunters, owner, skipped, fx, good_at, pend, .. } = self;
        fsk.push_soft(x, |i, mark, margin| {
            let (c, k) = (i / S, i % S);
            let bit = (mark == last_mark[c][k]) as u8;
            last_mark[c][k] = mark;
            if let Some((frame, fcs)) = deframers[c][k].push_soft(bit, margin.unsigned_abs()) {
                *good_at = *age;
                if matches!(*pend, Some((_, at)) if age.wrapping_sub(at) <= window) {
                    *pend = None; // another slicer had it
                }
                let dup = matches!(*last, Some((f, at)) if f == fcs && age.wrapping_sub(at) < 2000);
                if !dup {
                    *last = Some((fcs, *age));
                    found(frame);
                }
            }
            if let Some(b) = deframers[c][k].take_bad() {
                if age.wrapping_sub(*good_at) > window {
                    let tagged = fx.rx.collecting().is_some() || fx.rx.mode().is_some();
                    missed(b, tagged, *age, window, pend);
                }
            }
            if *fx_on {
                if let Some(tag) = hunters[c][k].push(bit, DEFAULT_TAG_ERRORS) {
                    claim(tag, i as u8, owner, skipped, fx);
                } else if *owner == i as u8 {
                    if let Some(mode) = fx.rx.push(bit) {
                        completed(mode, *last, *age, *spb, fx);
                    }
                }
            }
        });
    }

    /// The FX.25 block that came in, complete, for the caller to correct ([`Fx25Block::decode`]), here or in
    /// another task. It stays (and new tags are skipped) until [`Demodulator::release_fx25`].
    pub fn pending_fx25(&mut self) -> Option<&mut Fx25Block> {
        self.fx.rx.mode()?;
        Some(&mut self.fx)
    }

    /// Free the block buffer for the next tag.
    pub fn release_fx25(&mut self) {
        if self.fx.rx.mode().is_some() {
            self.fx.rx.clear();
        }
    }

    /// The FX.25 block that came in, copied out (264 bytes, to send to another task) and released; once.
    pub fn take_fx25(&mut self) -> Option<Fx25Block> {
        let b = *self.pending_fx25()?;
        self.release_fx25();
        Some(b)
    }

    /// Correct the block that came in, if any, here and now (0.27 to 1.03 M instructions), and release it: its
    /// frame unless the plain deframer reported it already. For callers with the time to spare; otherwise
    /// [`Demodulator::pending_fx25`].
    pub fn decode_fx25(&mut self, mut found: impl FnMut(&Fx25Frame)) {
        let Some(b) = self.pending_fx25() else { return };
        let mut fcs = None;
        if let Some(f) = b.decode() {
            if !f.duplicate {
                fcs = Some(f.fcs);
                found(&f);
            }
        }
        if let Some(f) = fcs {
            self.last = Some((f, self.age));
        }
        self.release_fx25();
    }

    /// A frame heard but not decoded ([`Miss`]), once, about [`MISS_BITS`] bits after it ended. Read it every block
    /// or so: a newer miss takes the place of one not read.
    pub fn take_miss(&mut self) -> Option<Miss> {
        self.ready.take()
    }

    /// FX.25 tags skipped since the start because the block buffer was busy (a block waiting to be released, or
    /// another transmission's block being collected); each slicer that found the tag counts.
    pub fn fx25_skipped(&self) -> u16 {
        self.skipped
    }

    /// Carrier detect: some slicer hears 1200-baud AFSK data, now. For channel access (do not transmit over a
    /// packet); combine with the radio's squelch so voice counts as busy too.
    pub fn busy(&self) -> bool {
        self.fsk.busy()
    }
}

/// Slicer `i` found a tag: it claims the block buffer if that is free (or takes over a block of the same code it
/// claimed less than [`TAKEOVER_BITS`] ago from a higher slicer); another slicer's copy of the tag in hand is
/// ignored, any other tag skipped. Out of line: runs once per tag.
#[cold]
#[inline(never)]
fn claim(tag: Tag, i: u8, owner: &mut u8, skipped: &mut u16, fx: &mut Fx25Block) {
    let free = fx.rx.collecting().is_none() && fx.rx.mode().is_none();
    let same = fx.rx.collecting() == Some(tag.mode) && fx.rx.bits() <= TAKEOVER_BITS;
    if free || (same && i < *owner) {
        fx.rx.start(tag);
        fx.plain = None;
        *owner = i;
    } else if !same {
        *skipped = skipped.wrapping_add(1);
    }
}

/// A slicer's frame failed its FCS, with no frame decoded near it: one packet's failures gather into one miss, the
/// likeliest packet kept (an AX.25 address field first, then the longest). Out of line: runs once per failure.
#[cold]
#[inline(never)]
fn missed(b: &[u8], fx25: bool, age: u32, window: u32, pend: &mut Option<(Miss, u32)>) {
    let mut m = Miss { len: b.len() as u16, addresses: crate::hdlc::ax25_addresses_ok(&b[..b.len() - 2]), fx25, head: [0; MISS_HEAD] };
    let n = b.len().min(MISS_HEAD);
    m.head[..n].copy_from_slice(&b[..n]);
    match pend {
        Some((p, at)) if age.wrapping_sub(*at) <= window => {
            if (m.addresses, m.len) > (p.addresses, p.len) {
                *p = Miss { fx25: m.fx25 || p.fx25, ..m };
            }
        }
        _ => *pend = Some((m, age)),
    }
}

/// The block is complete: note the plain copy, if the deframers got it (it ends inside the block, after the tag).
#[cold]
#[inline(never)]
fn completed(mode: Mode, last: Option<(u16, u32)>, age: u32, spb: u16, fx: &mut Fx25Block) {
    let span = (mode.n as u32 * 8 + 64 + 32) * spb as u32;
    fx.plain = last.filter(|&(_, at)| age.wrapping_sub(at) < span).map(|(f, _)| f);
}
