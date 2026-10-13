//! Broadcast FM by frequency (manual chapter 20, BCR-21 to BCR-30): a slot in the broadcast band plays on the
//! SI4732, one slot at a time, sharing the FM screen's state.

use super::*;
use crate::bcast::rds_text;
use crate::screens::activity::Mode;
use ferrite_core::freq::mhz;
use ferrite_core::msg::{BroadcastCmd, ModeLive};
use ferrite_core::txplan::Region;

const FM1005: Freq10 = mhz(100) + 50_000;
const FM991: Freq10 = mhz(99) + 10_000;
const TWO_M: Freq10 = mhz(146) + 52_000;

fn ready() -> T {
    let mut t = T::new();
    t.app.radio.tx_built = true;
    t.app.set_region(Some(Region::Us), false);
    t.wait(100);
    t.cmds();
    t
}

fn tune(t: &mut T, s: u8, f: Freq10) -> Vec<RadioCmd> {
    t.app.set_vfo(s, f);
    t.wait(100);
    t.cmds()
}

fn live_fm(t: &mut T, freq: u16, heard: bool) {
    let mut rt = [0u8; 64];
    rt[..9].copy_from_slice(b"Some text");
    t.live.mode = ModeLive::Fm { freq, ps: *b"STATION ", rt, rssi: 40, snr: 20, stereo: true, heard };
}

/// BCR-21, BCR-22: each slot tuned into the band plays broadcast on the SI4732 by itself, and tuned out is a
/// BK4829 slot again.
#[test]
fn each_slot_enters_and_leaves_the_band() {
    for s in 0..3u8 {
        let mut t = ready();
        let c = tune(&mut t, s, FM1005);
        assert_eq!(t.app.bcast_slot(), Some(s));
        assert_eq!(t.app.radio.mode, Mode::Fm);
        assert!(c.contains(&RadioCmd::BcastSlot(Some(s))), "{c:?}");
        assert!(c.contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_050))), "{c:?}");
        let c = tune(&mut t, s, TWO_M);
        assert_eq!(t.app.bcast_slot(), None);
        assert_eq!(t.app.radio.mode, Mode::Off);
        assert!(c.contains(&RadioCmd::Broadcast(BroadcastCmd::Off)) && c.contains(&RadioCmd::BcastSlot(None)), "{c:?}");
    }
}

/// BCR-21: the band's edges, inclusive; the APRS slot is never broadcast (APRS-19).
#[test]
fn band_edges_and_the_aprs_slot() {
    let mut t = ready();
    for (f, inb) in [(mhz(87) + 40_000, false), (mhz(87) + 50_000, true), (mhz(108), true), (mhz(108) + 2_500, false)] {
        tune(&mut t, 0, f);
        assert_eq!(t.app.bcast_slot().is_some(), inb, "{f}");
    }
    tune(&mut t, 0, TWO_M);
    t.app.set.aprs_mode = true;
    tune(&mut t, 2, FM1005);
    assert_eq!(t.app.bcast_slot(), None, "APRS on C");
}

/// BCR-21: region Japan's band is 76-95 MHz.
#[test]
fn japan_band() {
    let mut t = ready();
    t.app.set_region(Some(Region::Japan), false);
    for (f, inb) in [(mhz(80), true), (mhz(90), true), (mhz(95), true), (FM1005, false), (mhz(75), false)] {
        tune(&mut t, 0, f);
        assert_eq!(t.app.bcast_slot().is_some(), inb, "{f}");
    }
    t.app.set_region(Some(Region::Us), false);
    tune(&mut t, 0, mhz(80));
    assert_eq!(t.app.bcast_slot(), None, "80 MHz outside Japan");
}

/// BCR-22: a second slot tuned in takes the broadcast; the first shows "FM on B" until it is tuned out; when the
/// broadcast slot leaves, a slot still in the band takes it back.
#[test]
fn handover_between_slots() {
    let mut t = ready();
    tune(&mut t, 0, FM1005);
    let c = tune(&mut t, 1, FM991);
    assert_eq!(t.app.bcast_slot(), Some(1));
    assert!(c.contains(&RadioCmd::BcastSlot(Some(1))) && c.contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(9_910))), "{c:?}");
    assert!(!c.contains(&RadioCmd::Broadcast(BroadcastCmd::Off)), "one receiver, retuned: {c:?}");
    assert_eq!(t.app.state(0, &t.live).0, "FM on B");
    tune(&mut t, 0, mhz(101)); // A moves within the band: B keeps it
    assert_eq!(t.app.bcast_slot(), Some(1));
    let c = tune(&mut t, 1, TWO_M);
    assert_eq!(t.app.bcast_slot(), Some(0), "A takes it back");
    assert!(c.contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_100))), "{c:?}");
}

/// BCR-24, BCR-26: the card says FM while the broadcast plays, muted while a slot heard has the speaker (the Radio task
/// decides, 2 s after: ferrite-core's watch tests); BCR-15: a slot needing the relay says so.
#[test]
fn card_states() {
    let mut t = ready();
    tune(&mut t, 0, FM1005);
    live_fm(&mut t, 10_050, true);
    assert_eq!(t.app.state(0, &t.live).0, "FM");
    live_fm(&mut t, 10_050, false);
    assert_eq!(t.app.state(0, &t.live).0, "muted");
    tune(&mut t, 1, mhz(50) + 12_500);
    assert_eq!(t.app.state(1, &t.live).0, "Watch off: relay");
}

