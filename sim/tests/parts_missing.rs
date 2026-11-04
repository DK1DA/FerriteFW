//! UPD-21 – UPD-23 (decision 335): the simulator with no SPI parts (`--no-parts`), as a radio whose update file's
//! parts weren't written: every user of a part falls back. A menu note is left out, the self-test says the parts
//! are missing, the terminal is the plain command line (TERM-47) and refusals say the text pack is missing, and the
//! boot screen shows the wordmark without its logo. One test: the parts are the process's.

mod common;

use common::Radio;
use ferrite_ui::fb::FrameBuffer;

/// UPD-21, UPD-22, UPD-23: no parts, every fallback.
#[test]
fn no_parts_every_fallback() {
    assert_eq!(ferrite_sim::parts::none(), 0);
    assert!(!ferrite_ui::logo::present() && !ferrite_app::text::present());
    let mut r = Radio::new();
    r.menu("0 1").expect_text("Watch").expect_no_text("Which slots are heard besides the active one.");
    r.menu("0 9").type_("8").press(ferrite_core::keys::Key::Left).expect_text("0 of 3").expect_text("Text pack missing");
    r.menu("0 9").type_("8").expect_text("PolyForm Noncommercial"); // About's licence: the MCU's flash (SELF-10)
    // The terminal: the plain command line, `help` saying the pack is missing, a refusal's fallback.
    r.sim.term_feed(b"\r");
    r.wait(400);
    let s = String::from_utf8_lossy(&r.sim.term_take()).to_string();
    assert!(s.contains("terminal: type a command") && s.ends_with("> "), "{s:?}");
    r.sim.term_feed(b"help\r");
    r.wait(400);
    let s = String::from_utf8_lossy(&r.sim.term_take()).to_string();
    assert!(s.contains("the text pack is missing"), "{s:?}");
    r.sim.term_feed(b"ch\r");
    r.wait(400);
    let s = String::from_utf8_lossy(&r.sim.term_take()).to_string();
    assert!(s.contains("text pack is missing"), "{s:?}");
    r.sim.term_feed(b"get 0 7 1\r");
    r.wait(400);
    let s = String::from_utf8_lossy(&r.sim.term_take()).to_string();
    assert!(s.contains("0 7 1 Theme"), "commands still answer: {s:?}");
    // The boot screen: the wordmark, no logo (the logo's square is the background).
    let checks = [ferrite_ui::boot::Check { label: "MCU", value: "", state: ferrite_ui::boot::State::Ok }];
    let info = ferrite_ui::boot::BootInfo { version: "v0", callsign: None, line: None, owner: None, checks: &checks, note: None };
    let mut fb = FrameBuffer::new();
    ferrite_ui::boot::draw(&mut fb, &info);
    let bg = fb.at(2, 40);
    assert!((64..176).all(|x| (28..140).all(|y| fb.at(x, y) == bg)), "no logo");
    assert!((60..180).any(|x| (150..190).any(|y| fb.at(x, y) != bg)), "the wordmark");
}
