//! HF mode on the SI4732.

use super::*;

impl Radio {
    /// HF mode on, retuned or off (HF-3, HF-4, HF-10, HF-11): the SI4732 below 18 MHz and for AM up to 30 MHz,
    /// chip 1 through the energised relay otherwise. Watch, scan and the other chip pause meanwhile (HF-8).
    pub(super) async fn hf(&mut self, freq: Freq10, mode: u8, on: bool) {
        if !on {
            if self.hf.take().is_some_and(|h| h.si) {
                let _ = broadcast(&mut self.si, BroadcastCmd::Off).await;
            }
            self.tuned = [None; 2];
            self.live.mode = ModeLive::Off;
            let f = self.slots[self.active as usize].freq;
            let mut s = Steps::default();
            let _ = self.path.rx(Rx::Bk(Sel::rx(f)), Cause::HfReceiver, now_ms() as u32, &mut s);
            self.run(&s).await;
            log(b"hf off", 0);
            return;
        }
        let si = freq < mhz(18) || (mode == 2 && freq <= mhz(30));
        let was = self.hf.replace(HfOn { freq, mode, si });
        for n in 0..2u8 {
            if !(n == 1 && !si) {
                self.tune_chip(n, None).await;
            }
        }
        speaker(false);
        let mut s = Steps::default();
        let rx = if si { Rx::Si } else { Rx::Bk(Sel::rx(freq)) };
        let _ = self.path.rx(rx, Cause::HfReceiver, now_ms() as u32, &mut s);
        self.run(&s).await;
        if si {
            // AM only until the SSB patch is imported (HF-24; the UI asks for AM without it).
            let khz = (freq / 100) as u16;
            let ok = broadcast(&mut self.si, BroadcastCmd::Am(khz)).await.is_ok();
            log(if ok { b"hf si am ok, khz " } else { b"hf si FAILED, khz " }, khz as u32);
            speaker(ok && !self.muted);
        } else {
            if was.is_some_and(|h| h.si) {
                let _ = broadcast(&mut self.si, BroadcastCmd::Off).await;
            }
            self.tune_chip(1, Some((HF_SLOT, freq))).await;
            self.chip(1).af_out(AfOut::Normal);
            speaker(!self.muted);
        }
        log(b"hf khz ", freq / 100);
        self.hf_poll();
    }

    pub(super) fn hf_poll(&mut self) {
        let Some(h) = self.hf else { return };
        let (rssi, snr) = if h.si {
            match self.si.rsq(&mut BusyDelay) {
                Ok(q) => (q.rssi as i16 - 107, q.snr), // dBuV to dBm at 50 ohms
                Err(_) => (-127, 0),
            }
        } else {
            (self.chip(1).rssi_dbm(), 0)
        };
        self.live.mode = ModeLive::Hf { freq: h.freq, rssi, snr, si4732: h.si, loading: 255 };
        // The cable log (TERM-28): the HF receiver's level once a second.
        static LOGGED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let now = now_ms() as u32;
        if now.wrapping_sub(LOGGED.load(core::sync::atomic::Ordering::Relaxed)) >= 1000 {
            LOGGED.store(now, core::sync::atomic::Ordering::Relaxed);
            log(b"hf rssi -dbm ", (-(rssi as i32)).max(0) as u32);
            log(b"hf snr db ", snr as u32);
        }
    }

    /// HF-31: the SI4732's own seek from where it is (AM, below 30 MHz); the frequency it stops on is kept.
    pub(super) async fn hf_seek(&mut self, up: bool) {
        let Some(h) = self.hf.filter(|h| h.si) else { return };
        let d = &mut BusyDelay;
        if self.si.seek_start(up, d).is_err() {
            return;
        }
        let t0 = now_ms();
        let found = loop {
            match self.si.tune_poll(d) {
                Ok(Some(t)) => break Some(t.freq),
                Ok(None) if now_ms() - t0 < 15_000 => Timer::after_millis(20).await,
                _ => break None,
            }
        };
        if let Some(khz) = found {
            self.hf = Some(HfOn { freq: khz as Freq10 * 100, ..h });
            log(b"hf seek khz ", khz as u32);
        }
        self.hf_poll();
    }
}
