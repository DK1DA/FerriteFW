//! First-run setup's pages one by one (chapter 4) and the Extend TX warning.

use super::*;
use crate::screens::setup::{ExtendTx, Setup};

fn page(t: &T) -> u8 {
    match t.screen() {
        Screen::Setup(s) => s.page,
        s => panic!("not setup: {}", s.name()),
    }
}

/// Setup from the menu (MENU 0 9 7 1), on page `n` (0-based).
fn at(n: u8) -> T {
    let mut t = T::new();
    t.open(Dest::Setup);
    for _ in 0..n {
        t.tap(Key::Menu);
    }
    assert_eq!(page(&t), n);
    t
}

/// SETUP-2: eight pages in order, each with its heading drawn and its mock-up named; the top bar counts them.
#[test]
fn every_page_draws_its_heading() {
    let heads = ["Welcome to Ferrite", "Your callsign", "Region", "Owner card", "Time", "Look", "Battery Save", "All set"];
    let shots = ["SetupWelcome.png", "SetupCall.png", "SetupRegion.png", "SetupOwner.png", "SetupTime.png", "SetupLook.png", "SetupBattery.png", "SetupSummary.png"];
    let mut t = at(0);
    for (i, (h, m)) in heads.iter().zip(shots).enumerate() {
        assert_eq!(page(&t), i as u8);
        t.shows(h);
        t.shows(&std::format!(" {} of 8", i + 1));
        t.reads(&std::format!("setup page={} of=8", i + 1));
        assert_eq!(t.screen().mockup(), Some(m));
        if i == 2 {
            t.tap(Key::Menu); // OK on the focused region chooses it and goes on
        } else if i < 7 {
            t.tap(Key::Menu);
        }
    }
    let mut s = Setup::new(false);
    s.page = 9;
    assert_eq!(Screen::Setup(s).mockup(), None, "no page past the summary");
}

/// SETUP-4: the hint bar says Next/Previous/Skip, Start on the summary, Done for a page on its own.
#[test]
fn hints_follow_the_page() {
    let mut t = at(1);
    t.reads("hints \"OK\" \"Next\" \"BACK\" \"Previous\" \"#\" \"Skip\"");
    t.tap(Key::Hash);
    t.reads("hints \"OK\" \"Start\" \"BACK\" \"Previous\"");
    t.tap(Key::Hash); // # on the summary starts the radio too
    assert_eq!(t.screen(), Screen::Home);
    assert!(t.app.settings().setup_done);
    crate::screens::setup::callsign(&mut t.app);
    t.draw();
    t.reads("hints \"OK\" \"Done\" \"BACK\" \"Back\"");
    t.tap(Key::Hash); // # does nothing on a page on its own
    assert!(matches!(t.screen(), Screen::Setup(_)));
    t.tap(Key::Exit);
    assert_eq!(t.screen(), Screen::Home, "BACK returns from the callsign page alone");
}

/// SETUP-4: BACK on the first page (from the menu, not first boot) leaves setup.
#[test]
fn back_on_the_first_page_leaves() {
    let mut t = T::new();
    t.open(Dest::Setup);
    t.tap(Key::Exit);
    assert_eq!(t.screen(), Screen::Home);
}

/// SETUP-12, SETUP-13: the callsign's tip changes with the * mode, and over 6 characters warns in amber.
#[test]
fn callsign_tips() {
    let mut t = at(1);
    t.shows("digits type");
    t.tap(Key::Star); // digits
    t.shows("digits \u{b7} * letters");
    for d in [1, 2, 3, 4, 5, 6, 7] {
        t.tap(Key::Digit(d));
    }
    assert_eq!(t.app.callsign(), b"1234567");
    t.shows("over 6 characters: no APRS");
    assert!(t.px(170, 190, ferrite_ui::gfx::AMBER) > 20, "the warning is amber");
    t.tap(Key::Star); // letters again
    t.tap(Key::Left);
    assert_eq!(t.app.callsign(), b"123456");
    // 1 cycles / and 1 for a callsign, 0 only 0
    t.tap(Key::Digit(1));
    t.tap(Key::Digit(1));
    assert_eq!(t.app.callsign(), b"1234561");
    t.wait(1200);
    t.tap(Key::Digit(0));
    t.tap(Key::Digit(0));
    assert_eq!(t.app.callsign(), b"12345610", "0 has only the digit: a quick second tap stays on it");
}

