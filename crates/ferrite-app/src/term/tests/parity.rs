//! The pages that match the radio's own screens (TERM-48 to TERM-52; docs/research/vt100-parity.md).

use super::*;
use crate::screens::Screen;

/// TERM-48: the Radio page: A/B/C make a slot active; F tunes the active slot's VFO; N puts it on a channel; V
/// back to its VFO; the quick settings change, and go back to their default, as MENU's; a refusal says why.
#[test]
fn radio_page() {
    let mut r = Rig::main();
    r.send(b"1");
    assert_eq!(r.term.page(), Page::Radio);
    assert!(r.vt.has("Quick settings") && r.vt.has("2  TX power"), "{}", r.vt.text());
    r.send(b"b");
    assert_eq!(r.app.active(), 1);
    assert!(r.vt.has("slot B is active"));
    r.send(b"f146.94\r");
    assert_eq!(r.app.tuned_freq(1), 14_694_000);
    assert!(r.vt.has("146.94000"), "{}", r.vt.text());
    r.send(b"fabc\r");
    assert!(r.vt.has("Not a frequency"), "{}", r.vt.text());
    // a channel by its number, then V/M back to the VFO
    let c = r.store.by_number(1).unwrap();
    r.send(std::format!("n{}\r", c.num + 1).as_bytes());
    assert_eq!(r.app.slot(1).ch.map(|x| x.num), Some(c.num));
    assert!(r.app.slot(1).on_channel());
    r.send(b"v");
    assert!(!r.app.slot(1).on_channel());
    r.send(b"n4001\r");
    assert!(r.vt.has("A channel number, 1 to 4000"), "{}", r.vt.text());
    // TX power (MENU 2): a step, then # back to the default
    r.send(b"2");
    let p = r.app.slot(1).cfg().power;
    r.send(b"\x1b[C");
    assert_ne!(r.app.slot(1).cfg().power, p);
    assert!(r.vt.has("* "), "{}", r.vt.text());
    r.send(b"#");
    assert_eq!(r.app.slot(1).cfg().power, p);
    // the APRS slot is APRS mode's (APRS-19)
    r.app.set_aprs_mode(true);
    r.send(b"c");
    r.send(b"f146.52\r");
    assert!(r.vt.has("held by APRS mode") || r.vt.has("Slot C held"), "{}", r.vt.text());
    // S: a scan starts as SIDE2 starts it
    r.send(b"a");
    r.send(b"s");
    assert!(matches!(r.app.screen, Screen::Scan(_)), "{:?}", r.app.screen);
}

/// TERM-49: the APRS page: heard and messages; W writes a message and it is queued as Compose sends it; Enter marks
/// a message read; B a beacon now.
#[test]
fn aprs_page() {
    let mut r = Rig::main();
    r.send(b"5");
    assert_eq!(r.term.page(), Page::Aprs);
    let first = r.store.data().station(0).unwrap();
    assert!(r.vt.has(core::str::from_utf8(first.call.as_bytes()).unwrap()), "{}", r.vt.text());
    r.send(b"\x1b[C");
    assert!(r.vt.has("Messages ("), "{}", r.vt.text());
    let n = r.store.data().messages();
    r.send(b"wve3abc\r");
    assert!(r.vt.has("To VE3ABC: "), "{}", r.vt.text());
    r.send(b"Hello from the terminal\r");
    assert!(r.vt.has("Message queued"), "{}", r.vt.text());
    assert_eq!(r.store.data().messages(), n + 1);
    let m = r.store.data().message(0).unwrap();
    assert_eq!((m.peer.as_bytes(), m.text.as_bytes()), (&b"VE3ABC"[..], &b"Hello from the terminal"[..]));
    // an unread one, read with Enter
    let i = (0..r.store.data().messages()).find(|&i| r.store.data().message(i).is_some_and(|m| m.state == crate::data::MsgState::Unread)).unwrap();
    for _ in 0..i {
        r.send(b"\x1b[B");
    }
    r.send(b"\r");
    assert_eq!(r.store.data().message(i).unwrap().state, crate::data::MsgState::Read);
    // R replies: the sender's callsign as the recipient
    r.send(b"r");
    let peer = r.store.data().message(i).unwrap().peer;
    assert!(r.vt.has(&std::format!("To {}: ", core::str::from_utf8(peer.as_bytes()).unwrap())), "{}", r.vt.text());
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    // Enter on a heard station writes to it
    r.send(b"\x1b[D\r");
    assert!(r.vt.has(&std::format!("To {}: ", core::str::from_utf8(r.store.data().station(0).unwrap().call.as_bytes()).unwrap())), "{}", r.vt.text());
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    r.send(b"b");
    assert!(r.vt.has("Beacon now") || r.vt.has("Not sent"), "{}", r.vt.text());
}

/// TERM-50: the GPS page shows the radio's position page as text and the satellites; G steps 0 6 1.
#[test]
fn gps_page() {
    let mut r = Rig::main();
    r.send(b"6");
    assert_eq!(r.term.page(), Page::Gps);
    assert!(r.vt.has("Satellites in view") && r.vt.has("PRN"), "{}", r.vt.text());
    let g = r.app.set.gps;
    r.send(b"g");
    assert_ne!(r.app.set.gps, g);
}

/// TERM-51: the Remote page shows the radio's screen as text; its keys are the radio's (Enter is MENU, Esc BACK);
/// H goes home; Q leaves.
#[test]
fn remote_page() {
    let mut r = Rig::main();
    r.send(b"8");
    assert_eq!(r.term.page(), Page::Remote);
    r.send(b"h");
    assert!(r.vt.has("screen Home"), "{}", r.vt.text());
    r.send(b"\r");
    assert!(matches!(r.app.screen, Screen::Quick(_)), "{:?}", r.app.screen);
    r.send(b"\x1b");
    r.now += 100;
    r.poll();
    assert!(matches!(r.app.screen, Screen::Home), "{:?}", r.app.screen);
    assert_eq!(r.term.page(), Page::Remote);
    r.send(b"q");
    assert_eq!(r.term.page(), Page::Main);
}

/// TERM-52: a settings row that is a page opens the terminal's own (Heard, Position), or the radio's in Remote.
#[test]
fn settings_rows_open_pages() {
    let mut r = Rig::main();
    r.send(b"451");
    assert_eq!(r.term.page(), Page::Aprs);
    let mut r = Rig::main();
    r.send(b"462");
    assert_eq!(r.term.page(), Page::Gps);
    let mut r = Rig::main();
    r.send(b"433"); // 0 3 3: the frequency counter, a page on the radio
    assert_eq!(r.term.page(), Page::Remote);
    assert!(!matches!(r.app.screen, Screen::Home | Screen::Group(_)), "{:?}", r.app.screen);
    assert!(r.vt.has("screen "), "{}", r.vt.text());
}
