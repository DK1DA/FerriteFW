//! Entry and editing (docs/screens.md group A): direct entry, the channel editor, the value editor, the tone
//! picker, text entry, save as channel, add repeater, confirm, PIN, setup, the shortcut map; and the start-up
//! state sync.

use super::*;
use crate::chan::{ChanWrite, FLAG_SKIP};
use crate::screens::chedit::ChEdit;
use crate::screens::confirm::Ask;
use crate::screens::nav::Dest;
use crate::slot::{DCS_FLAG, STEPS};

impl T {
    fn digits(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '*' => self.tap(Key::Star),
                d => self.tap(Key::Digit(d as u8 - b'0')),
            }
        }
    }

    fn writes(&mut self) -> Vec<ChanWrite> {
        let mut v = Vec::new();
        while let Some(w) = self.app.pop_write() {
            v.push(w);
        }
        v
    }
}

/// The old start-up queued 33 commands into a 16-place ring and the first went missing: every slot's
/// frequency and settings, and the active slot, watch and Battery Save, must all reach the Radio task.
#[test]
fn start_up_sends_every_slot_whole() {
    let mut app = App::new(crate::tests::set(), crate::tests::alert());
    let mut v = Vec::new();
    while let Some(c) = app.pop_cmd() {
        v.push(c);
    }
    for s in 0..3u8 {
        assert!(v.contains(&RadioCmd::TuneSlot { slot: s, freq: app.slot(s).freq() }), "slot {s} tuned");
        for (param, value) in slot::params(app.slot(s).cfg()) {
            assert!(v.contains(&RadioCmd::SlotParam { slot: s, param, value }), "slot {s} {param:?}");
        }
    }
    assert!(v.contains(&RadioCmd::Active(0)) && v.contains(&RadioCmd::Watch(0b011)));
    assert!(v.iter().any(|c| matches!(c, RadioCmd::BatterySave(_))));
    assert_eq!(v.len(), 3 + 3 * 10);
    // Stepping through channels faster than the commands drain: the slot goes out once, as it is now.
    let mut t = T::new();
    t.tap(Key::VfoMem);
    let s = store();
    for _ in 0..30 {
        t.app.knob(1, t.t, &s);
    }
    let v = t.cmds();
    let tunes: Vec<_> = v.iter().filter(|c| matches!(c, RadioCmd::TuneSlot { .. })).collect();
    assert_eq!(tunes, [&RadioCmd::TuneSlot { slot: 0, freq: t.freq() }], "one tune, the latest");
}

/// NAV-4, VFO-11 to VFO-14: typed frequencies, the point, snapping, HF refused.
#[test]
fn frequency_entry() {
    let mut t = T::new();
    t.digits("147225");
    assert!(matches!(t.app.screen(), Screen::Entry(_)));
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.freq(), mhz(147) + 22_500);
    assert!(t.cmds().contains(&RadioCmd::TuneSlot { slot: 0, freq: mhz(147) + 22_500 }));
    // UHF, typed with the point by itself, snapped to 12.5 kHz
    t.digits("44612");
    t.tap(Key::Menu);
    assert_eq!(t.freq(), mhz(446) + 12_500);
    assert_eq!(slot::band_name(t.freq()), "UHF");
    // BACK deletes, then leaves
    t.digits("12");
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    // Below 18 MHz: HF mode (VFO-14)
    let f = t.freq();
    t.digits("7*1");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Entry(_)), "stays, with the reason");
    assert_eq!(t.freq(), f);
    t.hold(Key::Exit);
    // 6 m with the point typed
    t.digits("52*525");
    t.tap(Key::Menu);
    assert_eq!(t.freq(), mhz(52) + 52_500);
}

/// NAV-4 in channel mode: a number, its name or "empty".
#[test]
fn channel_entry() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    t.digits("11");
    assert!(matches!(t.app.screen(), Screen::Entry(_)));
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::Home);
    let s = t.app.slot(0);
    assert_eq!((s.ch.unwrap().name(), s.zone, s.pos), (&b"North hill"[..], 2, 0));
    t.digits("999");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Entry(_)), "empty: stays");
}