/// SETUP-3, SETUP-8: the region page's rows wrap to Extend TX; the knob moves without choosing; OK on Extend TX
/// asks with its warning, and \u{25c0}\u{25b6} on it turn it off again once warned.
#[test]
fn region_page_rows_and_extend_tx() {
    let mut t = at(2);
    t.knob(1);
    t.knob(-1);
    t.tap(Key::Right);
    t.reads("\"Canada\" \"\" sel");
    assert_eq!(t.app.settings().region_name(), "Not set");
    t.tap(Key::Up);
    t.tap(Key::Up);
    t.reads("\"Extend TX\" \"Off\" sel");
    t.shows("Extend TX");
    t.tap(Key::Menu); // OK on Extend TX: the warning first
    assert!(matches!(t.screen(), Screen::ExtendTx(_)));
    t.shows("You are responsible for every transmission.");
    t.shows("Emergency frequencies still ask for a second PTT.");
    t.reads("hints \"OK\" \"Turn on\" \"BACK\" \"Keep off\"");
    t.tap(Key::Digit(5)); // other keys do nothing
    assert!(matches!(t.screen(), Screen::ExtendTx(_)));
    t.tap(Key::Menu);
    assert!(t.app.settings().extend_tx);
    assert_eq!(page(&t), 2, "back on the region page");
    t.reads("\"Extend TX\" \"On\"");
    t.tap(Key::Left); // warned once: straight off
    assert!(!t.app.settings().extend_tx);
    assert!(matches!(t.screen(), Screen::Setup(_)));
    t.tap(Key::Down); // wraps to the first region
    t.reads("\"United States\" \"\" sel");
}

/// SETUP-3: the region page opened on its own (as an editor) chooses and returns.
#[test]
fn region_page_alone_returns() {
    let mut t = T::new();
    t.open(Dest::Setup);
    t.app.screen = Screen::Setup(Setup::single(2));
    t.draw();
    t.tap(Key::Down);
    t.tap(Key::Menu);
    assert_eq!(t.app.settings().region_name(), "Canada");
    assert_eq!(t.screen(), Screen::Home);
}

/// SETUP-11: the owner card's fields take letters in ABC, abc and 123 modes (* changes); \u{25c0} deletes; up and
/// down move between the three fields.
#[test]
fn owner_card_typing() {
    let mut t = at(3);
    t.shows("ABC \u{b7} * changes");
    t.tap(Key::Digit(2));
    t.tap(Key::Digit(2)); // B
    t.tap(Key::Star); // abc
    t.shows("abc \u{b7} * changes");
    t.wait(1100);
    t.tap(Key::Digit(3)); // d
    t.reads("\"Name\" \"Bd\" sel");
    t.tap(Key::Star); // 123
    t.shows("123 \u{b7} * changes");
    t.tap(Key::Digit(4));
    t.tap(Key::Digit(4));
    t.reads("\"Name\" \"Bd44\" sel");
    t.tap(Key::Left);
    t.reads("\"Name\" \"Bd4\" sel");
    t.tap(Key::Star); // back to ABC
    t.shows("ABC \u{b7} * changes");
    t.tap(Key::Down);
    t.tap(Key::Digit(1));
    t.reads("\"Phone\" \".\" sel");
    t.tap(Key::Up);
    t.tap(Key::Up); // wraps to E-mail
    t.reads("\"E-mail\" \"\" sel");
    t.tap(Key::Right); // \u{25b6} does nothing on text
    t.knob(1);
    t.reads("\"E-mail\" \"\" sel");
}

