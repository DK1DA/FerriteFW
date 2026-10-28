//! The shared confirm (MENU-23): the tone hint's TX question, deleting a range preset, the copy to the VFO, the
//! reset's countdown, and PTT.

use super::*;
use crate::data::Data;
use crate::screens::confirm::{self, Ask, Confirm};

/// TSRCH-16: "Set the TX tone too?" names the tone; on a channel # saves it there, on a VFO there is no #;
/// OK sets TX too, BACK leaves RX only.
#[test]
fn tone_tx() {
    let mut t = T::ready();
    let a = t.app.active;
    t.app.set_channel(a, 0, 0, &t.s);
    confirm::ask(&mut t.app, Ask::ToneTx(885));
    t.draw();
    t.shows("Set the TX tone too?");
    t.shows("T 88.5");
    t.reads("hints \"OK\" \"TX too\" \"#\" \"Save\" \"BACK\" \"RX only\"");
    t.tap(Key::Hash);
    assert_eq!(t.toast(), "Tone saved / in the channel");
    let mut t = T::ready();
    confirm::ask(&mut t.app, Ask::ToneTx(885));
    t.draw();
    t.reads("hints \"OK\" \"TX too\" \"BACK\" \"RX only\"");
    t.tap(Key::Menu);
    assert_eq!(t.app.slot(a).cfg().tx_tone, 885);
    confirm::ask(&mut t.app, Ask::ToneTx(1000));
    t.tap(Key::Exit);
    assert!(t.toast().contains("RX tone set"), "{}", t.toast());
}

/// MENU 0 1 4: deleting a range preset names it, says so, and the list comes back at its top.
#[test]
fn delete_preset() {
    let mut t = T::new();
    t.open(Dest::RangePresets);
    let name = String::from_utf8_lossy(t.s.preset(0).unwrap().name.as_bytes()).into_owned();
    let n = t.s.presets();
    confirm::ask(&mut t.app, Ask::DeletePreset(0));
    t.draw();
    t.shows("Delete this range?");
    t.shows(&name);
    t.tap(Key::Menu);
    assert_eq!(t.toast(), std::format!("Deleted / {name}"));
    assert_eq!(t.s.presets(), n - 1);
    assert!(matches!(t.screen(), Screen::ScanLists(_)), "{}", t.name());
}

/// VFO-23, KEY-23: the copy to the VFO names the channel with its frequency when it fits; OK copies.
#[test]
fn copy_to_vfo() {
    let mut t = T::ready();
    let a = t.app.active;
    // the zone's channel with the shortest name: its frequency fits beside it
    let pos = (0..crate::chan::ChannelStore::zone_len(&t.s, 0)).min_by_key(|&i| crate::chan::ChannelStore::zone_at(&t.s, 0, i).unwrap().name().len()).unwrap();
    t.app.set_channel(a, 0, pos, &t.s);
    let c = t.app.slot(a).ch.unwrap();
    confirm::ask(&mut t.app, Ask::CopyToVfo);
    t.draw();
    t.shows(&String::from_utf8_lossy(ferrite_ui::widgets::freq3(c.rx).as_bytes()));
    t.reads("hints \"OK\" \"Copy\" \"BACK\" \"Cancel\"");
    t.tap(Key::Menu);
    assert!(t.app.slot(a).vfo_mode, "on the VFO now");
    assert_eq!(t.app.slot(a).freq(), c.rx);
}

/// BOOT-14: the armed reset counts down while # is held; OK again does nothing; let go early, nothing goes.
#[test]
fn reset_countdown() {
    let mut t = T::new();
    t.open(Dest::Confirm);
    t.tap(Key::Menu);
    assert!(matches!(t.screen(), Screen::Confirm(Confirm { ask: Ask::Reset { armed: true }, .. })));
    t.tap(Key::Menu); // armed: only hold # resets
    assert!(matches!(t.screen(), Screen::Confirm(Confirm { ask: Ask::Reset { armed: true }, .. })));
    let hash = (0..ferrite_core::keys::CODES as u8).find(|&c| ferrite_core::keys::key(c) == Some(Key::Hash)).unwrap();
    t.app.raw_key(hash, true);
    t.wait(1200);
    t.shows("Resetting in 2 s");
    t.app.raw_key(hash, false);
    t.wait(100);
    assert!(!t.app.want_reset);
    t.shows("Hold # for 3 s to reset");
}

/// PIN-6: PTT transmits from a confirm, but not from the one asked at power on.
#[test]
fn ptt() {
    let mut t = T::ready();
    t.app.open(Dest::Confirm);
    t.app.screen = Screen::Confirm(Confirm { ask: Ask::Reset { armed: false }, boot: true, since: 0 });
    t.ptt(true);
    assert!(matches!(t.screen(), Screen::Confirm(_)), "at power on PTT waits");
    t.ptt(false);
    t.app.screen = Screen::Confirm(Confirm { ask: Ask::Reset { armed: false }, boot: false, since: 0 });
    t.ptt(true);
    assert!(matches!(t.screen(), Screen::Transmit(_)), "{}", t.name());
    t.ptt(false);
}
