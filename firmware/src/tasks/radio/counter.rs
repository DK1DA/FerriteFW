//! The frequency counter.

use super::*;

impl Radio {
    /// The frequency counter on or off (TSRCH-10): an exclusive mode like HF mode; watch and scans pause.
    pub(super) fn counter(&mut self, on: bool) {
        for n in 0..2u8 {
            if self.has(n) {
                self.chip(n).freq_scan(false);
            }
        }
        self.tuned = [None; 2]; // the counter, or the engines afterwards, tune them again
        let first = (0..2u8).find(|&n| self.has(n)).unwrap_or(0);
        self.counter = on.then_some(Counter { chip: first, since: 0, last: None });
        self.live.mode = if on { ModeLive::Probe { freq: 0, rssi: -127, tone: 0 } } else { ModeLive::Off };
        speaker(false);
        log(b"counter ", on as u32);
    }

    /// The counter's tick: one chip measures with the BK4829's frequency scan (stock's counter, 0x08021BC4), the
    /// other sleeps; a reading counts when the next one agrees within 500 Hz (`tonegen::counter_agree`). After
    /// `COUNTER_TURN_MS` with nothing, the other chip takes a turn. Each chip's path is asked for with
    /// `Cause::Auto`: the relay never moves (SAFE-20), so a chip whose band would need it just isn't used.
    pub(super) async fn counter_tick(&mut self, now: u32) {
        let Some(mut c) = self.counter else { return };
        let n = c.chip;
        let f = COUNTER_TUNE[n as usize];
        if self.tuned[n as usize].is_none() {
            self.tune_chip(1 - n, None).await;
            self.tune_chip(n, Some((COUNTER_SLOT, f))).await;
            self.select_rx(f, Cause::Auto).await;
            if self.path.rx_now() == Rx::Bk(Sel::rx(f)) {
                self.chip(n).freq_scan(true);
            }
            (c.since, c.last) = (now, None);
        } else {
            let mut bk = self.chip(n);
            if let Some(hz) = bk.freq_scan_result() {
                bk.freq_scan(false);
                let got = hz / 10;
                let rssi = bk.rssi_dbm();
                if tonegen::counter_chip(got) == Some(n) {
                    if let Some(freq) = c.last.and_then(|l| tonegen::counter_agree(l, got)) {
                        self.live.mode = ModeLive::Probe { freq, rssi, tone: 0 };
                        log(b"counter khz ", freq / 100);
                    }
                    c.last = Some(got);
                }
                self.chip(n).freq_scan(true);
                c.since = now;
            } else if now.wrapping_sub(c.since) > COUNTER_TURN_MS {
                bk.freq_scan(false);
                self.live.mode = ModeLive::Probe { freq: 0, rssi: -127, tone: 0 };
                let other = 1 - n;
                if self.has(other) {
                    self.tune_chip(n, None).await;
                    c.chip = other;
                }
                self.tuned[c.chip as usize] = None; // starts its scan on the next tick
            }
        }
        self.counter = Some(c);
    }
}
