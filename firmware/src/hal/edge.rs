//! TERM-33 `afsk-bk`'s bit timer (decision 134): TMR5, free otherwise (TMR2 the keypad light, TMR3 the time driver,
//! TMR4 the ADC trigger, TMR6 the DAC trigger, TMR7 the microsecond counter, TMR8 the screen light). It counts at
//! [`TICK_HZ`], 1.2 MHz (the 120 MHz timer clock / 100): exactly 1000 ticks a bit at 1200 Bd. The counter runs free
//! over 16 bits (54.6 ms a turn) and channel 1's compare interrupt marks each tone change; the caller adds each
//! change's ticks from its schedule (`ferrite_core::txtest::BkFlags`), so the changes never drift from the start,
//! whenever the interrupt is served. Only the `tx` build has it, and only while an `afsk-bk` test plays.
use super::regs::*;
use super::{irq, modify, rd, wr};

/// TMR5 global (RM_AT32F403A_407 V2.07, the vector table: position 50).
pub const IRQ: u16 = 50;
/// The count rate: 1200 Bd is exactly 1000 ticks.
pub const TICK_HZ: u32 = 1_200_000;
const _: () = assert!(super::clock::TMR_HZ.is_multiple_of(TICK_HZ) && TICK_HZ.is_multiple_of(1200));
const CC1: u32 = 1 << 1;

/// Counting from now, no compare armed, the interrupt at `prio`; returns the count.
pub fn start(prio: u8) -> u16 {
    modify(CRM_APB1EN, 0, CRM_APB1_TMR5);
    wr(TMR5 + TMR_CTRL1, 0);
    wr(TMR5 + TMR_IDEN, 0);
    wr(TMR5 + TMR_DIV, super::clock::TMR_HZ / TICK_HZ - 1);
    wr(TMR5 + TMR_PR, 0xFFFF);
    wr(TMR5 + TMR_SWEVT, 1); // load DIV now
    wr(TMR5 + TMR_ISTS, 0); // the load set the overflow flag
    irq::enable(IRQ, prio);
    wr(TMR5 + TMR_CTRL1, 1);
    now()
}

pub fn now() -> u16 {
    rd(TMR5 + TMR_CVAL) as u16
}

/// Channel 1's interrupt at count `at`. One already passed (the interrupt was held off longer than the gap to the
/// next change, which the 1-bit gap makes 833 us) is raised at once by a software compare event instead of a
/// whole turn of the counter later, unless the compare has already flagged itself.
pub fn arm(at: u16) {
    wr(TMR5 + TMR_C1DT, at as u32);
    modify(TMR5 + TMR_IDEN, 0, CC1);
    let ahead = at.wrapping_sub(now());
    if (ahead == 0 || ahead >= 0x8000) && rd(TMR5 + TMR_ISTS) & CC1 == 0 {
        wr(TMR5 + TMR_SWEVT, CC1);
    }
}

/// In the interrupt: the compare flag cleared (the others left, none of them enabled).
pub fn ack() {
    wr(TMR5 + TMR_ISTS, !CC1);
}

/// Stopped: the interrupt off and cleared, the counter stopped, the clock off.
pub fn stop() {
    irq::disable(IRQ);
    wr(TMR5 + TMR_IDEN, 0);
    wr(TMR5 + TMR_CTRL1, 0);
    wr(TMR5 + TMR_ISTS, 0);
    wr(NVIC_ICPR + 4 * (IRQ as u32 / 32), 1 << (IRQ % 32)); // nothing left pending for the next start
    modify(CRM_APB1EN, CRM_APB1_TMR5, 0);
}