/// SETUP-5: the time page's three rows turn with the knob and write the settings 0 6 x.
#[test]
fn time_page_turns_each_row() {
    let mut t = at(4);
    t.dev.gps = GpsState::Fix;
    t.draw();
    t.shows(", GPS");
    let s0 = *t.app.settings();
    t.knob(1);
    assert_ne!(t.app.settings().tz, s0.tz);
    t.tap(Key::Down);
    t.knob(1);
    t.reads("\"Clock\" \"12 h\" sel");
    t.tap(Key::Down);
    t.knob(-1);
    t.reads("\"Units\" \"Nautical\" sel");
    t.shows("Nautical");
    t.dev.gps = GpsState::Searching;
    t.tap(Key::Down); // wraps to the time zone
    t.reads("\"Time zone\"");
    t.hides(", GPS");
}

/// SETUP-5: the look page turns the home layout (with its previews) and the theme.
#[test]
fn look_page_layout_and_theme() {
    let mut t = at(5);
    for name in ["Single + scope", "Watch + activity", "Watch"] {
        t.knob(1);
        t.reads(&std::format!("\"Home\" \"{name}\" sel"));
        t.shows(name);
    }
    t.tap(Key::Down);
    t.knob(1);
    t.reads("\"Theme\" \"Daylight\" sel");
    t.knob(-1);
    t.reads("\"Theme\" \"Dark\" sel");
}

/// SETUP-5: Battery Save's rows are its levels; moving to one sets it, the trade-off shown under them.
#[test]
fn battery_page_moves_set_the_level() {
    let mut t = at(6);
    let b0 = t.app.bsave;
    t.tap(Key::Down);
    assert_eq!(t.app.bsave, (b0 + 1) % 4);
    t.knob(1);
    assert_eq!(t.app.bsave, (b0 + 2) % 4);
    t.tap(Key::Up);
    t.tap(Key::Up);
    t.tap(Key::Up);
    assert_eq!(t.app.bsave, (b0 + 3) % 4);
    t.reads("\"chosen\" sel");
    t.shows(if t.app.bsave == 0 { "hears everything" } else { "a signal can be heard up to" });
    t.tap(Key::Down);
    t.shows(if t.app.bsave == 0 { "hears everything" } else { "a signal can be heard up to" });
}

/// SETUP-9, SETUP-10: the summary says no callsign and what that means, the region, the clock from GPS, and
/// the channels stored (none yet with an empty store).
#[test]
fn summary_rows_and_note() {
    let mut t = at(7);
    t.app.set.clock_gps = true;
    t.draw();
    t.reads("\"Callsign\" \"none\"");
    t.reads("No callsign: automatic transmissions stay off.");
    t.reads(", from GPS");
    t.shows("No callsign");
    // An empty store: "none yet", drawn grey.
    let empty = crate::chan::Table { zones: &[], channels: &[] };
    let mut l = crate::term::cmd::Line::new();
    let lines: Vec<String> = (0..)
        .map_while(|i| crate::uistate::line(&t.app, &empty, &t.dev, &t.live, i, &mut l).then(|| String::from_utf8_lossy(l.as_bytes()).into_owned()))
        .collect();
    assert!(lines.iter().any(|l| l.contains("\"Channels\" \"none yet\"")), "{lines:?}");
    t.app.render(&mut t.fb, &t.live, &t.dev, &empty);
    // With a callsign, the note keeps only how to add channels.
    t.app.set.set_callsign(b"VA7FER");
    t.draw();
    t.reads("\"Callsign\" \"VA7FER\"");
    assert!(!t.ui().contains("No callsign"));
}

/// SETUP-8: the warning drawn alone, then BACK keeps Extend TX off.
#[test]
fn extend_tx_warning_back_keeps_off() {
    let mut t = T::new();
    t.open(Dest::ExtendTx);
    assert_eq!(t.screen(), Screen::ExtendTx(ExtendTx));
    t.shows("This may be illegal where you are");
    t.tap(Key::Exit);
    assert!(!t.app.settings().extend_tx);
    assert_eq!(t.screen(), Screen::Home);
}
