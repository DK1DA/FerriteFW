//! TERM-34 (decision 139, a bench test): `txtest suite`, every transmit baseband test in one PTT key-up, for one SDR
//! capture that `tools/sdrsuite.py` measures whole. A fixed, deterministic list of [`Step`]s ([`STEPS`]); the Radio
//! task runs it from the key-up, each step for its time (counted from the key-up, so nothing drifts), and after the
//! last holds [`HOLD`] (a 1 kHz tone from DAC1) until the release. Each step names what modulates the carrier
//! ([`Src`]: DAC1 through the mic input, the BK4829's tone generator, or the sub-tone path alone), the TX audio
//! set-up ([`Filters`]) and REG_40's deviation field; the Radio task changes only what differs from the step before
//! and puts every register back at the unkey (`ferrite_radio::chip::suite_*`).
//!
//! **Sections** ([`SECTIONS`]): each starts with a marker so the analysis finds it without timing: [`MARKER_GAP_MS`]
//! of silence (carrier only), then [`MARKER_MS`] of a tone from DAC1 in `data` at [`MARKER_AMP`], at
//! [`marker_hz`]: 400 Hz for section 1, 50 Hz more for each after (700 Hz for 7). No other step plays a tone of
//! that length after that much silence (the end tones' 800 Hz comes after 1200 Hz, or lasts 100 ms).
//!
//! 1. The sweep in `data` (the 23 third-octave tones, [`SWEEP_TONE_MS`] each, [`SWEEP_GAP_MS`] after each).
//! 2. The same in `voice`.
//! 3. DAC1's AFSK flags in `data`: the APRS level 3 s, 200 ms of silence, then 50 % for 2 s.
//! 4. The BK4829's AFSK flags (`afsk-bk` at 100 %) for 2 s at each REG_40 value of [`REG40_STEPS`] in `voice`, then
//!    0x43C in `data` and in `flat` (REG_2B[2:0] 100 and 111), 200 ms of carrier between: REG_40 to deviation.
//! 5. The tone generator at stock's gain and REG_40: 1000 Hz for 1 s (the swell's attack and the steady level), then
//!    each end tone (TONE-10: Beep, Two-tone up, Two-tone down, Chirp, Quindar) with 300 ms between.
//! 6. Sub-tones alone (the mic path with DAC1 silent, `voice`): CTCSS 67.0, 100.0, 151.4 and 250.3 Hz, then DCS 023
//!    normal, 2 s each with 400 ms between, at the calibration's gains (byte 1 CTCSS, byte 2 DCS: TX audit F7).
//! 7. 4FSK in `data` at 60 % for 3 s.

use crate::tonegen::END_TONES;
use crate::txtest::{Afsk, Filters, Fsk4, Gen, Sweep, Tone, MARKER_HZ};

/// REG_40's low 12 bits at every key-up (stock's 0x50A, `bk4829::tx::TX_DEVIATION`; checked in ferrite-radio's
/// tests): section 5 and every step that doesn't name another.
pub const REG40_STOCK: u16 = 0x050A;
/// Section 4's REG_40 values, low to stock's (measured so far: 0x387 1660 Hz, 0x50A 4518 Hz).
pub const REG40_STEPS: [u16; 6] = [0x300, 0x387, 0x3E0, 0x43C, 0x4A0, 0x50A];
/// The one section 4 repeats in `data` and `flat`.
pub const REG40_FILTERS: u16 = 0x43C;

