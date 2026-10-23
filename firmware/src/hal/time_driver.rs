//! Embassy time driver on TMR3 (a general-purpose timer, so SysTick stays free). No Artery support exists
//! in Embassy, so this is ours; it follows embassy-stm32's 16-bit timer driver.
//!
//! - TMR3 counts at 2 kHz (120 MHz timer clock / 60000; 1 kHz would need a prescaler over 16 bits): one tick is
//!   0.5 ms (embassy-time-driver feature `tick-hz-2_000`). At a speed switch (PWR-24.9) it is stopped at a tick
//!   edge and moved on by the time the switch took ([`freeze`], [`thaw`]), so time neither jumps nor drifts.
//! - The 16-bit counter is extended to 64 bits by counting half periods: the overflow interrupt and a
//!   compare at 0x8000 (channel 4) each advance `period`, so `now()` never needs the two to agree exactly.
//!   An overflow every 32.8 s, so four interrupts a minute when nothing else is due.
//! - Channel 1 is the alarm. An alarm more than 0xC000 ticks out is armed by the period interrupt once it
//!   comes within range.
//! - One timer queue (embassy-time-queue-utils, the executor's integrated queue) serves both executors.
use super::irq;
use super::regs::*;
use super::{modify, rd, wr};
use core::cell::{Cell, RefCell};
use core::sync::atomic::{compiler_fence, AtomicU32, Ordering};
use core::task::Waker;
use critical_section::{CriticalSection, Mutex};
use embassy_time_driver::Driver;
use embassy_time_queue_utils::Queue;
use ferrite_core::clocktree::{tick_count_safe, Tree, TICK_HZ};

pub const IRQ: u16 = 29; // TMR3 global
const UIF: u32 = 1 << 0; // overflow
const CC1: u32 = 1 << 1; // alarm
const CC4: u32 = 1 << 4; // half period

struct Tmr3 {
    period: AtomicU32,
    alarm: Mutex<Cell<u64>>,
    queue: Mutex<RefCell<Queue>>,
}

embassy_time_driver::time_driver_impl!(static DRIVER: Tmr3 = Tmr3 {
    period: AtomicU32::new(0),
    alarm: Mutex::new(Cell::new(u64::MAX)),
    queue: Mutex::new(RefCell::new(Queue::new())),
});

fn calc_now(period: u32, counter: u16) -> u64 {
    ((period as u64) << 15) + (counter as u32 ^ ((period & 1) << 15)) as u64
}

/// Start the timer; `prio` is its NVIC priority (0-15).
pub fn init(prio: u8) {
    modify(CRM_APB1EN, 0, CRM_APB1_TMR3);
    wr(TMR3 + TMR_CTRL1, 0);
    wr(TMR3 + TMR_DIV, super::clock::tree().tmr_div(TICK_HZ));
    wr(TMR3 + TMR_PR, 0xFFFF);
    wr(TMR3 + TMR_C4DT, 0x8000);
    wr(TMR3 + TMR_SWEVT, 1); // load DIV now
    wr(TMR3 + TMR_ISTS, 0); // the load set the overflow flag
    wr(TMR3 + TMR_IDEN, UIF | CC4);
    irq::enable(IRQ, prio);
    wr(TMR3 + TMR_CTRL1, 1);
}

/// The TMR3 interrupt handler.
pub fn on_interrupt() {
    let sts = rd(TMR3 + TMR_ISTS);
    wr(TMR3 + TMR_ISTS, !sts); // clear exactly the flags seen
    let armed = rd(TMR3 + TMR_IDEN) & CC1 != 0;
    if sts & UIF != 0 {
        DRIVER.next_period();
    }
    if sts & CC4 != 0 {
        DRIVER.next_period();
    }
    if sts & CC1 != 0 && armed {
        critical_section::with(|cs| DRIVER.trigger_alarm(cs));
    }
}

impl Tmr3 {
    fn next_period(&self) {
        let period = self.period.load(Ordering::Relaxed) + 1;
        self.period.store(period, Ordering::Relaxed);
        let t = (period as u64) << 15;
        critical_section::with(|cs| {
            if self.alarm.borrow(cs).get() < t + 0xC000 {
                modify(TMR3 + TMR_IDEN, 0, CC1); // C1DT already holds the right value
            }
        });
    }

