//! Embassy time driver on TMR3 (a general-purpose timer, so SysTick stays free). No Artery support exists
//! in Embassy, so this is ours; it follows embassy-stm32's 16-bit timer driver.
//!
//! - TMR3 counts at 1 kHz (8 MHz / 8000): one tick is 1 ms, the FreeRTOS tick of the C image
//!   (embassy-time-driver feature `tick-hz-1_000`). Revisit with the clock when the PLL comes in.
//! - The 16-bit counter is extended to 64 bits by counting half periods: the overflow interrupt and a
//!   compare at 0x8000 (channel 4) each advance `period`, so `now()` never needs the two to agree exactly.
//!   An overflow every 65.5 s, so two interrupts a minute when nothing else is due.
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
    wr(TMR3 + TMR_DIV, super::clock::APB1_HZ / 1000 - 1);
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
