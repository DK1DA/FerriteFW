//! Busy-wait timing on the cycle counter (usable before and outside the executors), and a free-running
//! 16-bit microsecond counter on TMR7 that keeps counting while the core sleeps. Mirrors fw/hal/hal_time.
use super::clock;
use ferrite_core::clocktree::Tree;
use super::regs::*;
use super::{modify, rd, wr};

pub fn init() {
    modify(DEMCR, 0, 1 << 24); // TRCENA
    wr(DWT_CYCCNT, 0);
    modify(DWT_CTRL, 0, 1); // CYCCNTENA
}

pub fn cycles() -> u32 {
    rd(DWT_CYCCNT)
}

pub fn delay_us(us: u32) {
    let start = cycles();
    let ticks = us * clock::mhz(); // the current speed (PWR-24.9)
    while cycles().wrapping_sub(start) < ticks {}
}

/// Busy wait; tasks use embassy_time::Timer instead.
pub fn delay_ms_busy(ms: u32) {
    for _ in 0..ms {
        delay_us(1000);
    }
}

pub fn us_init() {
    modify(CRM_APB1EN, 0, CRM_APB1_TMR7);
    wr(TMR7 + TMR_CTRL1, 0);
    wr(TMR7 + TMR_DIV, clock::tree().tmr_div(1_000_000));
    wr(TMR7 + TMR_PR, 0xFFFF);
    wr(TMR7 + TMR_SWEVT, 1);
    wr(TMR7 + TMR_CTRL1, 1);
}

/// Differences of up to 65 ms.
pub fn us16() -> u16 {
    rd(TMR7 + TMR_CVAL) as u16
}

/// TMR_CTRL1 OVFS: only a counter overflow raises the overflow event, not a software one (the re-timing below).
pub(super) const TMR_CTRL1_OVFS: u32 = 1 << 2;

/// A running timer's prescaler changed now, its count kept (PWR-24.9): the new DIV loaded by a software event
/// with OVFS set (no overflow flag), then the count written back. Interrupts are off.
pub(super) fn retime_counter(base: u32, div: u32) {
    let cnt = rd(base + TMR_CVAL);
    wr(base + TMR_DIV, div);
    modify(base + TMR_CTRL1, 0, TMR_CTRL1_OVFS);
    wr(base + TMR_SWEVT, 1);
    wr(base + TMR_CVAL, cnt);
    modify(base + TMR_CTRL1, TMR_CTRL1_OVFS, 0);
}

/// PWR-24.9: the microsecond counter at 1 MHz from the new tree (its count kept).
pub(super) fn retime(t: Tree) {
    if rd(CRM_APB1EN) & CRM_APB1_TMR7 != 0 {
        retime_counter(TMR7, t.tmr_div(1_000_000));
    }
}
