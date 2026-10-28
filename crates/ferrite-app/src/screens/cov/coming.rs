//! The placeholder for a screen not built yet. Every destination has its own screen now (nav's fallback arm
//! can't be reached), so these put it up directly: it must still work for the next new one.

use super::*;
use crate::screens::coming::Coming;

fn coming(t: &mut T, dest: Dest) {
    t.open(Dest::Status);
    t.app.screen = Screen::Coming(Coming { dest });
    t.draw();
}

/// MENU-5: a settings group's placeholder names itself and its chapter, follows the menu path with digits,
/// times out as menus do, and BACK returns.
#[test]
fn group_placeholder() {
    let mut t = T::new();
    coming(&mut t, Dest::Group(9));
    t.shows(": coming");
    t.shows("Manual chapter");
    t.shows("Digits follow the menu path");
    t.reads("hints \"1-9\" \"Open\" \"BACK\" \"Back\"");
    assert_eq!(t.screen().timeout(), Some(crate::MENU_TIMEOUT_MS));
    let title = t.ui();
    assert!(title.contains("title=\""), "{title}");
    t.tap(Key::Digit(1)); // MENU 0 9 1
    assert!(matches!(t.screen(), Screen::Group(g) if (g.g, g.sub) == (9, 1)), "{:?}", t.screen());
    coming(&mut t, Dest::Sub(9, 1));
    t.tap(Key::Digit(0)); // MENU 0 9 1 0: nothing there yet
    assert!(t.toast().contains("setting comes next"), "{}", t.toast());
    t.tap(Key::Exit);
    assert_ne!(t.screen(), Screen::Coming(Coming { dest: Dest::Sub(9, 1) }));
}

/// TX-22: a Transmit placeholder: PTT's release goes home; other placeholders key on PTT as anywhere.
#[test]
fn transmit_placeholder() {
    let mut t = T::ready();
    coming(&mut t, Dest::Transmit);
    t.reads("hints \"PTT\" \"Release: home\"");
    assert_eq!(t.screen().timeout(), None);
    t.ptt(true);
    assert!(matches!(t.screen(), Screen::Coming(_)), "the press does nothing here");
    t.ptt(false);
    assert_eq!(t.screen(), Screen::Home);
    coming(&mut t, Dest::EventLog);
    t.reads("hints \"BACK\" \"Back\"");
    t.hides("Digits follow");
    t.tap(Key::Digit(3)); // no menu path: nothing
    assert!(matches!(t.screen(), Screen::Coming(_)));
    t.ptt(true);
    assert!(matches!(t.screen(), Screen::Transmit(_)), "{}", t.name());
    t.ptt(false);
}
