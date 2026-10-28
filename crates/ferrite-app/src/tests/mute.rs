//! Quick mute (HOME-15): \* on home mutes or unmutes the active slot; the slot still receives.

use super::*;
use crate::persist::{Persist, Sink};

/// The App's commands of one kind: the quick mute masks sent.
fn mutes(t: &mut T) -> Vec<u8> {
    t.cmds().into_iter().filter_map(|c| if let RadioCmd::Mute(m) = c { Some(m) } else { None }).collect()
}

/// HOME-15, HOME-15.1: \* mutes the active slot, \* again turns it off and a third \* on again; the mask goes to
/// the Radio task; the state word says Muted, or RX muted while it receives (its S-meter and squelch carry on); the
/// mute stays with its slot across SLOT, each slot its own.
#[test]
fn star_mutes_the_active_slot() {
    let mut t = T::new();
    t.app.set.backlight = 0;
    t.tap(Key::Star);
    assert_eq!(mutes(&mut t), [0b001]);
    assert_eq!(t.app.state(0, &t.live), ("Muted", ferrite_ui::gfx::AMBER));
    t.live.slots[0].squelch_open = true;
    t.live.slots[0].rssi = -80;
    t.wait(100);
    assert_eq!(t.app.state(0, &t.live).0, "RX muted", "still receives");
    t.tap(Key::Slot);
    assert_eq!(t.app.active(), 1);
    assert_eq!(t.app.state(0, &t.live).0, "RX muted", "A keeps its mute");
    assert_eq!(t.app.state(1, &t.live).0, "idle");
    t.tap(Key::Star);
    assert_eq!(mutes(&mut t), [0b011], "B muted too");
    t.tap(Key::Slot);
    t.tap(Key::Slot);
    t.tap(Key::Star);
    assert_eq!(mutes(&mut t), [0b010], "A off (no longer muted), B still muted");
    assert_eq!(t.app.state(0, &t.live).0, "Off");
    t.tap(Key::Star);
    assert_eq!(mutes(&mut t), [0b010], "A on again");
    assert_eq!(t.app.state(0, &t.live).0, "RX");
}

/// HOME-15: RAM only: nothing saved changes, and a new App (a power cycle) starts with nothing muted.
#[test]
fn mute_is_never_saved() {
    struct V(Vec<u8>);
    impl Sink for V {
        fn put(&mut self, b: &[u8]) {
            self.0.extend_from_slice(b);
        }
    }
    let mut t = T::new();
    let save = |t: &T| {
        let mut v = V(Vec::new());
        t.app.save_state(&mut v);
        t.app.settings().save(&mut v);
        v.0
    };
    let before = save(&t);
    t.tap(Key::Star);
    assert_eq!(save(&t), before);
    let t = T::new();
    assert_eq!(t.app.state(0, &t.live).0, "idle");
}

/// KEY-18, SCAN-11, HOME-17: \* on a scanning slot still skips the hit (no mute), and hold \* there does nothing
/// (no quick menu); off the scan, hold \* opens the quick menu, which mutes nothing either.
#[test]
fn star_keeps_skip_and_hold_star_is_the_quick_menu() {
    let mut t = T::new();
    t.live.slots[0].scanning = true;
    t.live.slots[0].squelch_open = true;
    t.wait(100);
    t.cmds();
    t.tap(Key::Star);
    let c = t.cmds();
    assert!(c.contains(&RadioCmd::ScanSkip { slot: 0 }), "{c:?}");
    assert!(!c.iter().any(|c| matches!(c, RadioCmd::Mute(_))), "{c:?}");
    let rev = t.app.slot(0).cfg().reverse;
    t.hold(Key::Star);
    assert!(t.app.qmenu.is_none(), "no quick menu while scanning");
    assert_eq!(t.app.slot(0).cfg().reverse, rev, "and no reverse");
    assert!(!t.cmds().iter().any(|c| matches!(c, RadioCmd::ScanSkip { .. })), "a hold skips nothing");
    t.live.slots[0].scanning = false;
    t.live.slots[0].squelch_open = false;
    t.wait(100);
    t.hold(Key::Star);
    assert!(t.app.qmenu.is_some(), "the quick menu");
    assert_eq!(t.app.slot(0).cfg().reverse, rev, "hold * alone doesn't toggle reverse any more");
    assert!(mutes(&mut t).is_empty());
}

/// HOME-15, PIN-5: muting changes nothing stored, so channel-only mode allows it.
#[test]
fn mute_in_channel_only_mode() {
    let mut t = T::new();
    t.app.set.chan_only = true;
    t.tap(Key::Star);
    assert_eq!(mutes(&mut t), [0b001]);
}
