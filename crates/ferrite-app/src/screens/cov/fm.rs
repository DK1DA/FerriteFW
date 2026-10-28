//! The FM broadcast screen (chapter 20): playing with stereo and RDS (radio text, or the station name), the
//! monitor off, and the readback.

use super::*;
use ferrite_core::msg::ModeLive;
use ferrite_ui::text::Buf;

fn fm_live(t: &mut T, rt: &[u8], ps: &[u8; 8]) {
    let mut r = [0u8; 64];
    r[..rt.len()].copy_from_slice(rt);
    let f = t.app.fm_freq();
    t.live.mode = ModeLive::Fm { freq: f, ps: *ps, rt: r, rssi: 45, snr: 22, stereo: true, heard: true };
    t.wait(200);
}

/// BCR-2, BCR-30: playing: STEREO, the radio text wrapped, else the station name big; the readback's fm line.
#[test]
fn playing_with_rds() {
    let mut t = T::ready();
    t.open(Dest::FmBroadcast);
    assert!(matches!(t.screen(), Screen::Fm(_)), "{}", t.name());
    fm_live(&mut t, b"Morning show with news at the top of every hour", b"CBC     ");
    assert!(t.app.radio.fm_on, "the reconciler started it");
    t.shows("STEREO");
    t.shows("Morning show with news");
    t.reads("fm");
    t.reads("stereo=1 dbuv=45 snr=22 rds=\"Morning show");
    fm_live(&mut t, b"", b"CBC     ");
    t.shows("CBC");
}

/// BCR-24: with the radio monitor off the screen says the slots aren't heard.
#[test]
fn monitor_off() {
    let mut t = T::ready();
    t.app.set.fm_monitor = false;
    t.open(Dest::FmBroadcast);
    t.shows("Monitor off: slots not heard");
}

/// BCR-1: a frequency on the 10 kHz step shows its second decimal.
#[test]
fn ten_khz_frequencies() {
    let mut b = Buf::<8>::new();
    crate::screens::fm::fm_mhz(10_005, &mut b);
    assert_eq!(b.as_bytes(), b"100.05");
    b.clear();
    crate::screens::fm::fm_mhz(10_050, &mut b);
    assert_eq!(b.as_bytes(), b"100.5");
}
