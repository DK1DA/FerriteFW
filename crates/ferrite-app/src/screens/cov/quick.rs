//! Quick settings (MENU-1): on a broadcast slot (FM's rows), on a channel, a heard tone, and the hints.

use super::*;
use ferrite_core::freq::mhz;

fn row(t: &T) -> u8 {
    match t.screen() {
        Screen::Quick(q) => q.row,
        s => panic!("not quick settings: {}", s.name()),
    }
}

/// BCR-28: on a slot in the broadcast band, quick settings are FM's: the frequency and "Broadcast FM" at the
/// top, its four options then More; \u{25c0}\u{25b6}, the knob and OK change one; 0 or OK on More go on.
#[test]
fn broadcast_rows() {
    let mut t = T::ready();
    t.app.set_vfo(0, mhz(100) + 50_000);
    t.wait(100);
    t.tap(Key::Menu);
    assert_eq!(t.name(), "Quick");
    assert_eq!(t.screen().mockup(), Some("MenuQuick.png"));
    t.shows("100.5 MHz");
    t.shows("Broadcast FM");
    t.shows("Radio monitor");
    t.shows("More");
    t.reads("hints \"^v\" \"Move\" \"OK\" \"Open\" \"BACK\" \"Back\"");
    let step = t.app.set.fm_step;
    t.tap(Key::Right);
    assert_ne!(t.app.set.fm_step, step);
    t.tap(Key::Down);
    t.reads("hints \"^v\" \"Move\" \"Knob\" \"Change\" \"BACK\" \"Back\"");
    t.knob(1);
    t.tap(Key::Menu);
    t.tap(Key::Up);
    t.tap(Key::Up); // wraps to More
    assert_eq!(row(&t), 5);
    t.shows("Every setting, by group.");
    t.knob(1); // nothing on More
    t.tap(Key::Digit(2));
    assert_eq!(row(&t), 1);
    t.tap(Key::Slot); // nothing
    t.tap(Key::Exit);
    assert_eq!(t.screen(), Screen::Home);
    t.tap(Key::Menu);
    t.tap(Key::Digit(0));
    assert_eq!(t.name(), Dest::Advanced.name());
}

/// MENU-1: on a channel the head names it; MENU 8 there edits the channel; on a VFO it saves one.
#[test]
fn channel_row() {
    let mut t = T::ready();
    let a = t.app.active;
    t.app.set_channel(a, 0, 0, &t.s);
    t.wait(100);
    t.tap(Key::Menu);
    let num = t.app.slot(a).ch.unwrap().num;
    let mut l = ferrite_ui::text::Buf::<32>::new();
    crate::slot::chan_label(num, &mut l);
    t.shows(&String::from_utf8_lossy(l.as_bytes()));
    t.tap(Key::Digit(8));
    t.tap(Key::Menu);
    assert!(matches!(t.screen(), Screen::ChEdit(_)), "{}", t.name());
    let mut t = T::ready();
    t.tap(Key::Menu);
    t.tap(Key::Digit(8));
    t.tap(Key::Menu);
    assert!(matches!(t.screen(), Screen::Save(_)), "{}", t.name());
}

/// MENU-22, TSRCH-16: after a change the hint bar offers Undo; on Advanced, Open; with a tone heard, the Tones
/// row says it and OK uses it.
#[test]
fn hints_and_heard_tone() {
    let mut t = T::ready();
    t.tap(Key::Menu);
    t.knob(1); // the squelch, one up
    t.reads("hints \"Knob\" \"Change\" \"*\" \"Undo\" \"BACK\" \"Back\"");
    t.tap(Key::Star);
    t.app.undo = None;
    t.tap(Key::Digit(1));
    t.tap(Key::Up); // wraps to the tenth: Advanced
    assert_eq!(row(&t), 9);
    t.reads("hints \"^v\" \"Move\" \"OK\" \"Open\" \"BACK\" \"Back\"");
    let a = t.app.active as usize;
    t.live.slots[a].heard_tone = 885; // the Radio task heard 88.5 on the slot
    t.tap(Key::Digit(4)); // Tones
    assert_eq!(row(&t), 3);
    t.shows("heard 88.5");
    t.reads("hints \"^v\" \"Move\" \"OK\" \"Use heard\" \"BACK\" \"Back\"");
}
