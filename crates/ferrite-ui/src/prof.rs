//! The drawing profile (bench builds, the `prof` feature; Oct 23, 2026): cycles each stage of a redraw took, and
//! the big fonts' glyph rows decoded against drawn, summed until the UI task's draw log takes them. Without the
//! feature every call is empty and the clock reads 0.
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering::Relaxed};

/// A region's drawing run into its recording (DISP-29).
pub const REC: usize = 0;
/// Text in a run-length encoded (big) font, to a band or the panel.
pub const BIG: usize = 1;
/// Text in the other fonts.
pub const TEXT: usize = 2;
/// Masks (the logo, icons).
pub const MASK: usize = 3;
/// A new screen's background (DISP-29's cover).
pub const COVER: usize = 4;
/// Counts, not cycles: the big fonts' glyph rows decoded, and drawn.
pub const ROWS_DECODED: usize = 5;
pub const ROWS_DRAWN: usize = 6;
/// Recorded fills, rounded fills and frames replayed (into bands, or straight to the panel).
pub const FILL: usize = 7;
/// Each band's clear to its background and its send to the panel (the bus).
pub const PUSH: usize = 8;
/// Whole regions drawn in bands, everything in them included.
pub const BANDS: usize = 9;
pub const N: usize = 10;

static P: [AtomicU32; N] = [const { AtomicU32::new(0) }; N];
/// The cycle counter, a `fn() -> u32` (0: none).
static CLOCK: AtomicUsize = AtomicUsize::new(0);

/// The firmware's cycle counter.
pub fn set_clock(f: fn() -> u32) {
    if cfg!(feature = "prof") {
        CLOCK.store(f as usize, Relaxed);
    }
}

#[inline(always)]
pub fn now() -> u32 {
    if !cfg!(feature = "prof") {
        return 0;
    }
    match CLOCK.load(Relaxed) {
        0 => 0,
        // SAFETY: only `set_clock` stores here, a `fn() -> u32`.
        f => unsafe { core::mem::transmute::<usize, fn() -> u32>(f)() },
    }
}

/// Cycles since `t0` into stage `i`.
#[inline(always)]
pub fn add(i: usize, t0: u32) {
    if cfg!(feature = "prof") {
        P[i].fetch_add(now().wrapping_sub(t0), Relaxed);
    }
}

/// `n` more into count `i`.
#[inline(always)]
pub fn count(i: usize, n: u32) {
    if cfg!(feature = "prof") {
        P[i].fetch_add(n, Relaxed);
    }
}

/// Everything so far, zeroed.
pub fn take() -> [u32; N] {
    core::array::from_fn(|i| P[i].swap(0, Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cycles() -> u32 {
        1_000
    }

    /// With the feature (bench builds), cycles since a start and counts are summed per stage until taken, then
    /// start again from 0; without it the clock reads 0 and nothing is kept. (Other tests draw at the same time
    /// and may add to the sums, never take from them.)
    #[test]
    fn stages_sum_until_taken() {
        set_clock(cycles);
        let t0 = now();
        add(COVER, t0.wrapping_sub(7));
        count(ROWS_DECODED, u32::MAX / 4);
        count(ROWS_DECODED, 3);
        let p = take();
        if cfg!(feature = "prof") {
            assert_eq!(t0, 1_000);
            assert!(p[COVER] >= 7 && p[ROWS_DECODED] >= u32::MAX / 4 + 3);
            assert!(take()[ROWS_DECODED] < u32::MAX / 4, "taking zeroes them");
        } else {
            assert_eq!((t0, p), (0, [0; N]));
        }
    }
}