/// EDIT-2, EDIT-5, CHAN-18, CHAN-19: the channel editor, renumbering, Home, undo, hold 0.
#[test]
fn channel_editor() {
    let mut t = T::new();
    t.tap(Key::VfoMem); // CH 001 Calling
    t.hold(Key::Slot);
    assert_eq!(t.app.screen(), Screen::ChEdit(ChEdit { cell: 0 }), "EDIT-2: the channel version");
    t.writes();
    // Down to the grid, then to Number (row 4, col 1) and renumber: the next free number up is CH 003.
    for k in [Key::Down, Key::Down, Key::Down, Key::Down, Key::Down, Key::Down, Key::Right] {
        t.tap(k);
    }
    assert_eq!(t.app.screen(), Screen::ChEdit(ChEdit { cell: 13 }));
    t.knob(1);
    assert_eq!(t.app.slot(0).ch.unwrap().num, 2);
    let w = t.writes();
    assert!(matches!(w[..], [ChanWrite::Put { ch, from: Some(0) }] if ch.num == 2), "{w:?}");
    // Home (EDIT-5, CHAN-18), then undo puts the number back and Home off.
    t.tap(Key::Right);
    t.knob(1);
    assert_eq!(t.app.settings().home_ch, Some(2));
    t.tap(Key::Star);
    assert_eq!(t.app.slot(0).ch.unwrap().num, 0);
    assert_eq!(t.app.settings().home_ch, None);
    assert!(matches!(t.writes()[..], [ChanWrite::Put { ch, from: Some(2) }] if ch.num == 0));
    // A flag: Skip.
    t.tap(Key::Up);
    t.tap(Key::Left); // Skip
    assert_eq!(t.app.screen(), Screen::ChEdit(ChEdit { cell: 10 }));
    t.knob(1);
    assert_ne!(t.app.slot(0).ch.unwrap().flags & FLAG_SKIP, 0);
    // A setting saves the channel too (EDIT-4).
    t.tap(Key::Up);
    t.tap(Key::Up); // Bandwidth
    t.writes();
    t.knob(1);
    assert!(!t.app.slot(0).cfg().narrow);
    assert!(matches!(t.writes()[..], [ChanWrite::Put { ch, .. }] if !ch.cfg.narrow));
    // Home on, leave, go to the VFO; hold 0 brings the home channel back (CHAN-19).
    t.app.settings_mut().home_ch = Some(10);
    t.tap(Key::Exit);
    t.tap(Key::VfoMem);
    assert!(!t.app.slot(0).on_channel());
    t.hold(Key::Digit(0));
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.app.slot(0).ch.unwrap().name(), b"North hill");
    assert!(t.app.slot(0).on_channel());
}

/// MENU-17, MENU-20, SQL-7: the bigger editor changes live; BACK puts the old value back, OK keeps.
#[test]
fn value_editor() {
    let mut t = T::new();
    t.tap(Key::Menu);
    t.tap(Key::Menu); // OK on Squelch
    assert!(matches!(t.app.screen(), Screen::Value(_)));
    t.knob(2);
    assert_eq!(t.app.slot(0).cfg().squelch, 6);
    t.tap(Key::Digit(9)); // digits type a level
    assert_eq!(t.app.slot(0).cfg().squelch, 9);
    t.tap(Key::Exit);
    assert_eq!(t.app.slot(0).cfg().squelch, 4, "BACK restores");
    assert_eq!(t.app.screen(), Screen::Quick(crate::screens::quick::Quick { row: 0 }));
    t.tap(Key::Digit(2));
    t.tap(Key::Menu); // TX power
    t.tap(Key::Right);
    t.tap(Key::Menu); // OK keeps
    assert_eq!(t.app.slot(0).cfg().power, 1);
    let mut fb = FrameBuffer::new();
    t.tap(Key::Digit(1));
    t.tap(Key::Menu);
    t.render(&mut fb);
    assert!(count_px(&fb, 44, 206, |c| c == BLUE) > 300, "the box and the levels");
}

/// TONE-2: RX and TX apart; digits type a tone; BACK puts both back.
#[test]
fn tone_picker() {
    let mut t = T::new();
    t.tap(Key::Menu);
    t.tap(Key::Digit(4));
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Tones(_)));
    t.digits("885");
    assert_eq!(t.app.slot(0).cfg().tx_tone, 885);
    assert!(t.cmds().contains(&RadioCmd::SlotParam { slot: 0, param: Param::TxTone, value: 885 }));
    t.tap(Key::Up); // RX
    t.knob(51); // past the 50 CTCSS tones: the first DCS code
    assert_eq!(t.app.slot(0).cfg().rx_tone, DCS_FLAG | 0o023);
    assert_eq!(t.app.slot(0).cfg().tx_tone, 885, "TX untouched");
    t.wait(2000);
    t.digits("754"); // on a DCS code, digits are octal
    assert_eq!(t.app.slot(0).cfg().rx_tone, DCS_FLAG | 0o754);
    t.tap(Key::Exit);
    assert_eq!((t.app.slot(0).cfg().rx_tone, t.app.slot(0).cfg().tx_tone), (0, 0), "BACK restores both");
}

