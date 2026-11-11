//! The driver against a simulated SI4732: the bytes it sends (against AN332 and stock's SSB bring-up), CTS,
//! errors, tuning and seeking through polls, RDS.

use si4732::{prop, Error, Mode, Rsq, Si4732, Sideband, SsbConfig, Tune, ADDR};
use uradio_bus::mock::{I2cTxn, MockDelay, MockI2c};
use uradio_bus::Nack;

/// A model: answers status reads with CTS after `cts_after` polls, flags STC after `stc_after` GET_INT_STATUS
/// calls, and gives fixed replies to status commands.
#[allow(clippy::type_complexity)]
fn chip(cts_after: u32, stc_after: u32, err_on: Option<u8>) -> MockI2c<impl FnMut(&[I2cTxn], &mut [u8]) -> Result<(), Nack>> {
    let (mut polls, mut ints) = (0u32, 0u32);
    // Which command the clear-to-send has been given for (the index of its write).
    let mut answered = usize::MAX;
    MockI2c::new(move |txns, buf| {
        let Some((at, cmd)) = txns.iter().enumerate().rev().find_map(|(i, t)| if let I2cTxn::Write(_, d) = t { Some((i, d.clone())) } else { None }) else {
            return Err(Nack);
        };
        if answered != at {
            // Reads before CTS are status polls; power-up is polled with `ready` straight away.
            assert_eq!(buf.len(), 1, "a reply read before CTS");
            polls += 1;
            let cts = polls > cts_after;
            buf[0] = if cts { 0x80 } else { 0 } | if cts && err_on == Some(cmd[0]) { 0x40 } else { 0 };
            if cts {
                polls = 0;
                answered = at;
            }
            return Ok(());
        }
        buf.fill(0);
        buf[0] = 0x80;
        match cmd[0] {
            0x10 => buf.copy_from_slice(&[0x80, 32, b'6', b'0', 0x12, 0x34, b'3', b'0', b'C']),
            0x14 => {
                ints += 1;
                buf[0] = 0x80 | (ints > stc_after) as u8;
            }
            0x22 | 0x42 => buf[..8].copy_from_slice(&[0x80, 0x01, 0x27, 0x10, 45, 20, 0, 0]),
            0x23 | 0x43 => buf[..6].copy_from_slice(&[0x80, 0, 0x01, 0, 30, 12]),
            0x24 if cmd[1] == 0x04 => buf[3] = 1,
            0x24 => buf.copy_from_slice(&[0x80, 0, 0, 1, 0x12, 0x34, 0x00, 0x02, 0xAB, 0xCD, 0x41, 0x42, 0x00]),
            _ => {}
        }
        Ok(())
    })
}

fn writes(t: &[I2cTxn]) -> Vec<Vec<u8>> {
    t.iter().filter_map(|t| if let I2cTxn::Write(a, d) = t { assert_eq!(*a, ADDR); Some(d.clone()) } else { None }).collect()
}

#[test]
fn ssb_bring_up_matches_stock() {
    let mut si = Si4732::new(chip(0, 0, None), ADDR);
    let mut d = MockDelay::default();
    si.power_up_start(Mode::Ssb).unwrap();
    assert!(si.ready().unwrap());
    let record = [0x15, 0x00, 0x0F, 0xE0, 0xF2, 0x73, 0x76, 0x2F]; // any 8 bytes: the patch is not ours
    si.patch_record(&record, &mut d).unwrap();
    // Stock: audio bandwidth, cut-off, AVC divider, AVC on, soft mute select, AFC off.
    let cfg = SsbConfig { audio_bw: 1, sideband_cutoff: 1, avc_divider: 0, avc: true, soft_mute_snr: false, afc_off: true };
    si.ssb(cfg, -120, &mut d).unwrap();
    si.tune_start(7074, Sideband::Usb, &mut d).unwrap();
    let w = writes(&si.release().txns);
    assert_eq!(w[0], vec![0x01, 0x31, 0x05], "POWER_UP: patch, crystal, AM function, analog out (stock's bytes)");
    assert_eq!(w[1], record.to_vec());
    assert_eq!(w[2], vec![0x12, 0, 0x01, 0x01, 0x90, 0x11], "SSB_MODE");
    assert_eq!(w[3], vec![0x12, 0, 0x01, 0x00, 0xFF, 0x88], "SSB_BFO -120 Hz");
    assert_eq!(w[4], vec![0x40, 0x80, 0x1B, 0xA2, 0, 0], "SSB_TUNE_FREQ 7074 kHz USB");
}

