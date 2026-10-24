//! Two-tone FSK to bits: the front end of every modem here, tones and rate set by [`Tones`]. Integer only.
//!
//! The method follows Dire Wolf (WB2OSZ), in integers:
//! 1. **High-pass** (optional, on by default): two first-order sections at 1200 Hz. They lift the upper tone
//!    against the lower and remove hum and sub-audible tones; de-emphasised audio (a receiver's speaker output)
//!    needs it.
//! 2. **Correlate** each sample with oscillators at the two tones (in phase and quadrature), summed over a window
//!    of 1.4 bits by default ([`Config::window`]): each tone's amplitude over that window. A window longer than a
//!    bit narrows each tone's filter: on two off-air recordings 1.4 bits decodes 1.1 to 1.75 times the frames one
//!    bit does (most on steeply tilted audio), 1.3 to 1.5 are close, and past 1.6 the bits blur into each other.
//!    The ring keeps the samples, not the products; the products of the sample leaving the window are recomputed
//!    from the oscillators' phase a window ago.
//! 3. **Normalise each tone separately** with a peak and valley tracker (fast attack, slow decay), so twist (one
//!    tone louder than the other) matters less.
//! 4. **Slice**: mark (the lower tone) if its normalised amplitude is the larger. Several slicers weigh the upper
//!    tone differently (0 dB, -3.5 dB, +3.5 dB), each with its own clock. A fourth slicer, when `S` is 4, compares
//!    the raw amplitudes instead: better in white noise on flat audio, worse on twisted (de-emphasised) audio.
//! 5. **Recover bit timing** with a digital PLL: a phase accumulator advancing one turn per bit, pulled to 85% of
//!    its value at every tone change (74% before the longer window; 80 to 90% now do equally well), sampling
//!    half-way through the bit.
//!
//! [`Fsk::push_soft`] also gives each decision's margin, for the deframer's repair of frames that fail their FCS.
//!
//! Carrier detect ([`Fsk::busy`]) also needs both tones to come and go: a steady tone between them would otherwise
//! ripple into transitions that look like data at some frequencies.
//!
//! `Fsk<S, C>` has `S` slicers (1 to 4) per chain and `C` chains (1 or 2); a second chain runs without the
//! high-pass.

use udsp::trig::{cos_sin, phase_step};

/// Tone pair and bit rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tones {
    /// Lower tone (a slicer's "mark"), Hz.
    pub mark_hz: u32,
    /// Upper tone, Hz.
    pub space_hz: u32,
    pub baud: u32,
}

/// Bell 202: APRS and VHF packet.
pub const BELL202: Tones = Tones { mark_hz: 1200, space_hz: 2200, baud: 1200 };

/// Longest correlation window: 1.5 bits at 48 kHz.
const MAX_WIN: usize = 60;
/// Space gains of up to three slicers (Q8): 0 dB, -3.5 dB, +3.5 dB. A `static`, not a `const`: read per sample, and
/// a named table stays with the hot code on firmware that places unnamed constants in slower flash.
static GAINS: [i32; 3] = [256, 171, 383];
/// High-pass corner, Hz.
const HP_HZ: i64 = 1200;

/// Demodulator settings.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub rate_hz: u32,
    /// High-pass ahead of the (first chain's) correlators.
    pub highpass: bool,
    /// Correlation window, thousandths of a bit (rounded to whole samples): 1000 to 1500. Longer filters the
    /// tones more narrowly and blurs neighbouring bits more.
    pub window: u32,
    /// Repair a frame that fails its FCS by flipping some of its least confident tone decisions: how many to
    /// consider (0 = off, at most [`crate::hdlc::MAX_REPAIR`]). See [`crate::hdlc::Deframer::set_repair`]. Used by
    /// [`crate::demod::Demodulator`].
    pub repair: u8,
    /// Also receive FX.25 (forward error correction around AX.25; [`crate::demod`]): an FX.25 receiver beside
    /// each slicer's deframer. Used by [`crate::demod::Demodulator`].
    pub fx25: bool,
}

impl Config {
    /// High-pass on, a 1.4-bit window, repair over the 3 least confident decisions, FX.25 on.
    pub const fn new(rate_hz: u32) -> Self {
        Config { rate_hz, highpass: true, window: 1400, repair: 3, fx25: true }
    }
}

#[derive(Clone, Copy)]
struct Tracker {
    peak: i32,
    valley: i32,
}

impl Tracker {
    /// The tone comes and goes: its valley is under half its peak. A steady tone ripples a little in both
    /// correlators, and the normaliser stretches that ripple into transitions a slicer can mistake for data.
    fn keyed(&self) -> bool {
        self.valley * KEY_DIV < self.peak
    }

