//! TERM-34 `txtest suite` (decision 139): every transmit baseband test in one PTT key-up, run step by step from
//! `ferrite_core::txsuite::STEPS`. The Radio task owns the chips, so it runs the list: at each step's time (counted
//! from the key-up: each deadline is the last one plus the step's length, so nothing drifts) it writes what differs
//! from the step before (`ferrite_radio::chip::suite_step`), starts or stops TMR5 for the generator's flags
//! (`afskbk`), and hands DAC1's part to the DSP task (`dsp::TXTEST`, a new generator taking over while DAC1 runs).
//! After the last step it holds the 1 kHz `HOLD` until the release; the release (or any unkey) ends it and puts
//! every register back (`suite_end`) before the unkey's steps. The main loop waits on [`Radio::suite_due`].
use super::*;
use ferrite_core::txsuite::{self, Src, Step, HOLD, LEN, STEPS};

/// The suite while it runs: the keyed chip, the next step (LEN: the hold next; past it, holding), when it starts
/// (ms since boot; None while holding), and what the key-up left on the chip.
#[derive(Clone, Copy)]
pub(super) struct SuiteRun {
    chip: u8,
    next: u8,
    due: Option<u64>,
    regs: radio_chip::SuiteRegs,
}

impl Radio {
    /// After the key-up's plan, on the keyed chip `n`: its registers kept, the sub-tone off, and the first step.
    pub(super) fn suite_start(&mut self, n: u8, setup: &radio_chip::TxSetup) {
        let regs = radio_chip::suite_begin(&mut self.chip(n), setup.tone, setup.tone_dev);
        let now = now_ms();
        self.tx.suite = Some(SuiteRun { chip: n, next: 0, due: Some(now), regs });
        self.tx.test_playing = true;
        self.suite_next();
    }

    /// When the next step starts (the main loop's wait), while one is due.
    pub(super) fn suite_due(&self) -> Option<u64> {
        self.tx.suite.and_then(|s| s.due)
    }

    /// The section running (1-7; 0 the lead-in, 8 the hold) and the ms left to the hold, for the terminal.
    pub(super) fn suite_status(&self) -> Option<(u8, u32)> {
        let s = self.tx.suite?;
        let i = s.next as usize;
        if i > LEN {
            return Some((8, 0));
        }
        let left: u32 = STEPS[i.min(LEN)..].iter().map(|t| t.ms as u32).sum();
        let at = s.due.map_or(0, |d| d.saturating_sub(now_ms()) as u32);
        Some((txsuite::section(i.saturating_sub(1)), left + at))
    }

    /// The step due now: from the one before to it on the chip, TMR5 and DAC1; then the next deadline.
    pub(super) fn suite_next(&mut self) {
        let Some(mut run) = self.tx.suite else { return };
        let i = run.next as usize;
        let Some(due) = run.due else { return };
        let next: Step = if i < LEN { STEPS[i] } else { HOLD };
        let prev: Option<Step> = if i == 0 { None } else { Some(STEPS[i - 1]) };
        let n = run.chip;
        if prev.is_some_and(|p| p.src == Src::BkFlags) {
            self.afsk_bk_halt();
        }
        if self.has(n) {
            let t = next.src.subtone();
            let dev = self.tx.cal.map_or(0, |c| c.tone_dev(t, 1)); // the calibration's byte for its kind (F7)
            let narrow = self.tx.narrow;
            radio_chip::suite_step(&mut self.chip(n), &run.regs, prev.as_ref(), &next, narrow, dev);
            if next.src == Src::BkFlags {
                self.afsk_bk_timer(n);
            }
        }
        let dac = next.src.dac();
        if prev.map(|p| p.src.dac()) != Some(dac) {
            crate::tasks::dsp::TXTEST.signal(Some(dac.gen()));
        }
        if txsuite::SECTION_START.contains(&(i as u8)) {
            log(b"txtest suite section ", txsuite::section(i) as u32);
        }
        run.next += 1;
        run.due = (i < LEN).then(|| due + next.ms as u64);
        if i == LEN {
            log(b"txtest suite done, holding 1 kHz", 0);
        }
        self.tx.suite = Some(run);
    }

    /// At the release or any unkey (before its steps): TMR5 stopped, the chip's registers back as the key-up left
    /// them. DAC1 is stopped by the caller with the rest of the test audio.
    pub(super) fn suite_stop(&mut self) {
        let Some(run) = self.tx.suite.take() else { return };
        self.afsk_bk_halt();
        if self.has(run.chip) {
            radio_chip::suite_end(&mut self.chip(run.chip), &run.regs);
        }
        log(b"txtest suite ended at step ", run.next as u32);
    }
}
