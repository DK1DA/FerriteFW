//! The tone hint (chapter 22, TSRCH-13 and TSRCH-16): the card's "Heard 88.5" in each home layout, and quick
//! settings' Tones row using it, with tone search's TX question and its # save on a channel.

use super::{store, T};
use crate::chan::ChannelStore;
use crate::screens::confirm::{Ask, Confirm};
use crate::screens::quick::Quick;
use crate::screens::shared::{heard_text, HEARD_W};
use crate::slot::{CTCSS, DCS, DCS_FLAG, DCS_INV};
use crate::Screen;
use ferrite_core::keys::Key;
use ferrite_core::msg::{Param, RadioCmd};
use ferrite_ui::assets::UI_12;
use ferrite_ui::fb::FrameBuffer;
use ferrite_ui::gfx::{text4_width, AMBER};
use ferrite_ui::text::Buf;
use ferrite_ui::theme::map;

/// TSRCH-13: "Heard 88.5" and "Heard D023I"; every standard tone and code, both polarities, fits the card's
/// widened state field on one line.
#[test]
fn heard_text_fits() {
    let mut b = Buf::<16>::new();
    heard_text(885, &mut b);
    assert_eq!(b.as_bytes(), b"Heard 88.5");
    heard_text(DCS_FLAG | DCS_INV | 0o023, &mut b);
    assert_eq!(b.as_bytes(), b"Heard D023I");
    let all = CTCSS.iter().copied().chain(DCS.iter().flat_map(|&c| [DCS_FLAG | c, DCS_FLAG | DCS_INV | c]));
    for t in all.chain([2541, 1300]) {
        heard_text(t, &mut b);
        assert!(text4_width(b.as_bytes(), &UI_12) <= HEARD_W, "{:?} is too wide", core::str::from_utf8(b.as_bytes()));
    }
}

/// TSRCH-13: a hint replaces the slot's state on its card, in amber, in all three home layouts; it changes no
/// slot and opens no screen.
#[test]
fn hint_shows_on_the_card() {
    // Amber pixels in the state's corner of slot A's header (the widened field at the right, top of the card).
    let amber = |t: &mut T| {
        let mut fb = FrameBuffer::new();
        t.render(&mut fb);
        (20..80).flat_map(|y| (146..232).map(move |x| (x, y))).filter(|&(x, y)| fb.at(x, y) == map(AMBER)).count()
    };
    for layout in 0..3 {
        let mut t = T::new();
        t.app.set_layout(layout);
        t.live.slots[0].squelch_open = true;
        t.wait(200);
        let without = amber(&mut t);
        t.live.slots[0].heard_tone = 885;
        t.wait(200);
        let (state, colour) = t.app.state(0, &t.live);
        assert_eq!((state, colour), ("heard", AMBER));
        assert_eq!((t.app.screen(), t.app.active()), (Screen::Home, 0), "nothing moves (decision 81)");
        let with = amber(&mut t);
        assert!(with > without + 20, "layout {layout}: the hint in amber ({without} -> {with} px)");
    }
}

/// Quick settings with the active slot's Tones row highlighted (MENU 4).
fn tones_row(t: &mut T) {
    t.tap(Key::Menu);
    t.tap(Key::Digit(4));
    assert_eq!(t.app.screen(), Screen::Quick(Quick { row: 3 }));
}

/// TSRCH-16: with a hint, OK on Tones sets the heard tone as RX and asks about TX (tone search's question); OK
/// sets TX too. Without one, OK opens the tone editor as before.
#[test]
fn quick_tones_uses_the_hint() {
    let mut t = T::new();
    tones_row(&mut t);
    t.tap(Key::Menu);
    assert!(!matches!(t.app.screen(), Screen::Confirm(_)), "no hint: the editor");
    let mut t = T::new();
    t.live.slots[0].squelch_open = true;
    t.live.slots[0].heard_tone = 885;
    t.wait(100);
    tones_row(&mut t);
    t.cmds();
    t.tap(Key::Menu);
    assert_eq!(t.app.slot(0).cfg().rx_tone, 885);
    assert!(t.cmds().contains(&RadioCmd::SlotParam { slot: 0, param: Param::RxTone, value: 885 }));
    assert!(matches!(t.app.screen(), Screen::Confirm(Confirm { ask: Ask::ToneTx(885), .. })));
    assert_eq!(t.app.slot(0).cfg().tx_tone, 0, "TX only after the question");
    t.tap(Key::Menu);
    assert_eq!(t.app.slot(0).cfg().tx_tone, 885);
    assert_eq!(t.app.screen(), Screen::Quick(Quick { row: 3 }), "back where it was asked");
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"RX and TX tone set"));
    // BACK at the question: RX only.
    let mut t = T::new();
    t.live.slots[0].heard_tone = 1000;
    t.wait(100);
    tones_row(&mut t);
    t.tap(Key::Menu);
    t.tap(Key::Exit);
    assert_eq!((t.app.slot(0).cfg().rx_tone, t.app.slot(0).cfg().tx_tone), (1000, 0));
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"RX tone set"));
}

/// TSRCH-16, TSRCH-9: on a channel the heard tone loads into the slot without touching the record; # in the
/// question saves it there.
#[test]
fn quick_tones_on_a_channel() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    assert!(t.app.slot(0).on_channel());
    t.live.slots[0].heard_tone = 1000;
    t.wait(100);
    tones_row(&mut t);
    t.tap(Key::Menu);
    t.tap(Key::Exit);
    assert_eq!(t.app.slot(0).cfg().rx_tone, 1000, "loaded");
    assert_eq!(store().zone_at(0, 0).unwrap().cfg.rx_tone, 0, "the record unchanged");
    assert!(t.app.toast.is_some_and(|x| x.body.as_bytes() == b"loaded, not saved"));
    let mut t = T::new();
    t.tap(Key::VfoMem);
    t.live.slots[0].heard_tone = 1000;
    t.wait(100);
    tones_row(&mut t);
    t.tap(Key::Menu);
    t.cmds();
    t.tap(Key::Hash);
    assert!(t.app.toast.is_some_and(|x| x.head.as_bytes() == b"Tone saved"));
    assert_eq!(t.app.screen(), Screen::Quick(Quick { row: 3 }));
}