#[test]
fn fm_tune_by_polling_and_rds() {
    let mut si = Si4732::new(chip(3, 4, None), ADDR);
    let mut d = MockDelay::default();
    si.power_up_start(Mode::Fm).unwrap();
    while !si.ready().unwrap() {}
    si.rds_on(&mut d).unwrap();
    si.tune_start(10010, Sideband::Usb, &mut d).unwrap(); // 100.10 MHz
    let mut polls = 0;
    let t = loop {
        if let Some(t) = si.tune_poll(&mut d).unwrap() {
            break t;
        }
        polls += 1;
    };
    assert_eq!(polls, 4);
    assert_eq!(t, Tune { freq: 0x2710, rssi: 45, snr: 20, valid: true });
    assert_eq!(si.rsq(&mut d).unwrap(), Rsq { rssi: 30, snr: 12, valid: true, stereo: false });
    let g = si.rds_read(&mut d).unwrap().unwrap();
    assert_eq!(g.blocks, [0x1234, 0x0002, 0xABCD, 0x4142]);
    let w = writes(&si.release().txns);
    assert_eq!(w[0], vec![0x01, 0x10, 0x05]);
    assert!(w.contains(&vec![0x12, 0, 0x15, 0x02, 0xAA, 0x01]), "RDS config");
    assert!(w.contains(&vec![0x20, 0, 0x27, 0x1A, 0]), "FM_TUNE_FREQ 100.10 MHz");
    assert!(w.contains(&vec![0x22, 0x01]), "TUNE_STATUS with INTACK");
    // Every CTS wait was a few 20 us polls: nothing slept for long.
    assert!(d.total_us < 2_000, "{} us", d.total_us);
}

#[test]
fn am_seek_and_band_checks() {
    let mut si = Si4732::new(chip(0, 0, None), ADDR);
    let mut d = MockDelay::default();
    assert_eq!(si.tune_start(1000, Sideband::Usb, &mut d), Err(Error::WrongMode));
    si.power_up_start(Mode::Am).unwrap();
    assert_eq!(si.tune_start(100, Sideband::Usb, &mut d), Err(Error::OutOfBand));
    assert_eq!(si.tune_start(31000, Sideband::Usb, &mut d), Err(Error::OutOfBand));
    assert_eq!(si.am_seek_band(100, 30000, &mut d), Err(Error::OutOfBand));
    assert_eq!(si.am_seek_band(150, 30001, &mut d), Err(Error::OutOfBand));
    si.am_seek_band(150, 30000, &mut d).unwrap();
    si.seek_start(true, true, &mut d).unwrap();
    si.seek_start(true, false, &mut d).unwrap();
    si.seek_start(false, false, &mut d).unwrap();
    assert!(si.tune_poll(&mut d).unwrap().is_some());
    assert_eq!(si.rds_read(&mut d), Err(Error::WrongMode));
    assert_eq!(si.patch_record(&[0; 8], &mut d), Err(Error::WrongMode));
    si.set_property(prop::AM_SEEK_FREQ_SPACING, 9, &mut d).unwrap();
    let w = writes(&si.release().txns);
    assert_eq!(w[0], vec![0x01, 0x11, 0x05]);
    assert_eq!(w[1], vec![0x12, 0, 0x34, 0x00, 0x00, 150], "AM_SEEK_BAND_BOTTOM 150 kHz");
    assert_eq!(w[2], vec![0x12, 0, 0x34, 0x01, 0x75, 0x30], "AM_SEEK_BAND_TOP 30000 kHz");
    assert_eq!(w[3], vec![0x41, 0x0C, 0, 0, 0, 0], "AM_SEEK_START up, wrap");
    assert_eq!(w[4], vec![0x41, 0x08, 0, 0, 0, 0], "HF-31: up, stopping at the band edge (no WRAP)");
    assert_eq!(w[5], vec![0x41, 0x00, 0, 0, 0, 0], "down, no WRAP");
    assert!(w.contains(&vec![0x42, 0x01]));
}

