//! The frequency counter and tone search (chapter 22).

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

    /// Tone search on or off (TSRCH-1, TSRCH-3): an exclusive mode like the counter; watch and scans pause. Off,
    /// the engines tune the chips again, so the slot's own tone detector is back (`tune_finish`).
    pub(super) fn tone_search(&mut self, slot: u8, on: bool) {
        self.tuned = [None; 2]; // tuned below, or by the engines afterwards
        let slot = slot.min(SLOTS as u8 - 1);
        self.tsearch = on.then_some((slot, ferrite_core::tonesearch::Search::new()));
        let f = self.slots[slot as usize].rx_freq();
        self.live.mode = if on { ModeLive::Probe { freq: f, rssi: -127, tone: 0 } } else { ModeLive::Off };
        speaker(false);
        log(b"tone search ", on as u32);
    }

    /// Tone search's tick: the slot's own chip on its frequency (its band's, as the watch picks), the other asleep,
    /// the sub-tone detectors in scan (`tone_probe`, the tone hint's, TSRCH-12); each tick's reading goes to
    /// `tonesearch::Search`, which keeps one when the next agrees with the carrier there. The audio stays muted.
    /// The slot's card follows its squelch and signal (TSRCH-2).
    pub(super) async fn tsearch_tick(&mut self) {
        let Some((s, mut srch)) = self.tsearch else { return };
        let a = self.save.tick(now_ms(), true); // awake throughout, as the counter
        self.apply(a).await;
        let f = self.slots[s as usize].rx_freq();
        let n = ferrite_core::freq::band(f).1;
        if !self.has(n) {
            return;
        }
        if self.tuned[n as usize] != Some((s, f)) {
            self.tune_chip(1 - n, None).await;
            self.tune_chip(n, Some((s, f))).await;
            self.select_rx(f, Cause::ActiveSlot).await;
            self.chip(n).af_out(AfOut::Mute);
            radio_chip::tone_probe(&mut self.chip(n));
            return;
        }
        let mut bk = self.chip(n);
        let (rssi, st) = (bk.rssi_dbm(), bk.status());
        let (c, d) = bk.subtone_scan();
        let word = radio_chip::heard_tone(c, d, &ferrite_app::slot::CTCSS, &ferrite_app::slot::DCS);
        if srch.step(st.squelch_open, word) {
            log(b"tone found ", srch.found as u32);
        }
        let sl = &mut self.live.slots[s as usize];
        (sl.rssi, sl.squelch_open) = (if st.squelch_open { rssi } else { rssi.min(-121) }, st.squelch_open);
        self.live.mode = ModeLive::Probe { freq: f, rssi, tone: srch.found };
        self.tsearch = Some((s, srch));
    }
}
