//! The SAME weather-alert decode.

use super::*;

impl Radio {
    /// SAME (DEC-19, WX-4): the BK4829's own receiver on the chip of a slot tuned to a weather channel, as
    /// long as alerts are on. Each burst's FIFO words go through [`Assembler`]; the first that parses raises
    /// the alert, through the UI task ([`ALERTS`]), and the same text again within `REPEAT_S` doesn't (the
    /// header's other two bursts). The modem is set up again whenever its chip retunes or wakes (Battery Save
    /// keeps its cycle, WX-11).
    pub(super) fn same_tick(&mut self, now: u32) {
        // TERM-43: the bench's FSK modem test has the modem meanwhile; SAME sets up again after it.
        #[cfg(feature = "bench")]
        if self.fsk.is_some() {
            return self.fsk_tick();
        }
        let tuned = self.tuned;
        let wx = |n: u8| tuned[n as usize].filter(|&(s, f)| (s as usize) < SLOTS && WX.contains(&f));
        let chip = if self.same_on { (0..2u8).find(|&n| wx(n).is_some()) } else { None };
        let at = chip.and_then(wx);
        if chip != self.same.chip || at != self.same.at {
            if let Some(old) = self.same.chip.filter(|&o| self.tuned[o as usize].is_some()) {
                self.chip(old).fsk_off();
            }
            (self.same.chip, self.same.at) = (chip, at);
            self.same.armed = false;
        }
        let (Some(n), Some((slot, freq))) = (chip, at) else { return };
        if !self.same.armed {
            let mut bk = self.chip(n);
            radio_chip::same_arm(&mut bk);
            self.same.armed = true;
            return;
        }
        let flags = self.chip(n).interrupts();
        if flags & int::FSK_SYNC != 0 {
            self.same.asm.start();
        }
        if flags & (int::FSK_FIFO_FULL | int::FSK_RX_DONE) == 0 {
            return;
        }
        // Almost full is 4 words (REG_5E's default); done leaves at most the FIFO's 8.
        let words = if flags & int::FSK_RX_DONE != 0 { 8 } else { 4 };
        for _ in 0..words {
            let w = self.chip(n).fsk_read();
            match self.same.asm.push(w) {
                Step::More => continue,
                Step::Done => {
                    if let Some(header) = self.same.repeats.take(self.same.asm.text(), now / 1000) {
                        let _ = ALERTS.try_send(SameDecoded { header, slot, freq });
                    }
                    self.same.armed = false; // listen for the next burst
                }
                Step::Bad => self.same.armed = false,
            }
            break;
        }
        if flags & int::FSK_RX_DONE != 0 {
            self.same.armed = false;
        }
    }
}