/// HF-31, MODE-7.1: a seek stopped part way (a new command in HF or FM broadcast) leaves the receiver where it got
/// to.
#[test]
fn seek_cancel_sends_cancel_with_intack() {
    let mut si = Si4732::new(chip(0, u32::MAX, None), ADDR);
    let mut d = MockDelay::default();
    assert_eq!(si.seek_cancel(&mut d), Err(Error::WrongMode));
    si.power_up_start(Mode::Am).unwrap();
    si.seek_start(false, true, &mut d).unwrap();
    assert_eq!(si.tune_poll(&mut d).unwrap(), None);
    assert_eq!(si.seek_cancel(&mut d).unwrap().freq, 0x2710);
    let w = writes(&si.release().txns);
    assert_eq!(w.last(), Some(&vec![0x42, 0x03]), "AM_TUNE_STATUS CANCEL | INTACK");
}

#[test]
fn errors_and_timeouts() {
    // The chip flags SET_PROPERTY.
    let mut si = Si4732::new(chip(0, 0, Some(0x12)), ADDR);
    let mut d = MockDelay::default();
    si.power_up_start(Mode::Fm).unwrap();
    assert_eq!(si.volume(40, &mut d), Err(Error::Rejected));
    // No CTS within 10 ms.
    let mut si = Si4732::new(chip(u32::MAX, 0, None), ADDR);
    let mut d = MockDelay::default();
    si.power_up_start(Mode::Fm).unwrap();
    assert_eq!(si.volume(40, &mut d), Err(Error::Timeout));
    assert_eq!(d.total_us, 10_000);
    // Nobody there.
    let mut si = Si4732::new(MockI2c::new(|_, _| Err(Nack)), ADDR);
    assert_eq!(si.ready(), Err(Error::Nack));
    assert_eq!(si.revision(&mut MockDelay::default()).map(|r| r.part), Err(Error::Nack));
}

/// Passes through to `inner` until its writes or reads run out, then NACKs (a chip that drops off the bus part way).
struct Flaky<I> {
    inner: I,
    writes: usize,
    reads: usize,
}

impl<I: uradio_bus::I2c> uradio_bus::I2c for Flaky<I> {
    fn write(&mut self, addr: u8, data: &[u8]) -> Result<(), Nack> {
        if self.writes == 0 {
            return Err(Nack);
        }
        self.writes -= 1;
        self.inner.write(addr, data)
    }
    fn read(&mut self, addr: u8, buf: &mut [u8]) -> Result<(), Nack> {
        if self.reads == 0 {
            return Err(Nack);
        }
        self.reads -= 1;
        self.inner.read(addr, buf)
    }
}

#[allow(clippy::type_complexity)]
fn flaky(writes: usize, reads: usize) -> Flaky<MockI2c<impl FnMut(&[I2cTxn], &mut [u8]) -> Result<(), Nack>>> {
    Flaky { inner: chip(0, 0, None), writes, reads }
}

