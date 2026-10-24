//! DTMF heard (DEC-6): the BK4829's own detector on each chip that receives for a slot.

use super::*;

impl Radio {
    /// DEC-6: arm the detector on every chip that receives for a slot, while 0 4 3 is on, and take its digits
    /// into the slot's string ([`ferrite_core::dtmfrx::Track`]), published for the card. Armed again whenever
    /// the chip retunes or wakes (the receive set-up comes first), and after a transmission (the tone generator
    /// shares REG_70, so nothing is written while keyed). Never on the chip SAME listens with: that chip's
    /// interrupt flags are SAME's to read (DEC-19); on the others they are read here alone.
    pub(super) fn dtmf_tick(&mut self, now: u32) {
        for n in 0..2u8 {
            let i = n as usize;
            if self.live.tx.keyed {
                self.dtmf_key[i] = None; // armed again after the unkey's retune
                continue;
            }
            let t = self.tuned[i];
            let want = self.dtmf_on && self.has(n) && self.same.chip != Some(n) && t.is_some_and(|(s, _)| (s as usize) < SLOTS);
            let key = if want { t } else { None };
            if key != self.dtmf_key[i] {
                if key.is_some() {
                    self.chip(n).dtmf_rx(true);
                } else if t.is_some() && self.has(n) && self.same.chip != Some(n) {
                    self.chip(n).dtmf_rx(false);
                }
                self.dtmf_key[i] = key;
            }
            let Some((s, _)) = key else { continue };
            let mut bk = self.chip(n);
            if bk.interrupts() & int::DTMF != 0 {
                let code = bk.dtmf_index();
                self.dtmf[s as usize].digit(code, now);
            }
        }
        for s in 0..SLOTS {
            let carrier = self.live.slots[s].squelch_open;
            self.dtmf[s].tick(now, carrier);
            self.live.slots[s].dtmf = self.dtmf[s].heard;
        }
    }
}