/// The section marker: silence, then the tone, ms; its peak (full scale 32767: about 2.3 kHz in `data`).
pub const MARKER_GAP_MS: u16 = 300;
pub const MARKER_MS: u16 = 150;
pub const MARKER_AMP: i32 = 3000;
/// Silence before the first marker (the key-up's ramp, the SDR's settling), and before [`HOLD`].
pub const LEAD_MS: u16 = 1000;
pub const TAIL_MS: u16 = 300;
/// The sweep's tones and the silence after each, ms (the analysis needs at least 10 cycles of 50 Hz).
pub const SWEEP_TONE_MS: u32 = 250;
pub const SWEEP_GAP_MS: u32 = 50;
/// The sweep step: one pass and 60 ms more of silence.
pub const SWEEP_STEP_MS: u16 = (crate::txtest::SWEEP_HZ.len() as u32 * (SWEEP_TONE_MS + SWEEP_GAP_MS) + 60) as u16;
/// The CTCSS tones of section 6, 0.1 Hz, and its DCS code (octal digits' value: 023, normal).
pub const CTCSS_DHZ: [u16; 4] = [670, 1000, 1514, 2503];
pub const DCS_CODE: u16 = 0o23;

/// Section `n`'s marker (1 to 7): 350 + 50 n Hz.
pub const fn marker_hz(n: u8) -> u16 {
    350 + 50 * n as u16
}

/// What DAC1 plays (the chip on the mic input).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dac {
    Silence,
    /// A marker (or the hold) at [`MARKER_AMP`].
    Tone(u16),
    /// The sweep, once ([`Sweep::once`]).
    Sweep,
    /// AFSK flags at this percent of the APRS level.
    Afsk(u8),
    /// 4FSK at this percent of [`crate::txtest::FSK_AMP`].
    Fsk4(u8),
}

impl Dac {
    /// The generator the DSP task plays, from its start.
    pub fn gen(self) -> Gen {
        match self {
            Dac::Silence => Gen::Tone(Tone::new(0, 0)),
            Dac::Tone(hz) => Gen::Tone(Tone::new(hz, MARKER_AMP)),
            Dac::Sweep => Gen::Sweep(Sweep::once(SWEEP_TONE_MS, SWEEP_GAP_MS)),
            Dac::Afsk(l) => Gen::Afsk(Afsk::new(l)),
            Dac::Fsk4(l) => Gen::Fsk4(Fsk4::new(l)),
        }
    }
}

/// What modulates the carrier during a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Src {
    /// DAC1 through the mic input (REG_30 0xC1FE).
    Dac(Dac),
    /// The tone generator's AFSK flags, `afsk-bk` at 100 % (REG_30 0xC3FA; TMR5's bit clock).
    BkFlags,
    /// The tone generator: one tone at stock's gain (`bk4829::tx::TONE_TX_GAIN`), as an end tone goes.
    BkTone(u16),
    /// Tone transmit with the tones off: carrier only, between the generator's steps.
    BkQuiet,
    /// The sub-tone path alone, on the mic path with DAC1 silent: the tone word (CHAN-3: 0.1 Hz CTCSS, bit 15 DCS).
    Sub(u16),
}

impl Src {
    /// The chip in tone transmit (the mic ADC off) rather than on the mic.
    pub const fn tones(self) -> bool {
        matches!(self, Src::BkFlags | Src::BkTone(_) | Src::BkQuiet)
    }

    /// What DAC1 plays meanwhile: silence unless the step is DAC1's.
    pub const fn dac(self) -> Dac {
        match self {
            Src::Dac(d) => d,
            _ => Dac::Silence,
        }
    }

    /// The sub-tone sent (0: none).
    pub const fn subtone(self) -> u16 {
        match self {
            Src::Sub(t) => t,
            _ => 0,
        }
    }
}

/// One step: what modulates, for how long, with which TX audio set-up and REG_40 field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub ms: u16,
    pub src: Src,
    pub filters: Filters,
    /// REG_40[11:0] (the top four bits stay as the key-up found them).
    pub reg40: u16,
}

const fn st(ms: u16, src: Src, filters: Filters, reg40: u16) -> Step {
    Step { ms, src, filters, reg40 }
}

/// A DAC1 step at stock's REG_40.
const fn dac(ms: u16, d: Dac, f: Filters) -> Step {
    st(ms, Src::Dac(d), f, REG40_STOCK)
}

/// A tone generator step in `voice` at stock's REG_40.
const fn bk(ms: u16, src: Src) -> Step {
    st(ms, src, Filters::Voice, REG40_STOCK)
}