/// KEY-12: multi-tap, case, space, delete; OK names the channel.
#[test]
fn text_entry_names_a_channel() {
    let mut t = T::new();
    t.tap(Key::VfoMem);
    t.hold(Key::Slot);
    t.tap(Key::Down); // Name
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Text(_)));
    while !t.app.text.is_empty() {
        t.tap(Key::Exit);
    }
    t.digits("44"); // H
    t.wait(1100);
    t.tap(Key::Star); // lower case
    t.digits("444"); // i
    t.tap(Key::Hash); // space
    t.tap(Key::Star); // 123
    t.digits("2");
    assert_eq!(t.app.text.as_bytes(), b"Hi 2");
    t.tap(Key::Exit);
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::ChEdit(ChEdit { cell: 1 }));
    assert_eq!(t.app.slot(0).ch.unwrap().name(), b"Hi ");
    assert!(matches!(t.writes()[..], [ChanWrite::Put { ch, from: None }] if ch.name() == b"Hi "));
    assert!(t.app.callsign().is_empty(), "a channel name is never the callsign");
}

/// Text entry for the callsign (Dest::Text(Callsign)) sets only the callsign; for Save as channel's name, the
/// form's draft only (nothing written until Save).
#[test]
fn text_entry_callsign_and_save_name() {
    use crate::screens::text::TextFor;
    let mut t = T::new();
    t.app.open(Dest::Text(TextFor::Callsign));
    t.digits("2"); // A
    t.tap(Key::Star); // digits
    t.digits("7");
    t.tap(Key::Menu);
    assert_eq!(t.app.callsign(), b"A7");
    assert!(t.writes().is_empty());
    // Save as channel: the name stays in the form until it saves.
    t.hold(Key::VfoMem);
    assert!(matches!(t.app.screen(), Screen::Save(_)));
    t.tap(Key::Down);
    t.tap(Key::Down); // Name
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Text(x) if x.to == TextFor::SaveName));
    t.digits("33"); // E
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Save(_)));
    assert_eq!(t.app.text.as_bytes(), b"E");
    assert!(t.writes().is_empty());
    assert_eq!(t.app.callsign(), b"A7", "the callsign is untouched");
}

/// VFO-9, CHAN-22: hold V/M saves the VFO as a channel at the first free number; below 64 MHz it can't.
#[test]
fn save_as_channel() {
    let mut t = T::new();
    t.hold(Key::VfoMem);
    assert!(matches!(t.app.screen(), Screen::Save(_)));
    t.tap(Key::Down);
    t.tap(Key::Down); // Name
    t.tap(Key::Menu);
    t.digits("7");
    t.tap(Key::Menu);
    assert!(matches!(t.app.screen(), Screen::Save(_)), "back on the form");
    t.tap(Key::Down);
    t.tap(Key::Menu); // Save
    assert_eq!(t.app.screen(), Screen::Home);
    let w = t.writes();
    let [ChanWrite::Put { ch, from: None }] = w[..] else { panic!("{w:?}") };
    assert_eq!((ch.num, ch.rx, ch.name(), ch.zone), (2, mhz(146) + 52_000, &b"P"[..], 0));
    assert!(!t.app.slot(0).on_channel(), "stays on the VFO");
    // 6 m: refused
    t.digits("52*525");
    t.tap(Key::Menu);
    t.hold(Key::VfoMem);
    t.tap(Key::Up);
    t.tap(Key::Menu);
    assert!(t.writes().is_empty());
}

/// MENU-10: Add repeater asks frequency, shift and tone, and ends in the channel editor.
#[test]
fn add_repeater() {
    let mut t = T::new();
    t.app.open(Dest::AddRepeater);
    t.tap(Key::Menu); // Frequency: type it
    t.digits("14694");
    t.tap(Key::Menu);
    let Screen::Save(s) = t.app.screen() else { panic!() };
    assert_eq!((s.freq, s.offset), (mhz(146) + 94_000, -60_000), "2 m below 147: minus");
    t.tap(Key::Down);
    t.tap(Key::Down); // Tone
    for _ in 0..13 {
        t.knob(1); // 100.0 Hz is the 13th CTCSS tone
    }
    t.tap(Key::Down);
    t.tap(Key::Down);
    t.tap(Key::Down);
    t.tap(Key::Down); // Save
    t.tap(Key::Menu);
    assert_eq!(t.app.screen(), Screen::ChEdit(ChEdit { cell: 0 }));
    let c = t.app.slot(0).ch.unwrap();
    assert!(t.app.slot(0).on_channel());
    assert_eq!((c.rx, c.cfg.offset, c.cfg.tx_tone), (mhz(146) + 94_000, -60_000, 1000));
}

