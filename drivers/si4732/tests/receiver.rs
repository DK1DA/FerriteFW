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
            0x10 => buf.copy_from_slice(&[0x80, 32, b'6', b'0', 0, 0, 0, 0, b'C']),
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
    si.seek_start(true, &mut d).unwrap();
    assert!(si.tune_poll(&mut d).unwrap().is_some());
    assert_eq!(si.rds_read(&mut d), Err(Error::WrongMode));
    assert_eq!(si.patch_record(&[0; 8], &mut d), Err(Error::WrongMode));
    si.set_property(prop::AM_SEEK_FREQ_SPACING, 9, &mut d).unwrap();
    let w = writes(&si.release().txns);
    assert_eq!(w[0], vec![0x01, 0x11, 0x05]);
    assert_eq!(w[1], vec![0x41, 0x0C, 0, 0, 0, 0], "AM_SEEK_START up, wrap");
    assert!(w.contains(&vec![0x42, 0x01]));
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
    assert_eq!(si.revision(&mut MockDelay::default()).map(|r| r.0), Err(Error::Nack));
}