const D: Filters = Filters::Data;
const V: Filters = Filters::Voice;
const SIL: Dac = Dac::Silence;

/// The marker's silence and tone for section `n`.
const fn gap(_n: u8) -> Step {
    dac(MARKER_GAP_MS, SIL, D)
}
const fn mark(n: u8) -> Step {
    dac(MARKER_MS, Dac::Tone(marker_hz(n)), D)
}

/// Section 4's burst and the carrier after it.
const fn flags(reg40: u16, f: Filters) -> Step {
    st(2000, Src::BkFlags, f, reg40)
}
const fn quiet(reg40: u16, f: Filters) -> Step {
    st(200, Src::BkQuiet, f, reg40)
}

/// End tone `k`'s step `j` (TONE-10's list, as the unkey plays them).
const fn end(k: usize, j: usize) -> Step {
    bk(END_TONES[k][j].1, Src::BkTone(END_TONES[k][j].0))
}

/// The suite, in order. A `static`: the Radio task reads it (fast flash).
pub static STEPS: [Step; 64] = [
    dac(LEAD_MS, SIL, D),
    // 1: the sweep in data.
    gap(1),
    mark(1),
    dac(SWEEP_STEP_MS, Dac::Sweep, D),
    // 2: the sweep in voice.
    gap(2),
    mark(2),
    dac(SWEEP_STEP_MS, Dac::Sweep, V),
    // 3: DAC1's AFSK flags in data, at the APRS level, then half.
    gap(3),
    mark(3),
    dac(3000, Dac::Afsk(100), D),
    dac(200, SIL, D),
    dac(2000, Dac::Afsk(50), D),
    // 4: the tone generator's flags per REG_40, then the filters.
    gap(4),
    mark(4),
    flags(REG40_STEPS[0], V),
    quiet(REG40_STEPS[0], V),
    flags(REG40_STEPS[1], V),
    quiet(REG40_STEPS[1], V),
    flags(REG40_STEPS[2], V),
    quiet(REG40_STEPS[2], V),
    flags(REG40_STEPS[3], V),
    quiet(REG40_STEPS[3], V),
    flags(REG40_STEPS[4], V),
    quiet(REG40_STEPS[4], V),
    flags(REG40_STEPS[5], V),
    quiet(REG40_STEPS[5], V),
    flags(REG40_FILTERS, D),
    quiet(REG40_FILTERS, D),
    flags(REG40_FILTERS, Filters::Flat),
    quiet(REG40_FILTERS, Filters::Flat),
    // 5: the tone generator: a steady 1 kHz, then the end tones.
    gap(5),
    mark(5),
    bk(1000, Src::BkTone(1000)),
    bk(300, Src::BkQuiet),
    end(1, 0),
    bk(300, Src::BkQuiet),
    end(2, 0),
    end(2, 1),
    bk(300, Src::BkQuiet),
    end(3, 0),
    end(3, 1),
    bk(300, Src::BkQuiet),
    end(4, 0),
    end(4, 1),
    end(4, 2),
    bk(300, Src::BkQuiet),
    end(5, 0),
    bk(300, Src::BkQuiet),
    // 6: sub-tones alone, voice.
    gap(6),
    mark(6),
    bk(2000, Src::Sub(CTCSS_DHZ[0])),
    dac(400, SIL, V),
    bk(2000, Src::Sub(CTCSS_DHZ[1])),
    dac(400, SIL, V),
    bk(2000, Src::Sub(CTCSS_DHZ[2])),
    dac(400, SIL, V),
    bk(2000, Src::Sub(CTCSS_DHZ[3])),
    dac(400, SIL, V),
    bk(2000, Src::Sub(0x8000 | DCS_CODE)),
    dac(400, SIL, V),
    // 7: 4FSK in data at 60 %.
    gap(7),
    mark(7),
    dac(3000, Dac::Fsk4(60), D),
    dac(TAIL_MS, SIL, D),
];

/// The steps.
pub const LEN: usize = STEPS.len();