/// MENU-23, PIN-6, PIN-7: reset asks, then asks again for the channels; BACK keeps them.
#[test]
fn confirm_reset() {
    let mut t = T::new();
    t.tap(Key::Menu);
    t.tap(Key::Right); // squelch 5
    t.tap(Key::Exit);
    t.hold(Key::Menu);
    t.digits("982");
    assert!(matches!(t.app.screen(), Screen::Confirm(c) if c.ask == Ask::Reset { channels: false }));
    t.tap(Key::Menu);
    assert_eq!(t.app.slot(0).cfg().squelch, 4, "settings back to default");
    assert!(matches!(t.app.screen(), Screen::Confirm(c) if c.ask == Ask::Reset { channels: true }));
    t.tap(Key::Exit);
    assert_eq!(t.app.screen(), Screen::Home);
    assert!(t.writes().is_empty(), "channels kept");
    // A channel delete names it and clears it from the slot.
    t.tap(Key::VfoMem);
    crate::screens::confirm::ask(&mut t.app, Ask::DeleteChannel(0));
    t.tap(Key::Menu);
    assert_eq!(t.writes(), [ChanWrite::Delete(0)]);
    assert!(!t.app.slot(0).on_channel());
}

/// PIN-1 to PIN-3, KEY-16: unlocking asks; three misses wait 30 s; right unlocks.
#[test]
fn pin_at_unlock() {
    let mut t = T::new();
    let p = t.app.settings_mut();
    (p.pin, p.pin_len, p.pin_mode) = (1234, 4, 2);
    t.hold(Key::Hash);
    assert!(t.app.locked());
    t.hold(Key::Hash);
    assert!(matches!(t.app.screen(), Screen::Pin(_)));
    for _ in 0..3 {
        t.digits("0000");
        t.tap(Key::Menu);
    }
    t.digits("1234"); // waiting: ignored
    t.tap(Key::Menu);
    assert!(t.app.locked());
    t.wait(30_100);
    t.digits("1234");
    t.tap(Key::Menu);
    assert!(!t.app.locked());
    assert_eq!(t.app.screen(), Screen::Home);
    // At power on, PTT does nothing until it is right.
    t.app.ask_pin();
    let s = store();
    t.app.key_down(Key::Ptt, t.t, &s);
    assert!(matches!(t.app.screen(), Screen::Pin(_)));
    t.app.key_up(Key::Ptt, t.t + 100, &s);
}

/// HOME-8: holding a digit shows the map; the release runs the shortcut; BACK cancels.
#[test]
fn shortcut_map() {
    let mut t = T::new();
    let s = store();
    t.app.key_down(Key::Digit(5), t.t, &s);
    t.wait(800);
    assert!(matches!(t.app.screen(), Screen::Shortcut(_)));
    assert_eq!(t.app.slot(0).cfg().power, 0, "nothing yet");
    t.app.key_up(Key::Digit(5), t.t, &s);
    assert_eq!(t.app.screen(), Screen::Home);
    assert_eq!(t.app.slot(0).cfg().power, 1, "run at the release");
    t.app.key_down(Key::Digit(6), t.t, &s);
    t.wait(800);
    t.tap(Key::Exit);
    t.app.key_up(Key::Digit(6), t.t, &s);
    assert!(t.app.slot(0).cfg().narrow, "cancelled");
}

/// Every string the group's screens draw is in the fonts (as `fonts_have_every_character_the_ui_draws`).
#[test]
fn group_a_strings_are_in_the_fonts() {
    use ferrite_ui::assets::{UI_12, UI_12B};
    let sources = [
        include_str!("../screens/forms.rs"),
        include_str!("../screens/entry.rs"),
        include_str!("../screens/chedit.rs"),
        include_str!("../screens/value.rs"),
        include_str!("../screens/tones.rs"),
        include_str!("../screens/text.rs"),
        include_str!("../screens/save.rs"),
        include_str!("../screens/confirm.rs"),
        include_str!("../screens/shortcut.rs"),
    ];
    let mut n = 0;
    for src in sources {
        for s in string_literals(src) {
            for c in s.chars().filter(|c| !('\x18'..='\x1b').contains(c) && *c != '\n') {
                assert!(UI_12.has(c) && UI_12B.has(c), "{c:?} in {s:?}");
                n += 1;
            }
        }
    }
    assert!(n > 2000, "{n}");
    let _ = STEPS;
}

/// Every group A screen draws in each of its states without leaving the content area.
#[test]
fn group_a_screens_draw() {
    let mut t = T::new();
    let mut fb = FrameBuffer::new();
    t.tap(Key::VfoMem);
    for dest in [Dest::Entry(1), Dest::ChannelEdit, Dest::Value(Item::Squelch), Dest::Value(Item::Step), Dest::Tones, Dest::Text(crate::screens::text::TextFor::Callsign), Dest::SaveChannel, Dest::AddRepeater, Dest::Confirm, Dest::ExtendTx, Dest::Pin, Dest::Setup] {
        t.app.open(dest);
        t.render(&mut fb);
        assert!(count_px(&fb, 21, 299, |c| c != BG) > 400, "{dest:?}");
        t.app.go_home();
        t.render(&mut fb);
    }
}
