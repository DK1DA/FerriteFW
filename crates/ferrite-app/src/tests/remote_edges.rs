//! Remote keys and remote PTT at their edges (LINK-19, LINK-20, LINK-28): each refusal and its words.

use super::{store, T};
use crate::remote::{KeyRefusal, Press, BY_BT, BY_CABLE};
use ferrite_core::keys::Key;

fn menu() -> u8 {
    crate::uistate::key_code(b"MENU").unwrap()
}

/// LINK-19: a code that isn't a key, or is a PTT, is refused with the stock codes named; a key held by the link
/// can't be pressed again until it is let go (down again is fine); a key held on the radio is refused.
#[test]
fn remote_key_refusals() {
    let mut t = T::new();
    let s = store();
    let now = t.t;
    assert_eq!(t.app.remote_press(99, Press::Short, BY_CABLE, now, &s), Err(KeyRefusal::Invalid("no such key: stock key codes 0-21")));
    assert_eq!(KeyRefusal::Invalid("x").text(), "x");
    let up = crate::uistate::key_code(b"UP").unwrap();
    assert_eq!(t.app.remote_press(up, Press::Down, BY_CABLE, now, &s), Ok(()));
    assert_eq!(t.app.remote_press(up, Press::Down, BY_CABLE, now, &s), Ok(()), "down again: still held");
    assert_eq!(t.app.remote_press(up, Press::Short, BY_CABLE, now, &s), Err(KeyRefusal::Invalid("held: send up first")));
    // LINK-28: another port presses only once these are let go
    assert_eq!(t.app.remote_press(menu(), Press::Short, BY_BT, now, &s), Err(KeyRefusal::Busy));
    assert_eq!(KeyRefusal::Busy.text(), "busy: a long press is still held");
    assert_eq!(t.app.remote_press(up, Press::Up, BY_CABLE, now, &s), Ok(()));
    assert_eq!(t.app.remote_keys_held(), 0);
    // a key held on the radio
    t.app.key_down(Key::Down, now, &s);
    let down = crate::uistate::key_code(b"DOWN").unwrap();
    assert_eq!(t.app.remote_press(down, Press::Short, BY_CABLE, now, &s), Err(KeyRefusal::Invalid("held on the radio")));
    t.app.key_up(Key::Down, now + 50, &s);
}

/// LINK-19: while a remote long press is held, keys and knob steps are busy; the knob takes 1 to 20 steps.
#[test]
fn long_press_holds_off_keys_and_knob() {
    let mut t = T::new();
    let s = store();
    let now = t.t;
    assert_eq!(t.app.remote_knob(0, now, &s), Err(KeyRefusal::Invalid("knob: 1 to 20 steps either way")));
    assert_eq!(t.app.remote_knob(21, now, &s), Err(KeyRefusal::Invalid("knob: 1 to 20 steps either way")));
    let up = crate::uistate::key_code(b"UP").unwrap();
    assert_eq!(t.app.remote_press(up, Press::Long, BY_CABLE, now, &s), Ok(()));
    assert!(t.app.remote_key_busy());
    let down = crate::uistate::key_code(b"DOWN").unwrap();
    assert_eq!(t.app.remote_press(down, Press::Short, BY_CABLE, now, &s), Err(KeyRefusal::Busy));
    assert_eq!(t.app.remote_knob(1, now, &s), Err(KeyRefusal::Busy));
    t.wait(1000);
    assert!(!t.app.remote_key_busy(), "let go on time");
    t.app.radio.keyed = Some(0);
    assert_eq!(t.app.remote_knob(1, t.t, &s), Err(KeyRefusal::Invalid("transmitting: remote keys wait for the unkey")));
}

/// LINK-20: remote PTT: a slot that isn't the active one or B is refused, as is one past C; keyed already on the
/// radio, it is "in progress"; on the APRS slot or compose, no voice.
#[test]
fn remote_ptt_refusals() {
    let mut t = T::new();
    let s = store();
    let now = t.t;
    assert_eq!(t.app.remote_key(2, BY_CABLE, now, &s), Err("that slot isn't active: the active slot or B"));
    assert_eq!(t.app.remote_key(7, BY_CABLE, now, &s), Err("no such slot"));
    t.app.radio.keyed = Some(0);
    assert!(t.app.remote_key(0, BY_CABLE, now, &s).is_err());
    t.app.radio.keyed = None;
    t.app.open(crate::screens::nav::Dest::Compose);
    assert_eq!(t.app.remote_key(0, BY_CABLE, now, &s), Err("APRS: no voice there"));
}