/// GET_REV (AN332): part number in RESP1, firmware major and minor in RESP2-3, the patch in RESP4-5, the
/// component in RESP6-7, chip revision in RESP8.
#[test]
fn revision_volume_mute() {
    let mut si = Si4732::new(chip(0, 0, None), ADDR);
    let mut d = MockDelay::default();
    assert_eq!(si.mode(), None);
    assert_eq!(si.revision(&mut d), Ok(si4732::Rev { part: 32, fw: *b"60", patch: 0x1234, cmp: *b"30", chip: b'C' }));
    si.volume(80, &mut d).unwrap(); // held to 63
    si.mute(true, &mut d).unwrap();
    si.mute(false, &mut d).unwrap();
    si.power_down(&mut d).unwrap();
    assert_eq!(si.mode(), None);
    let t = si.release().txns;
    let w = writes(&t);
    assert_eq!(w[0], vec![0x10]);
    assert_eq!(w[1], vec![0x12, 0, 0x40, 0x00, 0, 63], "RX_VOLUME 63");
    assert_eq!(w[2], vec![0x12, 0, 0x40, 0x01, 0, 3], "RX_HARD_MUTE both channels");
    assert_eq!(w[3], vec![0x12, 0, 0x40, 0x01, 0, 0]);
    assert_eq!(w[4], vec![0x11], "POWER_DOWN");
    // GET_REV's reply is read whole (status and 8 bytes) once CTS is seen; the others have no reply read.
    assert_eq!(t[1..3], [I2cTxn::Read(ADDR, 1), I2cTxn::Read(ADDR, 9)]);
    assert_eq!(t.iter().filter(|t| matches!(t, I2cTxn::Read(_, n) if *n > 1)).count(), 1);
}

/// AM_TUNE_FREQ has ANTCAP (two bytes) after the frequency; SSB's ARG1 bits 7:6 pick the sideband (1 = LSB).
#[test]
fn am_and_lsb_tune_bytes() {
    let mut si = Si4732::new(chip(0, 0, None), ADDR);
    let mut d = MockDelay::default();
    si.power_up_start(Mode::Am).unwrap();
    assert_eq!(si.mode(), Some(Mode::Am));
    si.tune_start(149, Sideband::Lsb, &mut d).unwrap();
    si.tune_start(30000, Sideband::Usb, &mut d).unwrap();
    si.power_down(&mut d).unwrap();
    si.power_up_start(Mode::Ssb).unwrap();
    si.tune_start(3700, Sideband::Lsb, &mut d).unwrap();
    // FM band edges: 64.00 and 108.00 MHz in, just outside them out.
    si.power_down(&mut d).unwrap();
    si.power_up_start(Mode::Fm).unwrap();
    assert_eq!(si.tune_start(6399, Sideband::Usb, &mut d), Err(Error::OutOfBand));
    assert_eq!(si.tune_start(10801, Sideband::Usb, &mut d), Err(Error::OutOfBand));
    si.tune_start(6400, Sideband::Usb, &mut d).unwrap();
    si.tune_start(10800, Sideband::Usb, &mut d).unwrap();
    let w = writes(&si.release().txns);
    assert_eq!(w[1], vec![0x40, 0, 0, 149, 0, 0], "AM_TUNE_FREQ 149 kHz (sideband ignored)");
    assert_eq!(w[2], vec![0x40, 0, 0x75, 0x30, 0, 0], "AM_TUNE_FREQ 30000 kHz");
    assert_eq!(w[4], vec![0x01, 0x31, 0x05]);
    assert_eq!(w[5], vec![0x40, 0x40, 0x0E, 0x74, 0, 0], "SSB_TUNE_FREQ 3700 kHz LSB");
    assert_eq!(w[8], vec![0x20, 0, 0x19, 0x00, 0]);
    assert_eq!(w[9], vec![0x20, 0, 0x2A, 0x30, 0]);
    assert_eq!(w.len(), 10);
}

