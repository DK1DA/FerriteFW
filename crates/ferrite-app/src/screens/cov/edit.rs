//! The VFO editor (EDIT-1 to EDIT-7) and the DTMF dialer and memories (chapter 17).

use super::*;
use crate::data::Data;
use ferrite_core::msg::SlotLive;

/// EDIT-1, EDIT-7, MENU-18: hold SLOT on a VFO: the grid; # resets a setting (then Undo is offered); on the
/// APRS slot the settings are greyed and the note says why, in amber; the other slots' lines name a channel and
/// a scan's hits.
#[test]
fn vfo_editor() {
    let mut t = T::ready();
    let b = (t.app.active + 1) % 3;
    t.app.set_channel(b, 0, 0, &t.s);
    let c = (t.app.active + 2) % 3;
    t.live.slots[c as usize] = SlotLive { freq: ferrite_core::freq::mhz(446), scanning: true, hits: 2, ..Default::default() };
    t.wait(100);
    t.hold(Key::Slot);
    assert_eq!(t.name(), "Edit");
    assert_eq!(t.screen().mockup(), Some("HomeEdit.png"));
    t.shows(&String::from_utf8_lossy(t.app.slot(b).ch.unwrap().name()));
    t.shows("2 hits");
    let sq = t.app.slot(t.app.active).cfg().squelch;
    t.knob(2);
    t.reads("hints \"Knob\" \"Change\" \"*\" \"Undo\" \"BACK\" \"Done\"");
    t.tap(Key::Hash);
    assert_ne!(t.app.slot(t.app.active).cfg().squelch, sq.wrapping_add(2), "# put the default back");
    // C as the APRS slot: greyed, the note in amber
    t.tap(Key::Exit);
    t.app.set_aprs_mode(true);
    while t.app.active() != crate::aprs::APRS_SLOT {
        t.tap(Key::Slot);
    }
    t.wait(3500);
    t.app.screen = Screen::Edit(crate::screens::edit::Edit { cell: 0 });
    t.draw();
    assert!(t.px(185, 210, ferrite_ui::gfx::AMBER) > 0, "why it is greyed, in amber");
}

fn digits(t: &mut T, ds: &[u8]) {
    for &d in ds {
        t.tap(Key::Digit(d));
    }
}

/// DTMF-1, DTMF-11, DTMF-12: the dialer types * and #, holding 1-4 types A-D; hold MENU keeps the digits as the
/// first empty memory, or says to type some first.
#[test]
fn dialer_keeps_a_memory() {
    let mut t = T::new();
    t.open(Dest::DtmfDialer);
    t.hold(Key::Menu);
    assert_eq!(t.toast(), "DTMF / type digits first");
    digits(&mut t, &[1, 2]);
    t.tap(Key::Star);
    t.tap(Key::Hash);
    t.hold(Key::Digit(1));
    t.hold(Key::Digit(4));
    t.shows("12*#AD");
    let before = (0..crate::data::DTMF_MEMS).filter(|&i| t.s.dtmf(i).is_some()).count();
    t.hold(Key::Menu);
    assert!(t.toast().starts_with("DTMF saved / memory "), "{}", t.toast());
    assert_eq!((0..crate::data::DTMF_MEMS).filter(|&i| t.s.dtmf(i).is_some()).count(), before + 1);
}

/// DTMF-12: with every memory used, keeping says so.
#[test]
fn memories_full() {
    let mut t = T::new();
    for i in 0..crate::data::DTMF_MEMS {
        t.s.request(crate::data::Op::Dtmf(i, crate::data::Dtmf::parse(b"123")));
    }
    t.open(Dest::DtmfDialer);
    digits(&mut t, &[5]);
    t.hold(Key::Menu);
    assert_eq!(t.toast(), "DTMF memories / all 16 are used");
    t.open(Dest::DtmfMemories);
    t.tap(Key::Slot); // nothing in the list
    assert!(matches!(t.screen(), Screen::Dtmf(_)));
}
