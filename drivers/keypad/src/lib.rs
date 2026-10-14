//! # keypad
//!
//! Keys and an encoder for a radio's front panel. `no_std`, no allocation, a few dozen bytes of state.
//!
//! - [`Matrix`]: drive rows one at a time (open-drain, low), read columns (pull-ups, low = pressed). An optional
//!   *ground row* is read with no row driven: keys wired from a column straight to ground (the RT-950 Pro has
//!   four). Direct keys are single pins. Each key maps to a code (0-31) through the board's table; a scan is a
//!   32-bit mask.
//! - [`Keys`]: debouncing and events: press, release, long press, and repeat while held.
//! - [`Encoder`]: quadrature decoding, one step per detent.

#![no_std]
#![allow(clippy::needless_range_loop)] // rows and columns index several arrays at once

use uradio_bus::{Delay, InPin, OutPin};

/// No key at this matrix position.
pub const NONE: u8 = 0xFF;

/// A key matrix with `R` driven rows and `C` columns, an optional ground row, and `D` direct keys.
pub struct Matrix<O: OutPin, I: InPin, P: InPin, const R: usize, const C: usize, const D: usize> {
    rows: [O; R],
    cols: [I; C],
    direct: [P; D],
    /// Codes: `ground` for the ground row, `codes[r][c]` for row r; `direct_codes` for the direct keys.
    ground: Option<[u8; C]>,
    codes: [[u8; C]; R],
    direct_codes: [u8; D],
    settle_us: u32,
}

impl<O: OutPin, I: InPin, P: InPin, const R: usize, const C: usize, const D: usize> Matrix<O, I, P, R, C, D> {
    /// `settle_us`: wait after driving a row, for the pull-ups' RC (20 us on the RT-950 Pro). Rows are released
    /// (high) here.
    pub fn new(mut rows: [O; R], cols: [I; C], direct: [P; D], ground: Option<[u8; C]>, codes: [[u8; C]; R], direct_codes: [u8; D], settle_us: u32) -> Self {
        for r in rows.iter_mut() {
            r.set(true);
        }
        Matrix { rows, cols, direct, ground, codes, direct_codes, settle_us }
    }

    fn cols_low(&mut self) -> u32 {
        self.cols.iter_mut().enumerate().fold(0, |m, (c, p)| if p.is_high() { m } else { m | 1 << c })
    }

    /// One scan: bit `code` set for every key down.
    pub fn scan(&mut self, delay: &mut impl Delay) -> u32 {
        let mut keys = 0u32;
        let mut set = |code: u8| {
            if code != NONE && code < 32 {
                keys |= 1 << code;
            }
        };
        // A column held low with no row driven is a ground-row key; it also masks that column in every row.
        let grounded = self.cols_low();
        if let Some(g) = self.ground {
            for c in 0..C {
                if grounded & 1 << c != 0 {
                    set(g[c]);
                }
            }
        }
        for r in 0..R {
            self.rows[r].set(false);
            delay.delay_us(self.settle_us);
            let low = self.cols_low() & !grounded;
            self.rows[r].set(true);
            for c in 0..C {
                if low & 1 << c != 0 {
                    set(self.codes[r][c]);
                }
            }
        }
        for (p, &code) in self.direct.iter_mut().zip(&self.direct_codes) {
            if !p.is_high() {
                set(code);
            }
        }
        keys
    }
}

/// A key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Press(u8),
    Release(u8),
    /// Held for the long-press time (once per press).
    Long(u8),
    /// Still held: repeats every repeat interval after the long press.
    Repeat(u8),
}

/// Debouncing and events, fed one scan every tick (10 ms, say).
pub struct Keys {
    stable: u32,
    candidate: u32,
    count: u8,
    need: u8,
    /// Ticks the current single key has been held, and whether its long press was sent.
    held: u16,
    long_sent: bool,
    long_ticks: u16,
    repeat_ticks: u16,
}

impl Keys {
    /// `debounce`: scans that must agree (2-3 at 10 ms); `long_ticks`: hold for a long press; `repeat_ticks`:
    /// repeat interval after it (0: no repeat).
    pub const fn new(debounce: u8, long_ticks: u16, repeat_ticks: u16) -> Self {
        Keys {
            stable: 0,
            candidate: 0,
            count: 0,
            need: if debounce == 0 { 1 } else { debounce },
            held: 0,
            long_sent: false,
            long_ticks,
            repeat_ticks,
        }
    }

    /// The debounced keys down.
    pub fn down(&self) -> u32 {
        self.stable
    }

    /// One tick's scan in, events out (through `out`, in order: releases, then presses, then held).
    pub fn tick(&mut self, raw: u32, mut out: impl FnMut(Event)) {
        if raw == self.candidate {
            self.count = self.count.saturating_add(1);
        } else {
            self.candidate = raw;
            self.count = 1;
        }
        if self.count >= self.need && self.candidate != self.stable {
            let (old, new) = (self.stable, self.candidate);
            for code in 0..32 {
                if old & !new & 1 << code != 0 {
                    out(Event::Release(code));
                }
            }
            for code in 0..32 {
                if new & !old & 1 << code != 0 {
                    out(Event::Press(code));
                }
            }
            self.stable = new;
            self.held = 0;
            self.long_sent = false;
            return;
        }
        // Long press and repeat: only for a single key held alone (chords are the UI's business).
        if self.stable.count_ones() == 1 {
            let code = self.stable.trailing_zeros() as u8;
            self.held = self.held.saturating_add(1);
            if !self.long_sent && self.long_ticks > 0 && self.held >= self.long_ticks {
                self.long_sent = true;
                out(Event::Long(code));
            } else if self.long_sent && self.repeat_ticks > 0 && (self.held - self.long_ticks).is_multiple_of(self.repeat_ticks) {
                out(Event::Repeat(code));
            }
        }
    }
}

/// Quadrature encoder: call with both pins on every change (or every tick); returns +1 or -1 per detent.
pub struct Encoder {
    last: u8,
    acc: i8,
    per_detent: i8,
}

impl Encoder {
    /// `per_detent`: quadrature transitions per click (4 for most mechanical encoders, 2 or 1 for some).
    pub const fn new(per_detent: i8) -> Self {
        Encoder { last: 0, acc: 0, per_detent }
    }

    pub fn update(&mut self, a: bool, b: bool) -> i8 {
        // Valid Gray-code steps +-1; invalid (two bits changed: a missed step) count as 0.
        const TABLE: [i8; 16] = [0, -1, 1, 0, 1, 0, 0, -1, -1, 0, 0, 1, 0, 1, -1, 0];
        let now = (a as u8) << 1 | b as u8;
        self.acc += TABLE[(self.last << 2 | now) as usize];
        self.last = now;
        if self.acc >= self.per_detent {
            self.acc = 0;
            1
        } else if self.acc <= -self.per_detent {
            self.acc = 0;
            -1
        } else {
            0
        }
    }
}