/// After the last step, until the release: a 1 kHz tone from DAC1 in `data` (the end mark).
/// A `static`: the Radio task reads it (fast flash).
pub static HOLD: Step = dac(0, Dac::Tone(MARKER_HZ), D);

/// The suite's length to the hold, ms.
pub const SUITE_MS: u32 = {
    let mut t = 0;
    let mut i = 0;
    while i < LEN {
        t += STEPS[i].ms as u32;
        i += 1;
    }
    t
};

/// Each section's first step (its marker's silence), in [`STEPS`]; and its name for the terminal and the manual.
pub static SECTION_START: [u8; 7] = [1, 4, 7, 12, 30, 48, 60];
pub const SECTION_NAMES: [&str; 7] = [
    "sweep, data",
    "sweep, voice",
    "AFSK from DAC1, data, 100 % then 50 %",
    "AFSK from the tone generator per REG_40, then data and flat",
    "tone generator 1 kHz, then the end tones",
    "sub-tones: CTCSS 67.0, 100.0, 151.4, 250.3 Hz, DCS 023",
    "4FSK, data, 60 %",
];

/// The section (1 to 7) step `i` belongs to; 0 the lead-in.
pub fn section(i: usize) -> u8 {
    SECTION_START.iter().rposition(|&s| i >= s as usize).map_or(0, |k| k as u8 + 1)
}