/// HOME-15: \* on the broadcast slot mutes the broadcast too: the mask goes to the Radio task (which keeps the
/// SI4732 muted) and the card says Muted.
#[test]
fn star_mutes_the_broadcast() {
    let mut t = ready();
    tune(&mut t, 0, FM1005);
    live_fm(&mut t, 10_050, true);
    t.tap(Key::Star);
    assert!(t.cmds().contains(&RadioCmd::Mute(0b001)));
    live_fm(&mut t, 10_050, false);
    assert_eq!(t.app.state(0, &t.live).0, "Muted");
    t.tap(Key::Star);
    assert!(t.cmds().contains(&RadioCmd::Mute(0)));
}

/// BCR-26, BCR-30: the radio text (all 64 characters), else the station name, else nothing.
#[test]
fn rds_text_rule() {
    let mut rt = [0u8; 64];
    assert_eq!(rds_text(b"CBC R1  ", &rt), b"CBC R1");
    rt[..5].copy_from_slice(b"Hello");
    assert_eq!(rds_text(b"CBC R1  ", &rt), b"Hello");
    assert_eq!(rds_text(b"        ", &[0; 64]), b"");
    let full = [b'x'; 64];
    assert_eq!(rds_text(b"CBC R1  ", &full).len(), 64, "all 64 characters");
}

/// BCR-25: on home the knob tunes a broadcast VFO by the FM step (0 3 1 2) and ◀▶ seek; the station sought is
/// taken into the slot as it lands.
#[test]
fn knob_steps_and_seek() {
    let mut t = ready();
    tune(&mut t, 0, mhz(108) + 7_500); // airband: the slot's own step
    for _ in 0..40 {
        if t.app.bcast_slot().is_some() {
            break;
        }
        t.knob(-1);
        t.wait(50);
    }
    assert_eq!(t.app.bcast_slot(), Some(0), "tuned into the band");
    tune(&mut t, 0, mhz(108));
    t.knob(-1);
    t.wait(100);
    assert_eq!(t.freq(), mhz(107) + 90_000, "100 kHz");
    t.cmds();
    t.tap(Key::Right);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Seek { up: true })));
    live_fm(&mut t, 10_650, true);
    t.wait(200);
    assert_eq!(t.freq(), mhz(106) + 50_000, "the sought station");
    assert!(!t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_650))), "not tuned again");
}

/// BCR-25, BCR-27: the FM screen (MENU 0 3 1) shows and tunes the broadcast slot; a preset recalls into it;
/// MENU 0 3 1 2 still reaches the step.
#[test]
fn fm_screen_shares_the_slot() {
    let mut t = ready();
    tune(&mut t, 0, FM1005);
    menu0(&mut t, &[3, 1]);
    assert!(matches!(t.app.screen(), Screen::Fm(f) if !f.menu));
    assert_eq!(t.app.fm_freq(), 10_050);
    t.cmds();
    t.knob(1);
    t.wait(100);
    assert_eq!(t.app.slot(0).freq(), mhz(100) + 60_000, "the slot moved");
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_060))));
    t.hold(Key::Exit);
    assert_eq!(t.app.radio.mode, Mode::Fm, "home: the slot still plays");
    menu0(&mut t, &[3, 1, 2]);
    assert!(matches!(t.app.screen(), Screen::Fm(f) if f.menu && f.row == 1), "{:?}", t.app.screen());
    t.knob(1);
    assert_eq!(t.app.set.fm_step, 2);
}

fn menu0(t: &mut T, path: &[u8]) {
    t.hold(Key::Exit);
    t.tap(Key::Menu);
    t.tap(Key::Digit(0));
    for &d in path {
        t.tap(Key::Digit(d));
    }
}

/// BCR-28: quick settings on a broadcast slot are FM's rows.
#[test]
fn quick_settings_rows() {
    let mut t = ready();
    tune(&mut t, 0, FM1005);
    t.tap(Key::Menu);
    t.tap(Key::Digit(3));
    t.knob(1);
    assert_eq!(t.app.set.fm_agc, 1, "row 3: AGC");
}

/// TX-24, BCR-29: PTT on another slot stops the broadcast before the key-up and starts it again after.
#[test]
fn transmit_switches_the_receiver_away_and_back() {
    let mut t = ready();
    tune(&mut t, 1, FM1005);
    let s = store();
    t.app.key_down(Key::PTT, t.t, &s);
    t.t += 50;
    t.app.tick(t.t, &t.live, &s);
    let c = t.cmds();
    let off = c.iter().position(|c| *c == RadioCmd::Broadcast(BroadcastCmd::Off)).expect("broadcast off");
    let key = c.iter().position(|c| matches!(c, RadioCmd::Transmit { on: true, .. })).expect("keyed");
    assert!(off < key, "{c:?}");
    t.app.key_up(Key::PTT, t.t, &s);
    t.wait(200);
    assert!(t.cmds().contains(&RadioCmd::Broadcast(BroadcastCmd::Fm(10_050))));
    assert_eq!(t.app.radio.mode, Mode::Fm);
}
