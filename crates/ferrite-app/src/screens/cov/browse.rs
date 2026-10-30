//! The channel browser (C8-C10): the views' help in the picker, Recent, Nearby without positions, and the
//! actions' renumber, move and BACK.

use super::*;
use crate::chan::ChannelStore;
use crate::screens::browse::{Browse, Mode, PICKER};

fn br(t: &T) -> Browse {
    match t.screen() {
        Screen::Browse(b) => b,
        s => panic!("not the browser: {}", s.name()),
    }
}

/// The picker on the view `name` (after the zones).
fn view(t: &mut T, k: u16) {
    t.open(Dest::Browse);
    let z = t.s.zones() as u16;
    t.app.screen = Screen::Browse(Browse { view: PICKER, cur: crate::screens::lists::Cursor { sel: z + k, top: z + k - 8.min(z + k) }, ..Browse::new() });
    t.draw();
}

/// CHAN-14, CHAN-25, CHAN-29: the picker's help names each view.
#[test]
fn picker_help() {
    let mut t = T::new();
    view(&mut t, 0);
    t.shows("Every channel, zoned or not");
    view(&mut t, 1);
    t.shows("Every channel on this band");
    view(&mut t, 6);
    t.shows("Channels heard lately, from the event log");
}

/// CHAN-25: Recent is empty until a channel is used, then lists it, newest first.
#[test]
fn recent_view() {
    let mut t = T::new();
    view(&mut t, 6);
    t.tap(Key::Menu);
    t.shows("Nothing recent");
    let c = t.s.zone_at(0, 1).unwrap();
    t.s.used(c.num);
    t.draw();
    t.shows("newest first");
    t.shows(&String::from_utf8_lossy(c.name()));
}

/// CHAN-29: Nearby with a fix but no channel with a position says how they are set (the terminal, the link: LINK-53).
#[test]
fn nearby_without_positions() {
    let mut t = T::new();
    t.s.hide_positions(true);
    view(&mut t, 7);
    t.tap(Key::Menu);
    t.shows("No channels with a position");
}

/// C10: renumber to a free number (said in a toast), a move to the zone it is in changes nothing; BACK from the
/// actions returns to the list.
#[test]
fn renumber_and_move() {
    let mut t = T::new();
    t.open(Dest::Browse);
    t.tap(Key::Menu); // the first zone
    t.tap(Key::Menu); // the first channel's actions
    assert!(matches!(br(&t).mode, Mode::Actions(0)));
    t.tap(Key::Exit);
    assert_eq!(br(&t).mode, Mode::List);
    let c = t.s.zone_at(0, 0).unwrap();
    t.tap(Key::Menu);
    t.knob(3); // Renumber
    t.tap(Key::Menu);
    assert_eq!(br(&t).mode, Mode::Renumber);
    let free = (c.num + 1..1000).find(|&n| t.s.by_num(n).is_none()).unwrap();
    for _ in c.num..free {
        t.knob(1);
    }
    t.tap(Key::Menu);
    assert_eq!(t.toast(), std::format!("{} / CH {:03}", String::from_utf8_lossy(c.name()), free + 1));
    assert!(t.s.by_num(free).is_some(), "renumbered");
    // Move to the zone it is in: nothing changes
    t.tap(Key::Menu);
    t.knob(2); // Move
    t.tap(Key::Menu);
    t.app.toast = None;
    let z = t.s.by_num(free).unwrap().0.zone;
    t.app.screen = Screen::Browse(Browse { mode: Mode::Move(z), ..br(&t) });
    t.tap(Key::Menu);
    assert_eq!(t.toast(), "", "the same zone: no move");
    assert_eq!(br(&t).mode, Mode::List);
}