    /// Position of `a` between valley and peak, 0..=4096. Attack: 0.7 of the gap; decay: `decay` (Q16) of it per
    /// sample.
    #[inline(always)]
    fn update(&mut self, a: i32, decay: i32) -> i32 {
        if a > self.peak {
            self.peak += ((a - self.peak) * 179) >> 8;
        } else {
            self.peak -= (((self.peak - a) as i64 * decay as i64) >> 16) as i32;
        }
        if a < self.valley {
            self.valley -= ((self.valley - a) * 179) >> 8;
        } else {
            self.valley += (((a - self.valley) as i64 * decay as i64) >> 16) as i32;
        }
        let span = self.peak - self.valley;
        let d = a - self.valley;
        if span <= 0 {
            0
        } else if (-(1 << 19)..(1 << 19)).contains(&d) {
            // A 32-bit divide when the product fits (nearly always at 9600 to 12000 samples/s); the same result as the 64-bit one.
            (d << 12) / span
        } else {
            ((d as i64 * 4096) / span as i64) as i32
        }
    }
}

#[derive(Clone, Copy)]
struct Slicer {
    pll: i32,
    mark: bool,
    /// Carrier detect: the last 32 transitions, 1 where one fell close to where the clock expected it.
    score: u32,
    /// Bits since the last transition.
    quiet: u8,
    dcd: bool,
}

/// Carrier detect: a transition within 1/8 bit of the clock's expected edge is good. Noise lands there one time
/// in four; a packet nearly always.
const DCD_GOOD: i32 = 1 << 30;
/// On at 30 good of the last 32, off at 18 or fewer (hysteresis; as Dire Wolf does).
const DCD_ON: u32 = 30;
const DCD_OFF: u32 = 18;

/// [`Tracker::keyed`]: valley under peak / this.
const KEY_DIV: i32 = 2;

impl Slicer {
    fn transition(&mut self, good: bool) {
        self.score = self.score << 1 | good as u32;
        self.quiet = 0;
        self.update_dcd();
    }

    fn update_dcd(&mut self) {
        let n = self.score.count_ones();
        if n >= DCD_ON {
            self.dcd = true;
        } else if n <= DCD_OFF {
            self.dcd = false;
        }
    }
}

/// One filter, correlator and set of slicers.
struct Chain<const S: usize> {
    highpass: bool,
    hp: [(i32, i32); 2],
    /// Filtered samples over the last bit; the products of the one leaving the window are recomputed.
    ring: [i16; MAX_WIN],
    sums: [i32; 4],
    tm: Tracker,
    ts: Tracker,
    slicers: [Slicer; S],
}

impl<const S: usize> Chain<S> {
    fn new(highpass: bool) -> Self {
        Chain {
            highpass,
            hp: [(0, 0); 2],
            ring: [0; MAX_WIN],
            sums: [0; 4],
            tm: Tracker { peak: 0, valley: 0 },
            ts: Tracker { peak: 0, valley: 0 },
            slicers: [Slicer { pll: 0, mark: false, score: 0, quiet: 0, dcd: false }; S],
        }
    }
}

/// Audio in, bits out, from `S x C` slicers.
pub struct Fsk<const S: usize, const C: usize> {
    win: usize,
    pos: usize,
    phase_m: u32,
    phase_s: u32,
    step_m: u32,
    step_s: u32,
    /// Oscillator phase advance over one window: the phase of the sample leaving it is the current one minus this.
    back_m: u32,
    back_s: u32,
    decay: i32,
    pll_step: i32,
    hp_a: i32,
    chains: [Chain<S>; C],
}

impl<const S: usize, const C: usize> Fsk<S, C> {
    /// `cfg.rate_hz` must give 8 to 40 samples per bit; `cfg.window` 1000 to 1500.
    pub fn new(cfg: Config, tones: Tones) -> Self {
        assert!((1..=4).contains(&S) && (1..=2).contains(&C), "1 to 4 slicers, 1 or 2 chains");
        let rate = cfg.rate_hz;
        assert!((8..=40).contains(&((rate + tones.baud / 2) / tones.baud)), "rate must give 8 to 40 samples per bit");
        assert!((1000..=1500).contains(&cfg.window), "window must be 1000 to 1500 thousandths of a bit");
        let win = ((rate as u64 * cfg.window as u64 + tones.baud as u64 * 500) / (tones.baud as u64 * 1000)) as usize;
        let rate_mhz = rate as u64 * 1000;
        let (m, s) = (tones.mark_hz as u64 * 1000, tones.space_hz as u64 * 1000);
        Fsk {
            win,
            pos: 0,
            phase_m: 0,
            phase_s: 0,
            step_m: phase_step(m, rate_mhz),
            step_s: phase_step(s, rate_mhz),
            back_m: phase_step(m, rate_mhz).wrapping_mul(win as u32),
            back_s: phase_step(s, rate_mhz).wrapping_mul(win as u32),
            // Decay to about 1/e in 0.25 s.
            decay: (65536 * 4 / rate as i64).max(1) as i32,
            pll_step: phase_step(tones.baud as u64 * 1000, rate_mhz) as i32,
            // a = rate / (rate + 2 pi fc), Q15.
            hp_a: ((rate as i64 * 32768) / (rate as i64 + 2 * 314_159 * HP_HZ / 100_000)) as i32,
            chains: core::array::from_fn(|c| Chain::new(c == 0 && cfg.highpass)),
        }
    }

