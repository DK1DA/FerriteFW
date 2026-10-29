//! TERM-45 (decision 318, a bench test): 4FSK from the keyed chip's tone generator with the microphone off
//! (`ferrite_core::synthfsk`). A blocking burst on the Radio task: the carrier moved down by `shift_hz` (written
//! before re-entering transmit, the only way a frequency takes effect), tone transmit (REG_30 0xC3FA: the mic ADC
//! off), the transmit filters and DC blocks bypassed, REG_40 at the run's scale, tone 1 run for a quarter cycle at
//! 1 kHz and then stopped so its output holds near its peak, then tone 1's gain written each step on the cycle
//! counter's schedule. Everything is put back after: the registers, the frequency, the mic.

use super::*;
use bk4829::tx::Source;
use core::sync::atomic::Ordering;
use ferrite_core::synthfsk::{Cfg, Report, Steps};

impl Radio {
    pub(super) fn synth_fsk(&mut self, c: Cfg) -> DebugAns {
        if !crate::chan::DRY_TX.load(Ordering::SeqCst) {
            return DebugAns::Refused("Dry TX is off: synthfsk runs only with Dry TX on (0 8 8 5)");
        }
        let Some(sel) = self.path.keyed() else { return DebugAns::Refused("not keyed: key up first, then synthfsk") };
        let n = sel.chip();
        if !self.has(n) {
            return DebugAns::Refused("that BK4829 didn't answer at start-up");
        }
        let wide = !self.tx.narrow;
        let mut bk = self.chip(n);
        let f0 = bk.freq();
        let saved = [0x70u8, 0x71, 0x47, 0x7E, 0x29, 0x2B, 0x40].map(|r| (r, bk.read(r)));
        let get = |r: u8| saved.iter().find(|s| s.0 == r).map_or(0, |s| s.1);
        bk.set_freq(f0.saturating_sub(c.shift_hz as u32));
        bk.tx_on(Source::Tones, wide);
        bk.write(0x47, get(0x47) | 1); // all AF TX filters bypassed
        bk.write(0x7E, get(0x7E) & !0x0038); // TX DC filter off
        bk.write(0x29, get(0x29) & !0x0E00); // TX DC filter HPF1 off
        bk.write(0x2B, get(0x2B) | 0x0007); // TX HPF, LPF1, pre-emphasis off
        bk.write(0x40, (get(0x40) & 0xF000) | (c.dev & 0x0FFF));
        let mut steps = Steps::new(c);
        let rate = steps.rate() as u64;
        let first = steps.next_gain().unwrap_or(0);
        // tone 1 at 1 kHz for a quarter cycle, then held: its output stays near the peak
        bk.write(0x70, 1 << 15 | (first as u16) << 8);
        bk.write(0x71, bk4829::tone_word(1000));
        crate::hal::time::delay_us(250);
        bk.write(0x71, 0);
        let hz = crate::hal::clock::sysclk_hz() as u64;
        let mut r = Report { freq_word: f0 / 10, ..Report::default() };
        let (mut sum, mut k) = (0u64, 1u64);
        let t0 = crate::hal::time::cycles();
        while let Some(g) = steps.next_gain() {
            let due = t0.wrapping_add((k * hz / rate) as u32);
            while (crate::hal::time::cycles().wrapping_sub(due) as i32) < 0 {}
            let w0 = crate::hal::time::cycles();
            let late = w0.wrapping_sub(due) as u64;
            bk.write(0x70, 1 << 15 | (g as u16) << 8);
            let dw = crate::hal::time::cycles().wrapping_sub(w0) as u64;
            if late * rate * 4 > hz {
                r.late += 1;
            }
            r.late_max_us = r.late_max_us.max((late * 1_000_000 / hz).min(u16::MAX as u64) as u16);
            r.write_max_x10 = r.write_max_x10.max((dw * 10_000_000 / hz) as u16);
            sum += dw;
            k += 1;
        }
        r.steps = k as u32;
        r.write_avg_x10 = (sum * 10_000_000 / hz / k) as u16;
        for (reg, v) in saved {
            bk.write(reg, v);
        }
        bk.set_freq(f0);
        bk.tx_on(Source::Mic, wide);
        DebugAns::SynthFsk(r)
    }
}
