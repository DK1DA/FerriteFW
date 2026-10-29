//! TERM-45 (decision 318, a bench test): 4FSK from the BK4829's tone generator, with the microphone off. The mic
//! research (`docs/research/mic-audio.md`) found that the idle chip can digitise the mic only while PC12 keeps the
//! mic on the shared mic line, so the transmitting chip can't take M17's 4FSK from DAC1 at the same time. This test
//! makes the 4FSK inside the keyed chip instead: tone transmit (REG_30 0xC3FA: the mic ADC off), the transmit
//! filters and DC blocks bypassed, tone 1 frozen at a non-zero phase so its output is a constant, and that
//! constant's size stepped per symbol through tone 1's gain (REG_70 bits 14:8). The frozen output is one-signed, so
//! the carrier is first moved down by `shift_hz` and the four gains straddle it.
//!
//! - `cal`: holds the gains 0, 16, ... 112, 127 in turn, so a capture maps gain to frequency.
//! - otherwise: [`PREAMBLE_SYMBOLS`] of +3/-3, then PRBS9 dibits (M17's map, as `txtest 4fsk`), each symbol's gain
//!   reached in `sub` steps on a smoothstep curve.

use crate::txtest::{level, Prbs9, PREAMBLE_SYMBOLS, SYMBOL_HZ};

/// A run's settings (`synthfsk ...`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cfg {
    pub cal: bool,
    /// The run's length, ms.
    pub ms: u16,
    /// Gain steps per symbol (1-16).
    pub sub: u8,
    /// The carrier moved down by this much for the run, Hz (10 Hz steps).
    pub shift_hz: u16,
    /// REG_40's deviation scale for the run (its low 12 bits).
    pub dev: u16,
    /// Tone 1's gain for the levels -3, -1, +1, +3.
    pub g: [u8; 4],
}

/// What the run did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub steps: u32,
    /// The longest gain write, and the average, in tenths of a microsecond.
    pub write_max_x10: u16,
    pub write_avg_x10: u16,
    /// Steps that started late by more than a quarter step, and the worst lateness, microseconds.
    pub late: u32,
    pub late_max_us: u16,
    /// The carrier before the run (Hz / 10), for the record.
    pub freq_word: u32,
}

/// The gains `cal` holds, each for a ninth of the run.
pub static CAL_GAINS: [u8; 9] = [0, 16, 32, 48, 64, 80, 96, 112, 127];
pub const SUB_MAX: u8 = 16;

/// The gain for each step, in order.
pub struct Steps {
    cfg: Cfg,
    prbs: Prbs9,
    n: u32,
    /// The symbol index and the sub-step within it.
    sym: u32,
    k: u8,
    prev: u8,
    cur: u8,
}

impl Steps {
    pub fn new(cfg: Cfg) -> Self {
        let sub = cfg.sub.clamp(1, SUB_MAX);
        let mut s = Steps { cfg: Cfg { sub, ..cfg }, prbs: Prbs9::default(), n: 0, sym: 0, k: 0, prev: 0, cur: 0 };
        s.cur = s.symbol_gain();
        s.prev = s.cur;
        s
    }

    /// Steps a second.
    pub fn rate(&self) -> u32 {
        SYMBOL_HZ * self.cfg.sub as u32
    }

    /// Steps in the run.
    pub fn total(&self) -> u32 {
        self.rate() * self.cfg.ms as u32 / 1000
    }

    fn symbol_gain(&mut self) -> u8 {
        let s = if self.sym < PREAMBLE_SYMBOLS {
            if self.sym.is_multiple_of(2) { 3 } else { -3 }
        } else {
            let hi = self.prbs.bit();
            level(hi, self.prbs.bit())
        };
        self.cfg.g[((s + 3) / 2) as usize]
    }

    /// The next step's gain, or None at the end of the run.
    pub fn next_gain(&mut self) -> Option<u8> {
        if self.n >= self.total() {
            return None;
        }
        self.n += 1;
        if self.cfg.cal {
            let i = ((self.n - 1) as u64 * CAL_GAINS.len() as u64 / self.total() as u64) as usize;
            return Some(CAL_GAINS[i.min(CAL_GAINS.len() - 1)]);
        }
        let sub = self.cfg.sub as u32;
        // smoothstep from the last symbol's gain to this one's, ending on it at the last sub-step
        let x = (self.k as u32 + 1) * 1000 / sub;
        let w = x * x * (3000 - 2 * x) / 1_000_000; // 0..1000
        let g = (self.prev as i32 + (self.cur as i32 - self.prev as i32) * w as i32 / 1000) as u8;
        self.k += 1;
        if self.k as u32 == sub {
            self.k = 0;
            self.sym += 1;
            self.prev = self.cur;
            self.cur = self.symbol_gain();
        }
        Some(g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(sub: u8) -> Cfg {
        Cfg { cal: false, ms: 100, sub, shift_hz: 2400, dev: 0x50A, g: [10, 40, 70, 100] }
    }

    /// TERM-45: the preamble alternates +3/-3 (gains 100 and 10), each symbol ends on its own gain, and the run
    /// stops after `ms` of steps.
    #[test]
    fn steps_follow_the_symbols() {
        let mut s = Steps::new(cfg(4));
        assert_eq!(s.rate(), 19_200);
        let g: std::vec::Vec<u8> = core::iter::from_fn(|| s.next_gain()).collect();
        assert_eq!(g.len(), 1920);
        // symbol 0 is +3 from the start; symbol 1 (-3) ramps 100 -> 10 and ends on 10
        assert_eq!(&g[0..4], &[100, 100, 100, 100]);
        assert_eq!(g[7], 10);
        assert!(g[4] > g[5] && g[5] > g[6] && g[6] > g[7], "{:?}", &g[4..8]);
        // after the preamble only the four gains end a symbol
        for i in (PREAMBLE_SYMBOLS as usize..480).map(|i| i * 4 + 3) {
            assert!([10, 40, 70, 100].contains(&g[i]), "{i}: {}", g[i]);
        }
    }

    /// TERM-45: `cal` holds each of [`CAL_GAINS`] for a ninth of the run.
    #[test]
    fn cal_holds_each_gain() {
        let mut s = Steps::new(Cfg { cal: true, ms: 900, ..cfg(1) });
        let g: std::vec::Vec<u8> = core::iter::from_fn(|| s.next_gain()).collect();
        assert_eq!(g.len(), 4320);
        for (i, &c) in CAL_GAINS.iter().enumerate() {
            assert!(g[i * 480..(i + 1) * 480].iter().all(|&x| x == c), "{c}");
        }
    }
}
