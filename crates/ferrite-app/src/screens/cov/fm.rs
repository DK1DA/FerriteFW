//! FM broadcast on a broadcast slot (chapter 20; decision 352 retired the FM screen): stereo and RDS on the home
//! card, and the MHz text.

use super::*;
use ferrite_core::msg::ModeLive;
use ferrite_ui::text::Buf;

fn fm_live(t: &mut T, rt: &[u8], ps: &[u8; 8]) {
    let mut r = [0u8; 64];
    r[..rt.len()].copy_from_slice(rt);
    let f = t.app.fm_freq();
    t.live.mode = ModeLive::Fm { freq: f, ps: *ps, rt: r, rssi: 45, snr: 22, stereo: true, pty: 0, heard: true };
    t.wait(200);
}

/// BCR-26.2, BCR-26.3, BCR-30: a broadcast slot playing shows STEREO and the radio text on its card, else the
/// station name.
#[test]
fn playing_with_rds() {
    let mut t = T::ready();
    crate::screens::entry::tune_vfo(&mut t.app, 0, ferrite_core::freq::mhz(100) + 50_000);
    t.wait(200);
    assert_eq!(t.app.bcast_slot(), Some(0));
    fm_live(&mut t, b"Morning show with news at the top of every hour", b"CBC     ");
    assert!(t.app.radio.fm_on, "the reconciler started it");
    t.shows("STEREO");
    t.shows("Morning show");
    fm_live(&mut t, b"", b"CBC     ");
    t.shows("CBC");
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

/// BCR-26.8: the programme type takes the place of "Broadcast" in the playing slot's header, named from RBDS's
/// list in the US and Canada and RDS's elsewhere; "Broadcast" while none is sent. The longest name fits beside a
/// channel number.
#[test]
fn programme_type_in_the_header() {
    use ferrite_core::txplan::Region;
    let mut t = T::ready();
    crate::screens::entry::tune_vfo(&mut t.app, 0, ferrite_core::freq::mhz(100) + 50_000);
    t.wait(200);
    t.app.set.set_region(Some(Region::Canada));
    fm_live(&mut t, b"", b"CHEZ 99 ");
    t.shows("VFO \u{b7} Broadcast");
    let set_pty = |t: &mut T, p: u8| {
        if let ModeLive::Fm { pty, .. } = &mut t.live.mode {
            *pty = p;
        }
        t.wait(200);
    };
    set_pty(&mut t, 6);
    t.shows("VFO \u{b7} Classic rock");
    set_pty(&mut t, 16);
    t.shows("VFO \u{b7} Rhythm & blues");
    t.app.set.set_region(Some(Region::Europe));
    set_pty(&mut t, 6);
    t.shows("VFO \u{b7} Drama");
    set_pty(&mut t, 26);
    t.shows("National music");
    set_pty(&mut t, 0);
    t.shows("VFO \u{b7} Broadcast");
}

/// BCR-26.8: every name fits the header beside a three-digit channel number ("123 · "), both lists.
#[test]
fn programme_type_names_fit() {
    use ferrite_core::txplan::Region;
    for r in [Region::Canada, Region::Europe] {
        for p in 0..32 {
            let n = crate::bcast::pty_name(p, Some(r));
            assert!(n.len() <= 15, "{n}");
        }
    }
}
