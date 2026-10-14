//! TERM-33 `afsk-bk` (decision 134): AFSK flags from the keyed BK4829's tone generator, timed by TMR5.
//!
//! Timing and the bus are split so the bus stays the Radio task's alone: TMR5's interrupt ([`on_interrupt`],
//! priority 4, above the Radio task's executor) only counts the change, arms the next one from the schedule
//! (`ferrite_core::txtest::BkFlags`: exact 1200 Bd from the start, accumulated, never drifting) and wakes the Radio
//! task; the Radio task, woken on [`EDGE`] in its main wait, writes REG_71 for the count it finds
//! ([`Radio::afsk_bk_edge`]). A change's air time is the interrupt's (exact) plus the Radio task's wake (a few
//! microseconds when idle, longer when a tick's work or a command is running) plus the REG_71 write (24 bits at
//! about 100 kHz, about 250 us, the same every time). Should the Radio task fall a whole change behind, it writes the
//! tone the count says, so the tones stay right after a missed change, and the misses are logged at the end. About
//! 300 writes a second (two changes per flag): about 8 % of the CPU at the Radio task's priority while it plays.
//! Rejected: the embassy timer alone (its 2 kHz tick is 500 us, over half a bit), and busy-waiting to each change
//! (up to a tick's 500 us at the Radio task's priority, 300 times a second, would starve the UI); writing the
//! chip from the interrupt (the bus is the Radio task's; a write in progress there would be cut).
use super::*;
use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use critical_section::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex as M;
use embassy_sync::signal::Signal;
use ferrite_core::txtest::{bk_gain, bk_tone, BkFlags};

/// TMR5's priority: the time driver's (4), above every executor.
const PRIO: u8 = 4;

/// The schedule and the count at the start (TMR5's 16 bits), while playing.
static SCHED: Mutex<RefCell<Option<(BkFlags, u16)>>> = Mutex::new(RefCell::new(None));
/// Tone changes made by the timer so far.
static CHANGES: AtomicU32 = AtomicU32::new(0);
/// A change was made: the Radio task's wake.
pub(super) static EDGE: Signal<M, ()> = Signal::new();

/// TMR5's interrupt: the change counted, the next one armed, the Radio task woken.
pub fn on_interrupt() {
    crate::hal::edge::ack();
    let next = critical_section::with(|cs| SCHED.borrow_ref_mut(cs).as_mut().map(|(f, t0)| t0.wrapping_add(f.next_change() as u16)));
    if let Some(at) = next {
        crate::hal::edge::arm(at);
        CHANGES.fetch_add(1, Relaxed);
        EDGE.signal(());
    }
}

/// `afsk-bk` playing: the keyed chip, the changes written, and the changes missed (a later one came first).
#[derive(Clone, Copy)]
pub(super) struct BkRun {
    chip: u8,
    sent: u32,
    missed: u32,
}

impl Radio {
    /// After the key-up's plan: the chip to tone transmit on the space tone at LEVEL's gain, then the timer.
    pub(super) fn afsk_bk_start(&mut self, n: u8, level: u8) {
        let narrow = self.tx.narrow;
        radio_chip::afsk_bk_begin(&mut self.chip(n), bk_tone(0), bk_gain(level), narrow);
        self.afsk_bk_timer(n);
        log(b"txtest afsk-bk gain ", bk_gain(level) as u32);
    }

    /// The timer from now, the chip already sending the space tone (TERM-34's flag steps set it with the rest of
    /// the step): the first change 7 bits on.
    pub(super) fn afsk_bk_timer(&mut self, n: u8) {
        CHANGES.store(0, Relaxed);
        EDGE.reset();
        let t0 = crate::hal::edge::start(PRIO);
        let mut f = BkFlags::new(crate::hal::edge::TICK_HZ);
        let first = t0.wrapping_add(f.next_change() as u16);
        critical_section::with(|cs| *SCHED.borrow_ref_mut(cs) = Some((f, t0)));
        crate::hal::edge::arm(first);
        self.tx.bk = Some(BkRun { chip: n, sent: 0, missed: 0 });
    }

    /// The Radio task woken by [`EDGE`]: the tone for the count of changes so far (REG_71, one write).
    pub(super) fn afsk_bk_edge(&mut self) {
        let Some(run) = self.tx.bk.as_mut() else { return };
        let n = CHANGES.load(Relaxed);
        if n == run.sent {
            return;
        }
        run.missed += n - run.sent - 1;
        run.sent = n;
        let chip = run.chip;
        if self.has(chip) {
            radio_chip::afsk_bk_tone(&mut self.chip(chip), bk_tone(n));
        }
    }

    /// At the release (or any unkey): the timer stopped, then the tones off on the chip (REG_70 0, REG_30 left at
    /// tone transmit for the unkey's REG_30 = 0, as the end tone leaves it).
    pub(super) fn afsk_bk_stop(&mut self) {
        let Some(chip) = self.afsk_bk_halt() else { return };
        if self.has(chip) {
            radio_chip::afsk_bk_end(&mut self.chip(chip));
        }
    }

    /// The timer stopped and the run's counts logged; the chip untouched (TERM-34's next step turns the tones
    /// off). The chip it played on, if one played.
    pub(super) fn afsk_bk_halt(&mut self) -> Option<u8> {
        let run = self.tx.bk.take()?;
        crate::hal::edge::stop();
        critical_section::with(|cs| *SCHED.borrow_ref_mut(cs) = None);
        EDGE.reset();
        log(b"txtest afsk-bk changes ", run.sent);
        if run.missed != 0 {
            log(b"txtest afsk-bk missed ", run.missed);
        }
        Some(run.chip)
    }

    /// Whether `afsk-bk` plays (the main wait listens to [`EDGE`] only then).
    pub(super) fn afsk_bk_on(&self) -> bool {
        self.tx.bk.is_some()
    }
}