/// The TX timeout (TX-30, s) the suite needs to finish: its length with the hold's first seconds and the UI's
/// margin, rounded up to the setting's 30 s steps (90 s).
pub const TIMEOUT_MIN_S: u16 = (SUITE_MS.div_ceil(1000) as u16 + 5).div_ceil(30) * 30;

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec;

    /// TERM-34: the table: seven sections in order, each opening with its marker (300 ms of silence in `data`, then
    /// 150 ms of 350 + 50 n Hz), the markers distinct and no other step a marker-like tone; the total 59.95 s, the
    /// timeout it needs 90 s.
    #[test]
    fn table_and_markers() {
        assert_eq!(LEN, 64);
        assert_eq!(SUITE_MS, 59_950);
        assert_eq!(TIMEOUT_MIN_S, 90);
        assert_eq!(SWEEP_STEP_MS, 6960);
        assert_eq!(STEPS[0], dac(LEAD_MS, Dac::Silence, Filters::Data));
        for n in 1..=7u8 {
            let s = SECTION_START[n as usize - 1] as usize;
            assert_eq!(STEPS[s], dac(300, Dac::Silence, Filters::Data), "section {n}");
            assert_eq!(STEPS[s + 1], dac(150, Dac::Tone(marker_hz(n)), Filters::Data), "section {n}");
            assert_eq!(section(s), n);
            assert_eq!(section(s - 1), n - 1);
        }
        assert_eq!((marker_hz(1), marker_hz(7)), (400, 700));
        // Markers only where the sections start: no other DAC tone, and every generator tone in 400-700 Hz shorter
        // than a marker or not after silence.
        let starts: Vec<usize> = SECTION_START.iter().map(|&s| s as usize + 1).collect();
        for (i, s) in STEPS[..LEN].iter().enumerate() {
            if let Src::Dac(Dac::Tone(hz)) = s.src {
                assert!(starts.contains(&i), "step {i}: a DAC tone {hz} Hz that is no marker");
            }
            if let Src::BkTone(hz) = s.src {
                assert!(!(400..=700).contains(&hz) || s.ms < 120, "step {i}");
            }
            assert!(s.ms > 0, "step {i}");
            assert!(s.reg40 <= 0xFFF);
        }
        // Section by section, the contents.
        let sec = |n: u8| -> Vec<Step> {
            let a = SECTION_START[n as usize - 1] as usize + 2;
            let b = if n == 7 { LEN } else { SECTION_START[n as usize] as usize };
            STEPS[a..b].to_vec()
        };
        assert_eq!(sec(1), [dac(6960, Dac::Sweep, Filters::Data)]);
        assert_eq!(sec(2), [dac(6960, Dac::Sweep, Filters::Voice)]);
        assert_eq!(sec(3).iter().map(|s| (s.ms, s.src)).collect::<Vec<_>>(), [(3000, Src::Dac(Dac::Afsk(100))), (200, Src::Dac(Dac::Silence)), (2000, Src::Dac(Dac::Afsk(50)))]);
        let s4 = sec(4);
        assert_eq!(s4.len(), 16);
        let bursts: Vec<(u16, Filters)> = s4.iter().filter(|s| s.src == Src::BkFlags).map(|s| (s.reg40, s.filters)).collect();
        assert_eq!(bursts, [(0x300, V), (0x387, V), (0x3E0, V), (0x43C, V), (0x4A0, V), (0x50A, V), (0x43C, D), (0x43C, Filters::Flat)]);
        assert!(s4.chunks(2).all(|c| c[0].ms == 2000 && c[1] == quiet(c[0].reg40, c[0].filters)));
        let tones: Vec<(u16, u16)> = sec(5).iter().filter_map(|s| if let Src::BkTone(hz) = s.src { Some((hz, s.ms)) } else { None }).collect();
        assert_eq!(tones, [(1000, 1000), (1000, 150), (800, 100), (1200, 100), (1200, 100), (800, 100), (1000, 60), (1250, 60), (1500, 60), (2475, 250)]);
        assert!(sec(5).iter().all(|s| s.reg40 == REG40_STOCK && s.filters == V && s.src.tones()));
        let subs: Vec<u16> = sec(6).iter().map(|s| s.src.subtone()).filter(|&t| t != 0).collect();
        assert_eq!(subs, [670, 1000, 1514, 2503, 0x8013]);
        assert!(sec(6).iter().all(|s| s.filters == V && !s.src.tones() && s.src.dac() == Dac::Silence));
        assert_eq!(sec(7), [dac(3000, Dac::Fsk4(60), D), dac(TAIL_MS, Dac::Silence, D)]);
        // Every step not in section 4 at stock's REG_40, so the field is back before the next marker.
        assert!(STEPS[..LEN].iter().enumerate().all(|(i, s)| section(i) == 4 || s.reg40 == REG40_STOCK));
        assert_eq!(HOLD.src, Src::Dac(Dac::Tone(1000)));
        assert_eq!(REG40_STOCK, 0x50A);
        // The 1 kHz hold and the markers' and silences' generators.
        let mut g = Dac::Tone(1000).gen();
        let x: Vec<i16> = (0..48_000).map(|_| g.sample()).collect();
        assert_eq!(x.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count(), 999); // the first cycle starts at sample 0
        assert!((x.iter().map(|v| v.unsigned_abs() as i32).max().unwrap() - MARKER_AMP).abs() < 10);
        let mut g = Dac::Silence.gen();
        assert!((0..4800).all(|_| g.sample() == 0));
    }

    /// TERM-34: the suite's sweep is the 23 tones alone, 250 ms each with 50 ms after, once, then silence (no marker,
    /// no loop); its first tone starts at sample 0.
    #[test]
    fn suite_sweep_once() {
        let s = Sweep::once(SWEEP_TONE_MS, SWEEP_GAP_MS);
        assert_eq!(s.pass(), 48 * 300 * 23);
        assert_eq!(s.tone_at(0), Some(50));
        assert_eq!(s.tone_at(48 * 250), None);
        assert_eq!(s.tone_at(48 * 300), Some(63));
        assert_eq!(s.tone_at(48 * (22 * 300 + 249)), Some(8000));
        assert_eq!(s.tone_at(s.pass()), None);
        let mut g = Dac::Sweep.gen();
        let n = (SWEEP_STEP_MS as usize + 500) * 48;
        let x: Vec<i16> = (0..n).map(|_| g.sample()).collect();
        assert!(x[48 * 6900..].iter().all(|&v| v == 0), "silent after the pass");
        // The 1 kHz tone (index 13): 250 cycles, the first starting at sample 0 (249 crossings inside).
        let t = &x[48 * 300 * 13..48 * (300 * 13 + 250)];
        assert_eq!(t.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count(), 249);
    }
}