/// FM_SEEK_START has one argument (SEEKUP, WRAP); SSB can't seek; the AM seek band only in AM; SSB settings only
/// in SSB; RDS only in FM.
#[test]
fn fm_seek_and_wrong_modes() {
    let mut si = Si4732::new(chip(0, 0, None), ADDR);
    let mut d = MockDelay::default();
    let cfg = SsbConfig { audio_bw: 0, sideband_cutoff: 0, avc_divider: 0, avc: false, soft_mute_snr: false, afc_off: false };
    assert_eq!(si.seek_start(true, true, &mut d), Err(Error::WrongMode));
    assert_eq!(si.rsq(&mut d), Err(Error::WrongMode));
    si.power_up_start(Mode::Fm).unwrap();
    si.seek_start(false, true, &mut d).unwrap();
    si.seek_start(true, false, &mut d).unwrap();
    assert_eq!(si.am_seek_band(520, 1710, &mut d), Err(Error::WrongMode));
    assert_eq!(si.ssb(cfg, 0, &mut d), Err(Error::WrongMode));
    si.power_down(&mut d).unwrap();
    si.power_up_start(Mode::Ssb).unwrap();
    assert_eq!(si.seek_start(true, true, &mut d), Err(Error::WrongMode));
    assert_eq!(si.rds_on(&mut d), Err(Error::WrongMode));
    // SSB_MODE all fields at their widest: each masked to its 4 bits.
    let all = SsbConfig { audio_bw: 0xFF, sideband_cutoff: 0xFF, avc_divider: 0xFF, avc: true, soft_mute_snr: true, afc_off: true };
    assert_eq!(all.bits(), 0xBFFF);
    assert_eq!(cfg.bits(), 0);
    let w = writes(&si.release().txns);
    assert_eq!(w[1], vec![0x21, 0x04], "FM_SEEK_START down, wrap");
    assert_eq!(w[2], vec![0x21, 0x08], "FM_SEEK_START up, no wrap");
    assert_eq!(w.len(), 5, "nothing sent for the refused calls");
}

/// FM_RDS_STATUS (AN332): STATUSONLY (0x04) asks without popping; an empty FIFO pops nothing. MTFIFO | INTACK
/// (0x03) empties it.
#[test]
fn rds_empty_fifo_and_flush() {
    let mut si = Si4732::new(MockI2c::new(|_, buf| {
        buf.fill(0);
        buf[0] = 0x80;
        Ok(())
    }), ADDR);
    let mut d = MockDelay::default();
    si.power_up_start(Mode::Fm).unwrap();
    assert_eq!(si.rds_read(&mut d), Ok(None));
    si.rds_flush(&mut d).unwrap();
    let t = si.release().txns;
    let w = writes(&t);
    assert_eq!(w[1..], [vec![0x24, 0x04], vec![0x24, 0x03]]);
    // The status-only reply is read as 4 bytes (status, RDSINT, sync, FIFO used); the flush's as a whole group.
    assert!(t.contains(&I2cTxn::Read(ADDR, 4)) && t.contains(&I2cTxn::Read(ADDR, 13)));
}

/// ready() during power-up: not yet, then CTS; CTS with ERR is a rejected power-up.
#[test]
fn ready_reports_rejection() {
    let mut si = Si4732::new(chip(1, 0, Some(0x01)), ADDR);
    si.power_up_start(Mode::Fm).unwrap();
    assert_eq!(si.ready(), Ok(false));
    assert_eq!(si.ready(), Err(Error::Rejected));
}

