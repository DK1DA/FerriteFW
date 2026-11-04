//! UPD-21 – UPD-23 (decision 335): the simulator reading its SPI parts from part files, as the radio reads them from
//! its SPI flash (`--parts DIR`): the screens, the terminal and the boot logo are what they are with the parts
//! built in. A part for another build, or a damaged one, is not used. One test: the parts are the process's.

mod common;

use common::Radio;
use ferrite_core::part;

fn plain(s: &[u8]) -> String {
    let s = String::from_utf8_lossy(s);
    let (mut out, mut esc) = (String::new(), false);
    for c in s.chars() {
        match (esc, c) {
            (false, '\x1b') => esc = true,
            (true, c) if c.is_ascii_alphabetic() => esc = false,
            (true, _) => {}
            (false, c) => out.push(c),
        }
    }
    out
}

/// UPD-21, UPD-22, UPD-23: parts from files, checked by build ID and CRC; then a menu note, the terminal's pages
/// and help, and the logo come from them.
#[test]
fn parts_from_files_are_used_and_checked() {
    let dir = std::env::temp_dir().join(format!("ferrite-parts-{}", std::process::id()));
    // Another build's parts and a damaged one first: neither is used (checked with the same reader).
    ferrite_sim::parts::write_all(&dir, "zzzzzzz");
    let mut chip = Vec::new();
    for k in &part::KINDS {
        chip.push((k.slot.start, std::fs::read(dir.join(k.file)).unwrap()));
    }
    let read_from = |chip: &Vec<(u32, Vec<u8>)>| {
        let chip = chip.clone();
        move |a: u32, b: &mut [u8]| {
            b.fill(0xFF);
            for (at, f) in &chip {
                if a >= *at && (a - at) as usize + b.len() <= f.len() {
                    b.copy_from_slice(&f[(a - at) as usize..][..b.len()]);
                }
            }
            true
        }
    };
    for k in &part::KINDS {
        assert_eq!(part::check(k, ferrite_core::BUILD, &mut read_from(&chip)), None, "{}: another build's", k.file);
    }
    ferrite_sim::parts::write_all(&dir, ferrite_core::BUILD);
    let mut chip: Vec<(u32, Vec<u8>)> = part::KINDS.iter().map(|k| (k.slot.start, std::fs::read(dir.join(k.file)).unwrap())).collect();
    for k in &part::KINDS {
        assert!(part::check(k, ferrite_core::BUILD, &mut read_from(&chip)).is_some(), "{}", k.file);
    }
    chip[0].1[100] ^= 0x40;
    assert_eq!(part::check(&part::TEXT, ferrite_core::BUILD, &mut read_from(&chip)), None, "a damaged text pack");

    // The simulator with the files: every part good.
    assert_eq!(ferrite_sim::parts::load(&dir), 0b111);
    assert!(ferrite_ui::logo::present());
    let mut r = Radio::new();
    r.menu("0").expect_text("Watch, resume, priority, ranges, skips.");
    r.menu("0 1").expect_text("Which slots are heard besides the active one.");
    r.menu("0 9").type_("8").press(ferrite_core::keys::Key::Left).expect_text("3 of 3");
    // The terminal: its pages (the template part), and `help` from the text pack.
    r.sim.term_size(80, 24);
    r.sim.term_feed(b"\r");
    r.wait(400);
    r.sim.term_feed(b"\r");
    r.wait(400);
    let menu = plain(&r.sim.term_take());
    assert!(menu.contains("Channels") && !menu.contains("> "), "the pages: {menu}");
    r.sim.term_feed(b":help\r");
    r.wait(600);
    let help = plain(&r.sim.term_take());
    assert!(help.contains("Commands: type : then one of these"), "{help}");
    let _ = std::fs::remove_dir_all(&dir);
}