    fn trigger_alarm(&self, cs: CriticalSection) {
        let mut next = self.queue.borrow(cs).borrow_mut().next_expiration(self.now());
        while !self.set_alarm(cs, next) {
            next = self.queue.borrow(cs).borrow_mut().next_expiration(self.now());
        }
    }

    /// False when `at` has already passed (the caller then runs the queue again).
    fn set_alarm(&self, cs: CriticalSection, at: u64) -> bool {
        let alarm = self.alarm.borrow(cs);
        alarm.set(at);
        if at <= self.now() {
            modify(TMR3 + TMR_IDEN, CC1, 0);
            alarm.set(u64::MAX);
            return false;
        }
        // Write the compare value now; arm it if it falls within reach, else next_period will.
        wr(TMR3 + TMR_C1DT, at as u16 as u32);
        let near = at.saturating_sub(self.now()) < 0xC000;
        modify(TMR3 + TMR_IDEN, CC1, if near { CC1 } else { 0 });
        // The counter may have passed `at` while this ran.
        if at <= self.now() {
            modify(TMR3 + TMR_IDEN, CC1, 0);
            alarm.set(u64::MAX);
            return false;
        }
        true
    }
}

impl Driver for Tmr3 {
    fn now(&self) -> u64 {
        let period = self.period.load(Ordering::Relaxed);
        compiler_fence(Ordering::Acquire);
        calc_now(period, rd(TMR3 + TMR_CVAL) as u16)
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        critical_section::with(|cs| {
            let mut queue = self.queue.borrow(cs).borrow_mut();
            if queue.schedule_wake(at, waker) {
                let mut next = queue.next_expiration(self.now());
                while !self.set_alarm(cs, next) {
                    next = queue.next_expiration(self.now());
                }
            }
        });
    }
}

/// PWR-24.9, before a speed switch (interrupts on): the count just after a fresh tick, away from the half-period
/// marks ([`tick_count_safe`]); up to half a millisecond, a few at worst near a mark (once every 16 s).
pub(super) fn wait_safe_tick() -> u16 {
    loop {
        let c0 = rd(TMR3 + TMR_CVAL) as u16;
        let c = loop {
            let c = rd(TMR3 + TMR_CVAL) as u16;
            if c != c0 {
                break c;
            }
        };
        if tick_count_safe(c) {
            return c;
        }
    }
}

/// Interrupts off: TMR3 stopped, still on the tick [`wait_safe_tick`] returned (`seen`). None (and running
/// again) if an interrupt between them let the count move on: the caller waits for the next edge.
pub(super) fn freeze(seen: u16) -> Option<u16> {
    modify(TMR3 + TMR_CTRL1, 1, 0);
    if rd(TMR3 + TMR_CVAL) as u16 == seen {
        Some(seen)
    } else {
        modify(TMR3 + TMR_CTRL1, 0, 1);
        None
    }
}

/// Interrupts off: TMR3 at 2 kHz from the new tree, its count `cnt + ticks` (never past a half-period mark:
/// [`wait_safe_tick`]), started again. An alarm that fell due while it was stopped is raised now by a software
/// compare event: the interrupt runs the queue.
pub(super) fn thaw(cnt: u16, ticks: u32, t: Tree) {
    wr(TMR3 + TMR_DIV, t.tmr_div(TICK_HZ));
    modify(TMR3 + TMR_CTRL1, 0, super::time::TMR_CTRL1_OVFS);
    wr(TMR3 + TMR_SWEVT, 1); // load DIV, reset the prescaler; no overflow flag (OVFS)
    modify(TMR3 + TMR_CTRL1, super::time::TMR_CTRL1_OVFS, 0);
    wr(TMR3 + TMR_CVAL, cnt.wrapping_add(ticks as u16) as u32);
    if rd(TMR3 + TMR_IDEN) & CC1 != 0 {
        wr(TMR3 + TMR_SWEVT, CC1);
    }
    modify(TMR3 + TMR_CTRL1, 0, 1);
}
