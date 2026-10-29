//! HF mode on the SI4732.

use super::*;

/// HF-31: the SI4732's AM seek band in HF mode (kHz): HF mode's bottom (150 kHz) to chip 1's start (18 MHz).
const HF_SEEK_KHZ: (u16, u16) = (150, 18_000);

impl Radio {
    /// HF mode on, retuned or off (HF-3, HF-4, HF-10, HF-11): the SI4732 below 18 MHz, chip 1 through the
    /// energised relay from 18 MHz (decision 322, as stock and nicFW). Watch, scan and the other chip pause meanwhile (HF-8).
    pub(super) async fn hf(&mut self, freq: Freq10, mode: u8, on: bool) {
        self.seek_cancel(); // HF-31: a retune or off ends a seek
        if !on {
            if self.hf.take().is_some_and(|h| h.si) {
                let _ = broadcast(&mut self.si, BroadcastCmd::Off, self.si_opts).await;
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
        let si = freq < mhz(18);
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
            let ok = broadcast(&mut self.si, BroadcastCmd::Am(khz), self.si_opts).await.is_ok();
            log(if ok { b"hf si am ok, khz " } else { b"hf si FAILED, khz " }, khz as u32);
            speaker(ok && !self.muted);
        } else {
            if was.is_some_and(|h| h.si) {
                let _ = broadcast(&mut self.si, BroadcastCmd::Off, self.si_opts).await;
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

    /// HF-31: the SI4732's own seek from where it is (AM, below 30 MHz), started here and polled by the tick
    /// ([`Radio::seek_poll`]): the task keeps taking commands meanwhile (MODE-7), and the frequency it stops on is
    /// kept. A seek already running is cancelled first. The seek band is HF mode's SI4732 range ([`HF_SEEK_KHZ`];
    /// the chip's own default, 520-1710 kHz, sent a seek up from 9.4 MHz to 1.71 MHz on the radio, Oct 15, 2026),
    /// and it stops at the band edge rather than wrapping round.
    pub(super) fn hf_seek(&mut self, up: bool) {
        self.seek_cancel();
        if self.hf.is_none_or(|h| !h.si) {
            return;
        }
        let d = &mut BusyDelay;
        let (lo, hi) = HF_SEEK_KHZ;
        // HF-34, HF-35: the seek steps by the band's channel spacing (MW 9 or 10 kHz, LW 9, shortwave 5)
        let space = self.si_opts.am_space(self.hf.map_or(0, |h| (h.freq / 100) as u16));
        if self.si.am_seek_band(lo, hi, d)
            .and_then(|_| self.si.set_property(si4732::prop::AM_SEEK_FREQ_SPACING, space, d))
            .and_then(|_| self.si.seek_start(up, false, d))
            .is_err()
        {
            return;
        }
        let now = now_ms() as u32;
        self.seek = Some(SiSeek { hf: true, t0: now, poll_at: now.wrapping_add(SEEK_POLL_MS) });
        log(b"hf seek start, up ", up as u32);
    }

    /// FM broadcast's seek (BCR-18), the same way: started here, polled by the tick.
    pub(super) async fn bcast_seek(&mut self, up: bool) {
        self.seek_cancel();
        if self.hf.is_some() || broadcast(&mut self.si, BroadcastCmd::Seek { up }, self.si_opts).await.is_err() {
            return log(b"si FAILED, cmd ", 3);
        }
        let now = now_ms() as u32;
        self.seek = Some(SiSeek { hf: false, t0: now, poll_at: now.wrapping_add(SEEK_POLL_MS) });
    }

    /// A seek running is stopped where it is (FM_TUNE_STATUS with CANCEL): any HF or broadcast command, a new seek,
    /// power off.
    pub(super) fn seek_cancel(&mut self) {
        if self.seek.take().is_some() && self.si.mode().is_some() {
            let _ = self.si.seek_cancel(&mut BusyDelay);
            log(b"si seek cancelled", 0);
        }
    }

    /// The tick's turn for a seek (every [`SEEK_POLL_MS`]): done, it is kept as HF mode's frequency or the
    /// broadcast's station; with nothing found in [`SEEK_MAX_MS`] it is cancelled where it got to.
    pub(super) fn seek_poll(&mut self) {
        let Some(sk) = self.seek else { return };
        let now = now_ms() as u32;
        if now.wrapping_sub(sk.poll_at) > u32::MAX / 2 {
            return; // not due
        }
        let d = &mut BusyDelay;
        let found = match self.si.tune_poll(d) {
            Ok(None) if now.wrapping_sub(sk.t0) < SEEK_MAX_MS => {
                self.seek = Some(SiSeek { poll_at: now.wrapping_add(SEEK_POLL_MS), ..sk });
                return;
            }
            Ok(None) => {
                log(b"si seek timed out", 0);
                self.si.seek_cancel(d).ok().map(|t| t.freq)
            }
            Ok(Some(t)) => Some(t.freq),
            Err(_) => None,
        };
        self.seek = None;
        if sk.hf {
            if let (Some(h), Some(khz)) = (self.hf.filter(|h| h.si), found) {
                self.hf = Some(HfOn { freq: khz as Freq10 * 100, ..h });
                log(b"hf seek khz ", khz as u32);
            }
            self.hf_poll();
        } else if let Some(f) = found.filter(|_| self.hf.is_none()) {
            self.fm = Some(FmRx { freq: f, rds: si4732::rds::Rds::init(), poll_at: 0, rds_turn: false });
            if matches!(self.live.mode, ModeLive::Fm { .. }) {
                self.live.mode = ModeLive::Off; // nothing of the last station's RDS carries over
            }
            log(b"si seek ok, 10 khz ", f as u32);
        }
    }
}
