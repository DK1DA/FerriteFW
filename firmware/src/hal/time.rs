//! Busy-wait timing on the cycle counter (usable before and outside the executors), and a free-running
//! 16-bit microsecond counter on TMR7 that keeps counting while the core sleeps. Mirrors fw/hal/hal_time.
use super::clock::{SYSCLK_HZ, TMR_HZ};
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
    let ticks = us * (SYSCLK_HZ / 1_000_000);
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
    wr(TMR7 + TMR_DIV, TMR_HZ / 1_000_000 - 1);
    wr(TMR7 + TMR_PR, 0xFFFF);
    wr(TMR7 + TMR_SWEVT, 1);
    wr(TMR7 + TMR_CTRL1, 1);
}

/// Differences of up to 65 ms.
pub fn us16() -> u16 {
    rd(TMR7 + TMR_CVAL) as u16
}