/// A chip that stops answering part way: every call passes the NACK on, at whichever step it comes.
#[test]
fn nack_at_each_step() {
    let mut d = MockDelay::default();
    let cfg = SsbConfig { audio_bw: 1, sideband_cutoff: 1, avc_divider: 0, avc: true, soft_mute_snr: false, afc_off: true };
    // Power-up not acknowledged: the mode is not taken.
    let mut si = Si4732::new(flaky(0, 9), ADDR);
    assert_eq!(si.power_up_start(Mode::Fm), Err(Error::Nack));
    assert_eq!(si.mode(), None);
    // The reply read after CTS.
    let mut si = Si4732::new(flaky(9, 1), ADDR);
    assert_eq!(si.revision(&mut d), Err(Error::Nack));
    // Each of RDS's three properties.
    for n in [1, 2, 3] {
        let mut si = Si4732::new(flaky(n, 99), ADDR);
        si.power_up_start(Mode::Fm).unwrap();
        assert_eq!(si.rds_on(&mut d), Err(Error::Nack), "write {n}");
    }
    // RDS: the status-only ask, then the pop.
    for n in [1, 2] {
        let mut si = Si4732::new(flaky(n, 99), ADDR);
        si.power_up_start(Mode::Fm).unwrap();
        assert_eq!(si.rds_read(&mut d), Err(Error::Nack), "write {n}");
    }
    let mut si = Si4732::new(flaky(1, 99), ADDR);
    si.power_up_start(Mode::Fm).unwrap();
    assert_eq!(si.rsq(&mut d), Err(Error::Nack));
    // SSB: SSB_MODE, then SSB_BFO.
    for n in [1, 2] {
        let mut si = Si4732::new(flaky(n, 99), ADDR);
        si.power_up_start(Mode::Ssb).unwrap();
        assert_eq!(si.ssb(cfg, 0, &mut d), Err(Error::Nack), "write {n}");
    }
    // AM seek band: bottom, then top.
    for n in [1, 2] {
        let mut si = Si4732::new(flaky(n, 99), ADDR);
        si.power_up_start(Mode::Am).unwrap();
        assert_eq!(si.am_seek_band(520, 1710, &mut d), Err(Error::Nack), "write {n}");
    }
    // Tune poll: GET_INT_STATUS, then TUNE_STATUS once STC is set; seek cancel.
    for n in [1, 2] {
        let mut si = Si4732::new(flaky(n, 99), ADDR);
        si.power_up_start(Mode::Am).unwrap();
        assert_eq!(si.tune_poll(&mut d), Err(Error::Nack), "write {n}");
    }
    let mut si = Si4732::new(flaky(1, 99), ADDR);
    si.power_up_start(Mode::Am).unwrap();
    assert_eq!(si.seek_cancel(&mut d), Err(Error::Nack));
}

/// STC set with the chip powered down (no mode): the result can't be asked for, and nothing more is sent.
#[test]
fn tune_poll_without_a_mode() {
    let mut si = Si4732::new(chip(0, 0, None), ADDR);
    let mut d = MockDelay::default();
    assert_eq!(si.tune_poll(&mut d), Err(Error::WrongMode));
    assert_eq!(writes(&si.release().txns), vec![vec![0x14]]);
}

/// HF-35, BCR-32 (decision 292): the gain override is FM_AGC_OVERRIDE (0x28) in FM and AM_AGC_OVERRIDE (0x48) in
/// AM, AGC off with the index, or AGC on.
#[test]
fn agc_override_by_mode() {
    let mut si = Si4732::new(chip(0, 0, None), ADDR);
    let mut d = MockDelay::default();
    assert_eq!(si.agc(Some(3), &mut d), Err(Error::WrongMode));
    si.power_up_start(Mode::Fm).unwrap();
    si.agc(Some(26), &mut d).unwrap();
    si.agc(None, &mut d).unwrap();
    si.power_down(&mut d).unwrap();
    si.power_up_start(Mode::Am).unwrap();
    si.agc(Some(12), &mut d).unwrap();
    let w = writes(&si.release().txns);
    assert_eq!(w[1], vec![0x28, 1, 26], "FM_AGC_OVERRIDE, fixed index 26");
    assert_eq!(w[2], vec![0x28, 0, 0], "FM AGC on");
    assert_eq!(w[5], vec![0x48, 1, 12], "AM_AGC_OVERRIDE, fixed index 12");
}