    /// Feed one sample. `bit` gets each slicer's decision at the middle of each bit: the slicer's index (chain x S
    /// + slicer) and whether it heard the lower tone.
    #[inline(always)]
    pub fn push(&mut self, x: i16, mut bit: impl FnMut(usize, bool)) {
        self.push_soft(x, |i, m, _| bit(i, m))
    }

    /// As [`Fsk::push`], with each decision's margin: how far the slicer was towards the lower tone (positive) or
    /// the upper (negative), normalised (about -4096 to 4096; the raw-amplitude slicer's is not normalised). Near
    /// zero is a decision the slicer was unsure of.
    #[inline(always)]
    pub fn push_soft(&mut self, x: i16, mut bit: impl FnMut(usize, bool, i32)) {
        let (cm, sm) = cos_sin((self.phase_m >> 20) as usize);
        let (cs, ss) = cos_sin((self.phase_s >> 20) as usize);
        let (cmo, smo) = cos_sin((self.phase_m.wrapping_sub(self.back_m) >> 20) as usize);
        let (cso, sso) = cos_sin((self.phase_s.wrapping_sub(self.back_s) >> 20) as usize);
        self.phase_m = self.phase_m.wrapping_add(self.step_m);
        self.phase_s = self.phase_s.wrapping_add(self.step_s);
        let pos = self.pos;
        self.pos = if self.pos + 1 == self.win { 0 } else { self.pos + 1 };
        for c in 0..C {
            let ch = &mut self.chains[c];
            let mut v = x as i32;
            if ch.highpass {
                for st in ch.hp.iter_mut() {
                    // y = a (y_prev + x - x_prev)
                    let y = ((self.hp_a as i64 * (st.1 + v - st.0) as i64) >> 15) as i32;
                    st.0 = v;
                    st.1 = y;
                    v = y;
                }
                v = v.clamp(-32768, 32767);
            }
            // Correlate over one bit: add this sample's products, take away those of the sample a bit ago.
            let o = ch.ring[pos] as i32;
            ch.ring[pos] = v as i16;
            ch.sums[0] += ((v * cm) >> 15) - ((o * cmo) >> 15);
            ch.sums[1] += ((v * sm) >> 15) - ((o * smo) >> 15);
            ch.sums[2] += ((v * cs) >> 15) - ((o * cso) >> 15);
            ch.sums[3] += ((v * ss) >> 15) - ((o * sso) >> 15);
            // Normalise each tone.
            let m = ch.tm.update(mag(ch.sums[0], ch.sums[1]), self.decay);
            let s = ch.ts.update(mag(ch.sums[2], ch.sums[3]), self.decay);
            // Slice and clock.
            for k in 0..S {
                let mark = if k == 3 {
                    mag(ch.sums[0], ch.sums[1]) > mag(ch.sums[2], ch.sums[3])
                } else {
                    m * 256 > s * GAINS[k]
                };
                let sl = &mut ch.slicers[k];
                let before = sl.pll;
                sl.pll = sl.pll.wrapping_add(self.pll_step);
                if mark != sl.mark {
                    // Where the clock was at this transition says whether it looks like data.
                    sl.transition(sl.pll.unsigned_abs() < DCD_GOOD as u32);
                    // pll x 85 / 100 without a 64-bit divide: pll = 20 q + r, so it is 17 q + 17 r / 20 exactly.
                    let (q, r) = (sl.pll / 20, sl.pll % 20);
                    sl.pll = q * 17 + r * 17 / 20;
                    sl.mark = mark;
                }
                if before > 0 && sl.pll < 0 {
                    // Bit stuffing means a packet never goes 7 bits without a transition; longer counts as noise
                    // (or a steady tone).
                    sl.quiet = sl.quiet.saturating_add(1);
                    if sl.quiet > 7 {
                        sl.score <<= 1;
                        sl.update_dcd();
                    }
                    // The margin is recomputed here, once a bit, rather than kept from the slicing above: cheaper.
                    let margin = if k == 3 {
                        mag(ch.sums[0], ch.sums[1]) - mag(ch.sums[2], ch.sums[3])
                    } else {
                        m * 256 - s * GAINS[k]
                    };
                    bit(c * S + k, mark, margin >> 8);
                }
            }
        }
    }

    /// Carrier detect: some slicer hears data at this rate, now.
    pub fn busy(&self) -> bool {
        self.chains.iter().any(|c| c.tm.keyed() && c.ts.keyed() && c.slicers.iter().any(|s| s.dcd))
    }
}

/// |(i, q)| approximated as max + 3/8 min (within 7%).
#[inline]
fn mag(i: i32, q: i32) -> i32 {
    let (a, b) = (i.abs(), q.abs());
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    hi + ((lo * 3) >> 3)
}
