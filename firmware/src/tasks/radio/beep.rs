//! Beeps.

use super::*;

impl Radio {
    /// SND-11: a local beep from a BK4829's tone generator, as stock does it (0x0801EB8C, 0x0801EB4C): the chip
    /// leaves receive for the beep (REG_30 0x0302: no PLL, no PA) and comes back. It takes the chip that disturbs
    /// least: one nobody is hearing, a sleeping one before a listening one; a heard chip only if both are heard.
    /// Never while keyed. The task waits through it (at most `tonegen::BEEP_MAX_MS`).
    pub(super) async fn beep(&mut self, hz: u16, ms: u16) {
        if self.path.keyed().is_some() || hz == 0 {
            return;
        }
        // As stock (V0.27, Oct 12, 2026 trace): key beeps come from the MCU's DAC1 (PA4) into the speaker amp, not
        // a BK4829's tone generator (taking a receiving chip into tone mode and back squawked like a squelch break
        // on the radio). The chips keep receiving; the one passing audio is only muted for the beep (REG_47), and
        // the audio decision unmutes it on the next tick. Stock's order: DAC on, 30 ms, amp on; amp off, 30 ms,
        // DAC off.
        // SND-13: with audio already playing (a slot heard, the broadcast, HF), key and refusal beeps are skipped:
        // muting the chip and switching the amp off cut the audio and sounded rough on the radio (Oct 13, 2026).
        // The alert tone still sounds: the radio audio (chips and SI4732) is muted under it and put back after,
        // the amp left on.
        let playing = SPEAKER_ON.load(core::sync::atomic::Ordering::Relaxed);
        if playing && hz != ferrite_app::ALERT_HZ {
            return;
        }
        for n in 0..2u8 {
            if self.has(n) && self.tuned[n as usize].is_some() {
                self.chip(n).af_out(AfOut::Mute);
            }
        }
        // The SI4732 playing: the broadcast heard, or HF mode on the SI4732 (`hf`).
        let si_playing = playing && self.si.mode().is_some() && self.hf.map_or(self.si_heard, |h| h.si);
        if si_playing {
            let _ = self.si.mute(true, &mut BusyDelay);
        }
        crate::hal::tone::start(hz as u32, self.beep_level);
        Timer::after_millis(30).await;
        if !playing {
            speaker(true);
        }
        Timer::after_millis(ms.min(tonegen::BEEP_MAX_MS) as u64).await;
        if !playing {
            speaker(false);
        }
        Timer::after_millis(30).await;
        crate::hal::tone::stop();
        if si_playing {
            let _ = self.si.mute(false, &mut BusyDelay);
        }
        // The receive tick puts each chip's audio back; HF mode on chip 1 has no tick doing it (`hf`).
        if playing && self.hf.is_some_and(|h| !h.si) {
            self.chip(1).af_out(AfOut::Normal);
        }
        log(b"beep hz ", hz as u32);
    }

    /// APRS-6.1: a message sound, note by note from DAC1 with the amp on throughout (no amp click between notes).
    /// It sounds over received audio as the alert tone does (SND-13): the chips' and the SI4732's audio are muted
    /// under it and come back after. Never while keyed.
    pub(super) async fn chime(&mut self, i: u8, level: u8) {
        let packet = i == tonegen::CHIME_PACKET;
        let Some(notes) = tonegen::CHIMES.get(i as usize).filter(|n| packet || n[0].1 != 0) else { return };
        if self.path.keyed().is_some() {
            return;
        }
        let playing = SPEAKER_ON.load(core::sync::atomic::Ordering::Relaxed);
        for n in 0..2u8 {
            if self.has(n) && self.tuned[n as usize].is_some() {
                self.chip(n).af_out(AfOut::Mute);
            }
        }
        let si_playing = playing && self.si.mode().is_some() && self.hf.map_or(self.si_heard, |h| h.si);
        if si_playing {
            let _ = self.si.mute(true, &mut BusyDelay);
        }
        let level = level.clamp(1, 11); // 0 7 5 6, its own volume
        crate::hal::tone::start(if packet { 1200 } else { notes[0].0.max(1) as u32 }, level);
        Timer::after_millis(30).await;
        if !playing {
            speaker(true);
        }
        if packet {
            // APRS-6.1: Packet, a fake 1200 Bd burst, the bits 1 ms each
            for _ in 0..tonegen::PACKET_REPEAT {
                for &(hz, bits) in tonegen::PACKET.iter() {
                    crate::hal::tone::start(hz as u32, level);
                    Timer::after_micros(bits as u64 * 1000).await;
                }
            }
        }
        for &(hz, ms) in notes.iter().take_while(|n| n.1 != 0) {
            if hz == 0 {
                crate::hal::tone::stop();
            } else {
                crate::hal::tone::start(hz as u32, level);
            }
            Timer::after_millis(ms as u64).await;
        }
        if !playing {
            speaker(false);
        }
        Timer::after_millis(30).await;
        crate::hal::tone::stop();
        if si_playing {
            let _ = self.si.mute(false, &mut BusyDelay);
        }
        if playing && self.hf.is_some_and(|h| !h.si) {
            self.chip(1).af_out(AfOut::Normal);
        }
        log(b"chime ", i as u32);
    }
}
